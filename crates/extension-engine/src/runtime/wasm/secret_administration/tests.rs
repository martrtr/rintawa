//! Direct import checks for delegated identity, scope and cleanup after permission revoke.
use super::*;
use crate::{
    execution_context::VerifiedComponent,
    runtime::wasm::{WasmExecutionBudget, WasmHostServices},
    secrets::SecretManager,
};
use rintawa_sdk::{contracts::ComponentRef, manifest::ComponentPermissions};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
#[derive(Default)]
struct Access {
    next: AtomicU64,
    owners: Mutex<Vec<LivePolicyTarget>>,
    closed: AtomicU64,
}
impl SecretAccess for Access {
    fn begin(
        self: Arc<Self>,
        owner: &LivePolicyTarget,
        _: &str,
        _: Option<crate::SecretPromptAuthority>,
        _: Arc<AtomicBool>,
    ) -> crate::HostAccessResult<u64> {
        self.owners.lock().unwrap().push(owner.clone());
        Ok(self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }
    fn status(&self, _: &LivePolicyTarget, _: u64) -> crate::HostAccessResult<String> {
        Ok("{}".into())
    }
    fn close(&self, _: &LivePolicyTarget, _: u64, _: bool) -> crate::HostAccessResult<()> {
        self.closed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
fn fixture() -> (WasmHostState, Arc<Access>) {
    let access = Arc::new(Access::default());
    let mut services = WasmHostServices::standalone(SecretManager::default());
    services.host_access.secret_access = Some(access.clone());
    let state = WasmHostState::with_host_services_and_budget(
        "provider".into(),
        services,
        false,
        &WasmExecutionBudget::default(),
    );
    for (instance, scope) in [
        ("shell", "host"),
        ("other", "host"),
        ("world", "world:test"),
    ] {
        let owner = ComponentRef::new(instance, "ui");
        state
            .transport
            .contexts
            .begin_verified(
                owner.clone(),
                scope.into(),
                Some(VerifiedComponent {
                    digest: rintawa_artifacts::ArtifactDigest::sha256(instance.as_bytes()),
                    published_key: None,
                    permissions: ComponentPermissions {
                        runtime: vec![RuntimePermission::SecretManage],
                        ..Default::default()
                    },
                }),
            )
            .unwrap();
        state
            .transport
            .contexts
            .activate_instance(&owner.instance_id);
    }
    (state, access)
}
#[test]
fn delegated_import_never_inherits_provider_rights_and_world_cannot_manage() {
    let (mut state, access) = fixture();
    state
        .runtime_permissions
        .grant(
            ComponentRef::new("provider", "provider"),
            RuntimePermission::SecretManage,
        )
        .unwrap();
    let shell = ComponentRef::new("shell", "ui");
    state.begin_delegated_task_execution(shell.clone());
    assert!(matches!(
        admin::Host::begin(&mut state, "{}".into()),
        Err(admin::Error::Denied)
    ));
    assert!(access.owners.lock().unwrap().is_empty());
    state
        .runtime_permissions
        .grant(shell.clone(), RuntimePermission::SecretManage)
        .unwrap();
    let h = admin::Host::begin(&mut state, "{}".into()).unwrap();
    assert_eq!(access.owners.lock().unwrap()[0].instance_id, "shell");
    let other = ComponentRef::new("other", "ui");
    state
        .runtime_permissions
        .grant(other.clone(), RuntimePermission::SecretManage)
        .unwrap();
    state.begin_delegated_task_execution(other);
    assert!(matches!(
        admin::Host::status(&mut state, h),
        Err(admin::Error::NotFound)
    ));
    assert!(matches!(
        admin::Host::release(&mut state, h),
        Err(admin::Error::NotFound)
    ));
    let world = ComponentRef::new("world", "ui");
    state
        .runtime_permissions
        .grant(world.clone(), RuntimePermission::SecretManage)
        .unwrap();
    state.begin_delegated_task_execution(world);
    assert!(matches!(
        admin::Host::begin(&mut state, "{}".into()),
        Err(admin::Error::Denied)
    ));
}
#[test]
fn revoke_denies_poll_but_owner_can_cleanup_and_trap_discards_owned_handles() {
    let (mut state, access) = fixture();
    let owner = ComponentRef::new("shell", "ui");
    state
        .runtime_permissions
        .grant(owner.clone(), RuntimePermission::SecretManage)
        .unwrap();
    state.begin_delegated_task_execution(owner.clone());
    let h = admin::Host::begin(&mut state, "{}".into()).unwrap();
    state
        .runtime_permissions
        .grants
        .write()
        .unwrap()
        .get_mut(&owner)
        .unwrap()
        .remove(&RuntimePermission::SecretManage);
    assert!(matches!(
        admin::Host::status(&mut state, h),
        Err(admin::Error::Denied)
    ));
    admin::Host::cancel(&mut state, h).unwrap();
    admin::Host::release(&mut state, h).unwrap();
    assert!(state.secret_operations.is_empty());
    state
        .runtime_permissions
        .grant(owner.clone(), RuntimePermission::SecretManage)
        .unwrap();
    admin::Host::begin(&mut state, "{}".into()).unwrap();
    state.revoke_runtime_resources_for_owner(&owner);
    assert!(state.secret_operations.is_empty());
    assert!(access.closed.load(Ordering::Relaxed) >= 3);
}
