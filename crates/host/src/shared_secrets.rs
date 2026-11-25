//! Home-local shared entries and an explicitly selected native credential backend.

#[cfg(target_os = "linux")]
mod files;
#[cfg(all(test, target_os = "linux"))]
mod tests;

use crate::{HostHome, HostResult};
use rintawa_extension_engine::{SecretManager, SecretVault, SystemSecretVault};
use rintawa_sdk::secrets::{SecretAccessError, SecretPath, SecretValue};
#[cfg(target_os = "linux")]
use serde::Deserialize;
use serde::Serialize;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[cfg(target_os = "linux")]
const MAX_ENTRIES: usize = 256;
#[cfg(target_os = "linux")]
const MAX_LABEL: usize = 128;
#[cfg(target_os = "linux")]
const MAX_VALUE: usize = 65536;
#[cfg(target_os = "linux")]
const MAX_TOTAL: usize = 16 * 1024 * 1024;
// A JSON string can encode each source byte as six ASCII bytes (e.g. NUL).
#[cfg(target_os = "linux")]
const MAX_RECORD: usize = 6 * (MAX_VALUE + MAX_LABEL) + 256;

/// Explicit installation-local selection. Changing it never copies credentials.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SecretBackend {
    /// Platform credentials; legacy direct paths remain account-global.
    #[default]
    System,
    /// Linux plaintext records protected by OS ownership and permissions.
    LocalFile,
}

impl SecretBackend {
    /// Canonical CLI/configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::LocalFile => "local-file",
        }
    }
}

impl std::str::FromStr for SecretBackend {
    type Err = SecretStorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "local-file" => Ok(Self::LocalFile),
            _ => Err(SecretStorageError::InvalidInput),
        }
    }
}

/// Trusted-manager metadata. It deliberately contains no value or physical path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ManagedSecretEntry {
    /// Immutable random identity, independent of the mutable label.
    pub id: Uuid,
    /// User display text, never an authorization key.
    pub label: String,
    /// Increases on rename or replacement; checked by administrative edits.
    pub revision: u64,
}

/// Sanitized native administration failures. Backend/record text is never included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SecretStorageError {
    /// Invalid label, value, revision or backend selection.
    #[error("invalid secret input (label: 1..128 bytes; value: 1..65536 UTF-8 bytes)")]
    InvalidInput,
    /// Missing managed entry in the selected backend/home.
    #[error("shared secret is missing in the selected backend")]
    NotFound,
    /// Concurrent administrative edit; inspect current metadata before retrying.
    #[error("shared secret changed; inspect it before retrying")]
    RevisionConflict,
    /// An entry or byte budget would be exceeded.
    #[error("shared secret storage quota exceeded")]
    QuotaExceeded,
    /// Ownership, type, permissions, links or parent directories are unsafe.
    #[error("unsafe secret storage ownership, permissions or file type")]
    UnsafeStorage,
    /// A bounded record or selection is malformed.
    #[error("secret storage is corrupt")]
    CorruptStorage,
    /// I/O or platform vault failure, without raw backend diagnostics.
    #[error("secret storage is unavailable")]
    Unavailable,
    /// Publication completed but its directory sync failed.
    #[error("secret change was published but durability is unknown; recheck before retrying")]
    DurabilityUnknown,
    /// Staged value or temporary-file cleanup failed.
    #[error("secret operation failed and staged data cleanup failed")]
    CleanupFailed,
}

// Never derive Debug or expose this record through the manager/bridge API.
#[cfg(target_os = "linux")]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: Uuid,
    label: String,
    revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    // System values are outside the file, but still count towards decoded quotas.
    value_bytes: usize,
}

#[cfg(target_os = "linux")]
impl Record {
    fn metadata(&self) -> ManagedSecretEntry {
        ManagedSecretEntry {
            id: self.id,
            label: self.label.clone(),
            revision: self.revision,
        }
    }
}

pub(crate) fn entry_path(id: Uuid) -> SecretPath {
    SecretPath::parse(format!("managed.entry_{}", id.simple()))
        .expect("UUID forms a canonical path")
}

fn entry_id(path: &SecretPath) -> Option<Uuid> {
    let suffix = path.as_str().strip_prefix("managed.entry_")?;
    let id = Uuid::parse_str(suffix).ok()?;
    (id.simple().to_string() == suffix).then_some(id)
}

#[derive(Clone)]
pub(crate) struct ManagedSecrets {
    #[cfg(target_os = "linux")]
    home: PathBuf,
    backend: SecretBackend,
    system: Arc<dyn SecretVault>,
}

