//! Bounded, activation-owned RTW staging. Heavy validation/export never runs in guest.
use crate::execution_context::ExecutionContext;
use crate::runtime::wasm::bindings::rintawa::engine::world_sessions::TransferStatus;
use crate::runtime::wasm::{RuntimeContextError, WasmHostState, WorldSessionError};
use rintawa_sdk::runtime_permissions::RuntimePermission;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;

const IDLE: Duration = Duration::from_secs(120);
const CHUNK: usize = 32 * 1024;
static TRANSFERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Arc<Self>, WorldSessionError> {
        TRANSFERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .map_err(|_| WorldSessionError::LimitExceeded)?;
        Ok(Arc::new(Self))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        TRANSFERS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Upload {
    directory: TempDir,
    _lease: File,
    file: File,
    expected: u64,
    written: u64,
}
enum Completed {
    Installed(String),
    Download {
        _directory: TempDir,
        _lease: File,
        file: File,
        size: u64,
    },
}
enum Phase {
    Upload(Upload),
    Pending(Receiver<Result<Completed, String>>),
    Ready(Completed),
    Failed(String),
}
pub(super) struct Transfer {
    pub(super) context: ExecutionContext,
    permit: Arc<Permit>,
    touched: Instant,
    phase: Phase,
}
impl Transfer {
    pub(super) fn expired(&self) -> bool {
        self.touched.elapsed() >= IDLE
    }
}
fn reason(mut value: String) -> String {
    if value.len() > 2048 {
        let mut end = 2048;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}
impl WasmHostState {
    fn transfer_context(
        &mut self,
        permission: bool,
    ) -> Result<ExecutionContext, WorldSessionError> {
        if !self.host_access_active {
            return Err(WorldSessionError::AccessNotActive);
        }
        let context = self
            .active_execution_context()
            .map_err(|e| match e {
                RuntimeContextError::Unavailable => WorldSessionError::Unavailable,
                _ => WorldSessionError::AccessNotActive,
            })?
            .clone();
        if context.scope.as_str() != "host" {
            return Err(WorldSessionError::PermissionDenied);
        }
        if permission {
            match self.runtime_permission_owner(RuntimePermission::WorldSessionWrite) {
                Ok(_) => {}
                Err(super::RuntimePermissionCheck::Denied) => {
                    self.package_transfers.retain(|_, t| t.context != context);
                    return Err(WorldSessionError::PermissionDenied);
                }
                Err(_) => return Err(WorldSessionError::Unavailable),
            }
        }
        self.package_transfers.retain(|_, t| !t.expired());
        Ok(context)
    }
    fn transfer_get(&mut self, handle: u64) -> Result<&mut Transfer, WorldSessionError> {
        let context = self.transfer_context(true)?;
        let transfer = self
            .package_transfers
            .get_mut(&handle)
            .filter(|t| t.context == context)
            .ok_or(WorldSessionError::NotFound)?;
        transfer.touched = Instant::now();
        Ok(transfer)
    }
    fn transfer_insert(
        &mut self,
        phase: Phase,
        permit: Arc<Permit>,
        context: ExecutionContext,
    ) -> Result<u64, WorldSessionError> {
        self.next_package_transfer = self
            .next_package_transfer
            .checked_add(1)
            .ok_or(WorldSessionError::LimitExceeded)?;
        let handle = self.next_package_transfer;
        self.package_transfers.insert(
            handle,
            Transfer {
                context,
                permit,
                touched: Instant::now(),
                phase,
            },
        );
        Ok(handle)
    }
    fn transfer_spawn(
        &mut self,
        handle: u64,
        job: impl FnOnce() -> Result<Completed, String> + Send + 'static,
    ) -> Result<(), WorldSessionError> {
        let transfer = self
            .package_transfers
            .get_mut(&handle)
            .ok_or(WorldSessionError::NotFound)?;
        let permit = transfer.permit.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("rtw-transfer".into())
            .spawn(move || {
                // Closing a pending handle cannot bypass the process-wide quota.
                let _permit = permit;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    .unwrap_or_else(|_| Err("package worker failed".into()));
                let _ = sender.send(result);
            });
        if worker.is_err() {
            transfer.phase = Phase::Failed("package worker unavailable".into());
            return Err(WorldSessionError::Unavailable);
        }
        transfer.phase = Phase::Pending(receiver);
        Ok(())
    }
    pub(super) fn transfer_upload(&mut self, size: u64) -> Result<u64, WorldSessionError> {
        let context = self.transfer_context(true)?;
        self.consume_world_session_mutation_budget()?;
        if size == 0 || size > rintawa_artifacts::RtwLimits::default().max_archive_bytes {
            return Err(WorldSessionError::MessageTooLarge);
        }
        let permit = Permit::acquire()?;
        let (directory, lease) = self
            .host_access
            .world_sessions
            .package_staging()
            .map_err(|_| WorldSessionError::Unavailable)?;
        let file = File::create(directory.path().join("upload.rtw"))
            .map_err(|_| WorldSessionError::Unavailable)?;
        self.transfer_insert(
            Phase::Upload(Upload {
                directory,
                _lease: lease,
                file,
                expected: size,
                written: 0,
            }),
            permit,
            context,
        )
    }
    pub(super) fn transfer_append(
        &mut self,
        handle: u64,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64, WorldSessionError> {
        if bytes.is_empty() || bytes.len() > CHUNK || bytes.len() > self.max_host_message_bytes {
            return Err(WorldSessionError::MessageTooLarge);
        }
        let transfer = self.transfer_get(handle)?;
        let Phase::Upload(upload) = &mut transfer.phase else {
            return Err(WorldSessionError::Rejected);
        };
        if offset != upload.written || bytes.len() as u64 > upload.expected.saturating_sub(offset) {
            return Err(WorldSessionError::Rejected);
        }
        // A failed OS write poisons this upload; never infer a resumable byte boundary.
        if upload.file.write_all(bytes).is_err() {
            transfer.phase = Phase::Failed("package staging write failed".into());
            return Err(WorldSessionError::Unavailable);
        }
        upload.written += bytes.len() as u64;
        Ok(upload.written)
    }
    pub(super) fn transfer_finish(&mut self, handle: u64) -> Result<(), WorldSessionError> {
        let transfer = self.transfer_get(handle)?;
        let Phase::Upload(upload) = &mut transfer.phase else {
            return Err(WorldSessionError::Rejected);
        };
        if upload.written != upload.expected {
            return Err(WorldSessionError::Rejected);
        }
        let Phase::Upload(upload) = std::mem::replace(
            &mut transfer.phase,
            Phase::Failed("package worker unavailable".into()),
        ) else {
            unreachable!()
        };
        let access = self.host_access.world_sessions.clone();
        self.transfer_spawn(handle, move || {
            let _lease = upload._lease;
            let path = upload.directory.path().join("upload.rtw");
            upload.file.sync_all().map_err(|e| e.to_string())?;
            drop(upload.file);
            let digest = access.install_package_file(&path)?;
            rintawa_artifacts::ArtifactDigest::parse(&digest)
                .map_err(|_| "invalid installed digest".to_string())?;
            Ok(Completed::Installed(digest))
        })
    }
    pub(super) fn transfer_export(&mut self, world_id: String) -> Result<u64, WorldSessionError> {
        let context = self.transfer_context(true)?;
        self.consume_world_session_mutation_budget()?;
        if !uuid::Uuid::parse_str(&world_id).is_ok_and(|id| id.to_string() == world_id) {
            return Err(WorldSessionError::InvalidWorldId);
        }
        let permit = Permit::acquire()?;
        let (directory, lease) = self
            .host_access
            .world_sessions
            .package_staging()
            .map_err(|_| WorldSessionError::Unavailable)?;
        let handle = self.transfer_insert(
            Phase::Failed("package worker unavailable".into()),
            permit,
            context,
        )?;
        let access = self.host_access.world_sessions.clone();
        let started = self.transfer_spawn(handle, move || {
            let path = directory.path().join("world.rtw");
            access.export_world_file(&world_id, &path)?;
            let file = File::open(path).map_err(|e| e.to_string())?;
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            if size == 0 || size > rintawa_artifacts::RtwLimits::default().max_archive_bytes {
                return Err("export exceeds RTW archive limit".into());
            }
            Ok(Completed::Download {
                _directory: directory,
                _lease: lease,
                file,
                size,
            })
        });
        if let Err(error) = started {
            self.package_transfers.remove(&handle);
            return Err(error);
        }
        Ok(handle)
    }
    pub(super) fn transfer_poll(
        &mut self,
        handle: u64,
    ) -> Result<TransferStatus, WorldSessionError> {
        let transfer = self.transfer_get(handle)?;
        if let Phase::Pending(receiver) = &transfer.phase {
            match receiver.try_recv() {
                Ok(Ok(result)) => transfer.phase = Phase::Ready(result),
                Ok(Err(error)) => transfer.phase = Phase::Failed(reason(error)),
                Err(TryRecvError::Empty) => {}
                Err(_) => transfer.phase = Phase::Failed("package worker disconnected".into()),
            }
        }
        Ok(match &transfer.phase {
            Phase::Upload(upload) => TransferStatus::Uploading(upload.written),
            Phase::Pending(_) => TransferStatus::Pending,
            Phase::Ready(Completed::Installed(digest)) => TransferStatus::Installed(digest.clone()),
            Phase::Ready(Completed::Download { size, .. }) => TransferStatus::Download(*size),
            Phase::Failed(message) => TransferStatus::Failed(message.clone()),
        })
    }
    pub(super) fn transfer_read(
        &mut self,
        handle: u64,
        offset: u64,
        maximum_bytes: u32,
    ) -> Result<Vec<u8>, WorldSessionError> {
        let maximum = maximum_bytes as usize;
        if maximum == 0 || maximum > CHUNK || maximum > self.max_host_message_bytes {
            return Err(WorldSessionError::MessageTooLarge);
        }
        let Phase::Ready(Completed::Download { file, size, .. }) =
            &mut self.transfer_get(handle)?.phase
        else {
            return Err(WorldSessionError::Rejected);
        };
        if offset > *size {
            return Err(WorldSessionError::Rejected);
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| WorldSessionError::Unavailable)?;
        let mut bytes = vec![0; maximum.min((*size - offset) as usize)];
        let read = file
            .read(&mut bytes)
            .map_err(|_| WorldSessionError::Unavailable)?;
        bytes.truncate(read);
        Ok(bytes)
    }
    pub(super) fn transfer_close(&mut self, handle: u64) -> Result<(), WorldSessionError> {
        let context = self.transfer_context(false)?;
        if !self
            .package_transfers
            .get(&handle)
            .is_some_and(|t| t.context == context)
        {
            return Err(WorldSessionError::NotFound);
        }
        self.package_transfers.remove(&handle);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host_access::{
            HostAccessError, HostAccessResult, WorldSessionAccess, WorldSessionAssetRef,
            WorldSessionSummary,
        },
        runtime::wasm::WorldSessionsHost,
    };
    use rintawa_sdk::{
        contracts::ComponentRef,
        types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
    };
    use std::sync::{Condvar, Mutex, atomic::AtomicBool};
    #[derive(Default)]
    struct Files {
        paths: Mutex<Vec<std::path::PathBuf>>,
        block: Mutex<bool>,
        wake: Condvar,
        entered: AtomicBool,
        installed: AtomicUsize,
    }
    impl WorldSessionAccess for Files {
        fn package_staging(&self) -> Result<(TempDir, File), String> {
            let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            let lock = File::create(dir.path().join("owner.lock")).map_err(|e| e.to_string())?;
            self.paths.lock().unwrap().push(dir.path().to_owned());
            Ok((dir, lock))
        }
        fn install_package_file(&self, path: &std::path::Path) -> Result<String, String> {
            self.entered.store(true, Ordering::Release);
            let mut blocked = self.block.lock().unwrap();
            while *blocked {
                blocked = self.wake.wait(blocked).unwrap();
            }
            self.installed.store(
                path.metadata().map_err(|e| e.to_string())?.len() as usize,
                Ordering::Release,
            );
            Ok(format!("sha256:{}", "a".repeat(64)))
        }
        fn export_world_file(&self, _: &str, path: &std::path::Path) -> Result<(), String> {
            std::fs::write(path, b"portable fixture").map_err(|e| e.to_string())
        }
        fn list_worlds(&self) -> HostAccessResult<Vec<WorldSessionSummary>> {
            Err(HostAccessError::Unavailable)
        }
        fn create_world(&self) -> HostAccessResult<WorldSessionSummary> {
            Err(HostAccessError::Unavailable)
        }
        fn set_metadata(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<WorldSessionAssetRef>,
        ) -> HostAccessResult<WorldSessionSummary> {
            Err(HostAccessError::Unavailable)
        }
        fn delete_world(&self, _: &str) -> HostAccessResult<()> {
            Err(HostAccessError::Unavailable)
        }
        fn set_active(&self, _: &str, _: bool) -> HostAccessResult<()> {
            Err(HostAccessError::Unavailable)
        }
    }
    fn state(files: Arc<Files>, id: &str, scope: &str) -> WasmHostState {
        let owner = ComponentRef::new(ExtensionInstanceId::new(id), ComponentId::new("shell"));
        let mut s = WasmHostState::new(owner.component_id.clone());
        s.host_access.world_sessions = files;
        s.transport
            .contexts
            .begin(owner.clone(), RuntimeScopeId::new(scope))
            .unwrap();
        s.transport.contexts.activate_instance(&owner.instance_id);
        s.begin_registration(
            ExtensionId::new("test"),
            owner.instance_id.clone(),
            RuntimeScopeId::new(scope),
        )
        .unwrap();
        s.finish_registration().unwrap();
        s.begin_guest_execution();
        s
    }
    fn grant(s: &mut WasmHostState) {
        s.runtime_permissions
            .grant(
                s.registered_owner().unwrap(),
                RuntimePermission::WorldSessionWrite,
            )
            .unwrap();
    }
    fn until(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn terminal(s: &mut WasmHostState, h: u64) -> TransferStatus {
        let mut result = None;
        until(|| {
            let value = WorldSessionsHost::poll_package_transfer(s, h).unwrap();
            if matches!(value, TransferStatus::Pending) {
                false
            } else {
                result = Some(value);
                true
            }
        });
        result.unwrap()
    }
    #[test]
    fn test_should_bound_disk_transfers_and_cleanup_exact_activations_and_abandoned_workers() {
        let files = Arc::new(Files::default());
        let mut s = state(files.clone(), "one", "host");
        assert!(matches!(
            WorldSessionsHost::begin_package_upload(&mut s, 1),
            Err(WorldSessionError::PermissionDenied)
        ));
        grant(&mut s);
        let mut other = state(files.clone(), "two", "host");
        grant(&mut other);
        let mut world = state(files.clone(), "world", "world:test");
        grant(&mut world);
        assert!(matches!(
            WorldSessionsHost::begin_package_upload(&mut world, 1),
            Err(WorldSessionError::PermissionDenied)
        ));
        assert!(matches!(
            WorldSessionsHost::begin_package_upload(&mut s, 0),
            Err(WorldSessionError::MessageTooLarge)
        ));
        assert!(matches!(
            WorldSessionsHost::begin_package_upload(&mut s, 1024 * 1024 * 1024 + 1),
            Err(WorldSessionError::MessageTooLarge)
        ));
        let size = 17 * 1024 * 1024;
        let h = WorldSessionsHost::begin_package_upload(&mut s, size).unwrap();
        assert!(matches!(
            WorldSessionsHost::append_package_upload(&mut s, h, 1, vec![0; 32]),
            Err(WorldSessionError::Rejected)
        ));
        assert!(matches!(
            WorldSessionsHost::append_package_upload(&mut s, h, 0, vec![0; CHUNK + 1]),
            Err(WorldSessionError::MessageTooLarge)
        ));
        for offset in (0..size).step_by(CHUNK) {
            assert_eq!(
                WorldSessionsHost::append_package_upload(&mut s, h, offset, vec![7; CHUNK])
                    .unwrap(),
                offset + CHUNK as u64
            );
        }
        s.begin_guest_execution();
        let peer = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        assert!(matches!(
            WorldSessionsHost::begin_package_upload(&mut other, 4),
            Err(WorldSessionError::LimitExceeded)
        ));
        WorldSessionsHost::close_package_transfer(&mut s, peer).unwrap();
        WorldSessionsHost::finish_package_upload(&mut s, h).unwrap();
        assert!(matches!(terminal(&mut s, h), TransferStatus::Installed(_)));
        assert_eq!(files.installed.load(Ordering::Acquire), size as usize);
        let export =
            WorldSessionsHost::begin_world_export(&mut s, uuid::Uuid::now_v7().to_string())
                .unwrap();
        assert!(matches!(
            terminal(&mut s, export),
            TransferStatus::Download(16)
        ));
        assert_eq!(
            WorldSessionsHost::read_package_transfer(&mut s, export, 0, 8).unwrap(),
            b"portable"
        );
        assert_eq!(
            WorldSessionsHost::read_package_transfer(&mut s, export, 0, 8).unwrap(),
            b"portable"
        );
        assert!(matches!(
            WorldSessionsHost::read_package_transfer(&mut s, export, 0, CHUNK as u32 + 1),
            Err(WorldSessionError::MessageTooLarge)
        ));
        assert!(matches!(
            WorldSessionsHost::read_package_transfer(&mut other, export, 0, 1),
            Err(WorldSessionError::NotFound)
        ));
        WorldSessionsHost::close_package_transfer(&mut s, h).unwrap();
        WorldSessionsHost::close_package_transfer(&mut s, export).unwrap();
        s.begin_guest_execution();
        let stale = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        let owner = s.registered_owner().unwrap();
        s.transport
            .contexts
            .begin(owner.clone(), RuntimeScopeId::new("host"))
            .unwrap();
        s.transport.contexts.activate_instance(&owner.instance_id);
        s.begin_guest_execution();
        assert!(matches!(
            WorldSessionsHost::poll_package_transfer(&mut s, stale),
            Err(WorldSessionError::NotFound)
        ));
        s.revoke_runtime_resources_for_owner(&owner);
        assert!(files.paths.lock().unwrap().iter().all(|p| !p.exists()));
        let expired = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        s.package_transfers.get_mut(&expired).unwrap().touched = Instant::now() - IDLE;
        assert!(matches!(
            WorldSessionsHost::poll_package_transfer(&mut s, expired),
            Err(WorldSessionError::NotFound)
        ));
        s.begin_guest_execution();
        let cleanup = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        s.runtime_permissions
            .revoke_component(&owner, &s.transport.contexts);
        WorldSessionsHost::close_package_transfer(&mut s, cleanup).unwrap();
        grant(&mut s);
        s.begin_guest_execution();
        let denied = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        s.runtime_permissions
            .revoke_component(&owner, &s.transport.contexts);
        assert!(matches!(
            WorldSessionsHost::poll_package_transfer(&mut s, denied),
            Err(WorldSessionError::PermissionDenied)
        ));
        assert!(s.package_transfers.is_empty());
        assert!(files.paths.lock().unwrap().iter().all(|p| !p.exists()));
        grant(&mut s);
        *files.block.lock().unwrap() = true;
        files.entered.store(false, Ordering::Release);
        s.begin_guest_execution();
        let pending = WorldSessionsHost::begin_package_upload(&mut s, 4).unwrap();
        WorldSessionsHost::append_package_upload(&mut s, pending, 0, b"hold".to_vec()).unwrap();
        WorldSessionsHost::finish_package_upload(&mut s, pending).unwrap();
        until(|| files.entered.load(Ordering::Acquire));
        WorldSessionsHost::close_package_transfer(&mut s, pending).unwrap();
        other.begin_guest_execution();
        let peer = WorldSessionsHost::begin_package_upload(&mut other, 4).unwrap();
        assert!(
            matches!(
                WorldSessionsHost::begin_package_upload(&mut s, 4),
                Err(WorldSessionError::LimitExceeded)
            ),
            "abandoning a worker must not bypass quota"
        );
        WorldSessionsHost::close_package_transfer(&mut other, peer).unwrap();
        *files.block.lock().unwrap() = false;
        files.wake.notify_all();
        until(|| TRANSFERS.load(Ordering::Acquire) == 0);
        assert!(files.paths.lock().unwrap().iter().all(|p| !p.exists()));
        other.begin_guest_execution();
        WorldSessionsHost::begin_package_upload(&mut other, 4).unwrap();
        drop(other);
        assert_eq!(TRANSFERS.load(Ordering::Acquire), 0);
        assert!(files.paths.lock().unwrap().iter().all(|p| !p.exists()));
    }
}
