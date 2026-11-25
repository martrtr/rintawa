//! Sequential permission-checked CAS readers owned by exact guest activations.

use crate::{
    execution_context::ExecutionContext,
    host_access::HostAccessError,
    runtime::wasm::{AssetStoreError as Error, RuntimePermissionCheck, WasmHostState, WitAssetRef},
};
use rintawa_sdk::runtime_permissions::RuntimePermission;
use std::{
    fs::File,
    io::Read,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_READER: AtomicU64 = AtomicU64::new(1);
pub(super) struct AssetReader {
    pub(super) context: ExecutionContext,
    file: File,
}
impl WasmHostState {
    fn asset_reader_context(&self) -> Result<ExecutionContext, Error> {
        if self.world_system_snapshot.is_some() || self.current_execution_owner().is_none() {
            return Err(Error::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::AssetRead)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => Error::PermissionDenied,
                RuntimePermissionCheck::Unavailable => Error::Unavailable,
            })?;
        self.execution_context.clone().ok_or(Error::AccessNotActive)
    }
    pub(super) fn open_asset_reader(&mut self, reference: WitAssetRef) -> Result<u64, Error> {
        let context = self.asset_reader_context()?;
        if self
            .asset_readers
            .values()
            .filter(|reader| reader.context == context)
            .count()
            >= 8
        {
            return Err(Error::Unavailable);
        }
        let handle = NEXT_READER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map_err(|_| Error::Unavailable)?;
        let file = self
            .host_access
            .asset_store
            .open_asset(&reference.digest, reference.size, &reference.media_type)
            .map_err(|error| match error {
                HostAccessError::InvalidDigest
                | HostAccessError::InvalidAsset
                | HostAccessError::NotFound => Error::InvalidAsset,
                _ => Error::Unavailable,
            })?;
        self.asset_readers
            .insert(handle, AssetReader { context, file });
        Ok(handle)
    }
    pub(super) fn read_asset_reader(
        &mut self,
        handle: u64,
        maximum: u32,
    ) -> Result<Vec<u8>, Error> {
        let context = self.asset_reader_context()?;
        if maximum == 0 {
            return Err(Error::InvalidAsset);
        }
        if maximum > 65536 {
            return Err(Error::MessageTooLarge);
        }
        let reader = self
            .asset_readers
            .get_mut(&handle)
            .filter(|reader| reader.context == context)
            .ok_or(Error::InvalidAsset)?;
        let mut bytes = vec![0; maximum as usize];
        let length = reader
            .file
            .read(&mut bytes)
            .map_err(|_| Error::Unavailable)?;
        bytes.truncate(length);
        Ok(bytes)
    }
    pub(super) fn close_asset_reader(&mut self, handle: u64) -> Result<(), Error> {
        let context = self
            .active_execution_context()
            .map_err(|_| Error::AccessNotActive)?
            .clone();
        if !self
            .asset_readers
            .get(&handle)
            .is_some_and(|reader| reader.context == context)
        {
            return Err(Error::InvalidAsset);
        }
        self.asset_readers.remove(&handle);
        Ok(())
    }
}