impl ManagedSecrets {
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn test_system(home: PathBuf, vault: Arc<dyn SecretVault>) -> Self {
        Self {
            home,
            backend: SecretBackend::System,
            system: vault,
        }
    }

    pub(crate) fn selected(home: &HostHome) -> Result<Self, SecretStorageError> {
        Ok(Self {
            #[cfg(target_os = "linux")]
            home: home.root().to_owned(),
            backend: home.secret_backend()?,
            system: Arc::new(SystemSecretVault),
        })
    }

    pub(crate) fn manager(&self) -> SecretManager {
        SecretManager::with_vault(Arc::new(self.clone()))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn backend_name(&self) -> &'static str {
        self.backend.as_str()
    }

    #[cfg(target_os = "linux")]
    fn import_legacy(
        &self,
        path: &SecretPath,
        label: &str,
    ) -> Result<ManagedSecretEntry, SecretStorageError> {
        if self.backend != SecretBackend::System {
            return Err(SecretStorageError::InvalidInput);
        }
        let value = self
            .system
            .load(path)
            .map_err(|_| SecretStorageError::Unavailable)?
            .ok_or(SecretStorageError::NotFound)?;
        self.create(label, &value)
    }

    #[cfg(target_os = "linux")]
    fn directory(&self, create: bool) -> Result<Option<files::Directory>, SecretStorageError> {
        let Some(root) = files::Directory::credentials(&self.home, create)? else {
            return Ok(None);
        };
        root.child(self.backend.as_str(), create)
    }

    #[cfg(target_os = "linux")]
    fn records(&self, directory: &files::Directory) -> Result<Vec<Record>, SecretStorageError> {
        let mut records = Vec::new();
        let mut total = 0usize;
        for name in directory.names()? {
            if name.starts_with(".tmp_") {
                continue;
            }
            let id = Uuid::parse_str(&name).map_err(|_| SecretStorageError::CorruptStorage)?;
            if name != id.simple().to_string() || records.len() >= MAX_ENTRIES {
                return Err(SecretStorageError::CorruptStorage);
            }
            let record = self
                .read_record(directory, id)?
                .ok_or(SecretStorageError::CorruptStorage)?;
            total += record.value_bytes;
            if total > MAX_TOTAL {
                return Err(SecretStorageError::CorruptStorage);
            }
            records.push(record);
        }
        Ok(records)
    }

    #[cfg(target_os = "linux")]
    fn read_record(
        &self,
        directory: &files::Directory,
        id: Uuid,
    ) -> Result<Option<Record>, SecretStorageError> {
        let Some(bytes) = directory.read(&id.simple().to_string(), MAX_RECORD)? else {
            return Ok(None);
        };
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| SecretStorageError::CorruptStorage)?;
        if record.id != id
            || record.revision == 0
            || record.label.is_empty()
            || record.label.len() > MAX_LABEL
            || !(1..=MAX_VALUE).contains(&record.value_bytes)
            || match self.backend {
                SecretBackend::LocalFile => {
                    record.value.as_ref().map(String::len) != Some(record.value_bytes)
                }
                SecretBackend::System => record.value.is_some(),
            }
        {
            return Err(SecretStorageError::CorruptStorage);
        }
        Ok(Some(record))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn list(&self) -> Result<Vec<ManagedSecretEntry>, SecretStorageError> {
        let Some(directory) = self.directory(false)? else {
            return Ok(Vec::new());
        };
        let _lock = directory.lock()?;
        Ok(self
            .records(&directory)?
            .iter()
            .map(Record::metadata)
            .collect())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn create(
        &self,
        label: &str,
        value: &SecretValue,
    ) -> Result<ManagedSecretEntry, SecretStorageError> {
        validate_input(label, value)?;
        let directory = self
            .directory(true)?
            .ok_or(SecretStorageError::Unavailable)?;
        let _lock = directory.lock()?;
        directory.reclaim()?;
        let records = self.records(&directory)?;
        if records.len() >= MAX_ENTRIES
            || records.iter().map(|r| r.value_bytes).sum::<usize>() + value.expose_secret().len()
                > MAX_TOTAL
        {
            return Err(SecretStorageError::QuotaExceeded);
        }
        let record = Record {
            id: Uuid::new_v4(),
            label: label.to_owned(),
            revision: 1,
            value: (self.backend == SecretBackend::LocalFile)
                .then(|| value.expose_secret().to_owned()),
            value_bytes: value.expose_secret().len(),
        };
        // A collision is never treated as authorization to replace an old entry.
        if self.read_record(&directory, record.id)?.is_some() {
            return Err(SecretStorageError::RevisionConflict);
        }
        if self.backend == SecretBackend::System {
            self.system
                .store(&entry_path(record.id), value)
                .map_err(|_| SecretStorageError::Unavailable)?;
        }
        if let Err(error) = write_record(&directory, &record) {
            if self.backend == SecretBackend::System
                && error != SecretStorageError::DurabilityUnknown
                && self.system.delete(&entry_path(record.id)).is_err()
            {
                return Err(SecretStorageError::CleanupFailed);
            }
            return Err(error);
        }
        Ok(record.metadata())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn edit(
        &self,
        id: Uuid,
        revision: u64,
        label: Option<&str>,
        value: Option<&SecretValue>,
    ) -> Result<ManagedSecretEntry, SecretStorageError> {
        let directory = self.directory(false)?.ok_or(SecretStorageError::NotFound)?;
        let _lock = directory.lock()?;
        directory.reclaim()?;
        let records = self.records(&directory)?;
        let mut record = self
            .read_record(&directory, id)?
            .ok_or(SecretStorageError::NotFound)?;
        if record.revision != revision {
            return Err(SecretStorageError::RevisionConflict);
        }
        if let Some(label) = label {
            if label.is_empty() || label.len() > MAX_LABEL {
                return Err(SecretStorageError::InvalidInput);
            }
            record.label = label.to_owned();
        }
        if let Some(value) = value {
            validate_input(&record.label, value)?;
            if records.iter().map(|r| r.value_bytes).sum::<usize>() - record.value_bytes
                + value.expose_secret().len()
                > MAX_TOTAL
            {
                return Err(SecretStorageError::QuotaExceeded);
            }
            record.value_bytes = value.expose_secret().len();
            record.value = (self.backend == SecretBackend::LocalFile)
                .then(|| value.expose_secret().to_owned());
        }
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(SecretStorageError::InvalidInput)?;
        if let Some(value) = value.filter(|_| self.backend == SecretBackend::System) {
            self.system
                .store(&entry_path(id), value)
                .map_err(|_| SecretStorageError::Unavailable)?;
            // The keyring has committed independently. File failure cannot roll it back.
            write_record(&directory, &record).map_err(|_| SecretStorageError::DurabilityUnknown)?;
        } else {
            write_record(&directory, &record)?;
        }
        Ok(record.metadata())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn remove(&self, id: Uuid, revision: u64) -> Result<(), SecretStorageError> {
        let directory = self.directory(false)?.ok_or(SecretStorageError::NotFound)?;
        let _lock = directory.lock()?;
        directory.reclaim()?;
        let record = self
            .read_record(&directory, id)?
            .ok_or(SecretStorageError::NotFound)?;
        if record.revision != revision {
            return Err(SecretStorageError::RevisionConflict);
        }
        // Remove metadata first: even failed keyring cleanup cannot expose the deleted entry.
        directory.unlink(&id.simple().to_string())?;
        if self.backend == SecretBackend::System {
            self.system
                .delete(&entry_path(id))
                .map_err(|_| SecretStorageError::CleanupFailed)?;
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn load_entry(&self, id: Uuid) -> Result<Option<SecretValue>, SecretStorageError> {
        let Some(directory) = self.directory(false)? else {
            return Ok(None);
        };
        let Some(record) = self.read_record(&directory, id)? else {
            return Ok(None);
        };
        match self.backend {
            SecretBackend::LocalFile => Ok(record.value.map(SecretValue::new)),
            SecretBackend::System => self
                .system
                .load(&entry_path(id))
                .map_err(|_| SecretStorageError::Unavailable),
        }
    }
}

impl SecretVault for ManagedSecrets {
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        // Legacy direct paths remain system-only, including pre-existing names.
        if self.backend != SecretBackend::System {
            return Err(SecretAccessError::Unavailable);
        }
        self.system.store(path, value)
    }

    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        if self.backend == SecretBackend::System {
            self.system.load(path)
        } else {
            Err(SecretAccessError::Unavailable)
        }
    }

    fn load_bound(&self, entry: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        let id = entry_id(entry).ok_or(SecretAccessError::Unavailable)?;
        #[cfg(target_os = "linux")]
        return self
            .load_entry(id)
            .map_err(|_| SecretAccessError::Unavailable);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = id;
            return Err(SecretAccessError::Unavailable);
        }
    }

    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        if self.backend != SecretBackend::System {
            return Err(SecretAccessError::Unavailable);
        }
        self.system.delete(path)
    }
}

#[cfg(target_os = "linux")]
fn write_record(directory: &files::Directory, record: &Record) -> Result<(), SecretStorageError> {
    let bytes = serde_json::to_vec(record).map_err(|_| SecretStorageError::Unavailable)?;
    if bytes.len() > MAX_RECORD {
        return Err(SecretStorageError::InvalidInput);
    }
    directory.write(&record.id.simple().to_string(), &bytes)
}

#[cfg(target_os = "linux")]
fn validate_input(label: &str, value: &SecretValue) -> Result<(), SecretStorageError> {
    if label.is_empty()
        || label.len() > MAX_LABEL
        || !(1..=MAX_VALUE).contains(&value.expose_secret().len())
    {
        return Err(SecretStorageError::InvalidInput);
    }
    Ok(())
}

impl HostHome {
    /// Explicitly copies one legacy system path into a new managed system entry.
    /// Leaves the original value and its existing readers unchanged.
    #[cfg(target_os = "linux")]
    pub fn import_legacy_secret(
        &self,
        path: &SecretPath,
        label: &str,
    ) -> HostResult<ManagedSecretEntry> {
        Ok(ManagedSecrets::selected(self)?.import_legacy(path, label)?)
    }

    /// Reads the explicit home-local selection; absent configuration means system.
    pub fn secret_backend(&self) -> Result<SecretBackend, SecretStorageError> {
        #[cfg(target_os = "linux")]
        {
            let Some(directory) = files::Directory::credentials(self.root(), false)? else {
                return Ok(SecretBackend::System);
            };
            let Some(bytes) = directory.read("backend", 16)? else {
                return Ok(SecretBackend::System);
            };
            let text =
                std::str::from_utf8(&bytes).map_err(|_| SecretStorageError::CorruptStorage)?;
            text.parse().map_err(|_| SecretStorageError::CorruptStorage)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(SecretBackend::System)
        }
    }

    /// Explicitly selects a backend for future managers/startups. No migration or fallback.
    pub fn select_secret_backend(&self, backend: SecretBackend) -> Result<(), SecretStorageError> {
        #[cfg(target_os = "linux")]
        {
            let directory = files::Directory::credentials(self.root(), true)?
                .ok_or(SecretStorageError::Unavailable)?;
            let _lock = directory.lock()?;
            directory.reclaim()?;
            directory.write("backend", backend.as_str().as_bytes())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = backend;
            Err(SecretStorageError::Unavailable)
        }
    }

    /// Constructs the same explicitly selected backend used by native administration.
    pub fn secret_manager(&self) -> HostResult<SecretManager> {
        Ok(ManagedSecrets::selected(self)?.manager())
    }

    /// Lists metadata only for the trusted native manager, never for an extension.
    #[cfg(target_os = "linux")]
    pub fn list_shared_secrets(&self) -> HostResult<Vec<ManagedSecretEntry>> {
        Ok(ManagedSecrets::selected(self)?.list()?)
    }

    /// Creates a new shared entry without approving any consumer.
    #[cfg(target_os = "linux")]
    pub fn create_shared_secret(
        &self,
        label: &str,
        value: &SecretValue,
    ) -> HostResult<ManagedSecretEntry> {
        Ok(ManagedSecrets::selected(self)?.create(label, value)?)
    }

    /// Renames an entry without changing its identity, value or bindings.
    #[cfg(target_os = "linux")]
    pub fn rename_shared_secret(
        &self,
        id: Uuid,
        revision: u64,
        label: &str,
    ) -> HostResult<ManagedSecretEntry> {
        Ok(ManagedSecrets::selected(self)?.edit(id, revision, Some(label), None)?)
    }

    /// Replaces a value for all readers. Native UI must explain this before consent.
    #[cfg(target_os = "linux")]
    pub fn replace_shared_secret(
        &self,
        id: Uuid,
        revision: u64,
        value: &SecretValue,
    ) -> HostResult<ManagedSecretEntry> {
        Ok(ManagedSecrets::selected(self)?.edit(id, revision, None, Some(value))?)
    }

    /// Deletes the entry; existing consumer bindings remain visibly missing.
    #[cfg(target_os = "linux")]
    pub fn delete_shared_secret(&self, id: Uuid, revision: u64) -> HostResult<()> {
        Ok(ManagedSecrets::selected(self)?.remove(id, revision)?)
    }
}
