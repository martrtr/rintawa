//! Host-owned secret storage and component-scoped access policy.
//!
//! Secrets are stored by their global [`SecretPath`], not under an extension
//! directory. An extension receives access only through an active component
//! context after Rintawa has granted a manifest-requested path pattern.

use std::{
    collections::HashMap,
    fmt::Display,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use keyring::Entry;
use rintawa_sdk::{
    contracts::ComponentRef,
    secrets::{SecretAccessError, SecretPath, SecretPathPattern, SecretValue},
};
use tracing::warn;

/// The credential-store service name reserved for Rintawa secrets.
pub const KEYRING_SERVICE_NAME: &str = "rintawa";

/// A trusted host interface for durable secret storage.
///
/// Extensions never receive this trait. They receive only a component context
/// that performs an owner-scoped read check.
pub(crate) trait SecretVault: Send + Sync {
    /// Stores a secret at the supplied path.
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError>;

    /// Loads one secret, returning `None` when it has not been configured.
    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError>;

    /// Deletes one secret, returning whether it existed.
    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError>;
}

/// A platform credential-store implementation for production Rintawa hosts.
#[derive(Debug, Default)]
pub(crate) struct SystemSecretVault;

impl SystemSecretVault {
    fn entry(path: &SecretPath) -> Result<Entry, SecretAccessError> {
        Entry::new(KEYRING_SERVICE_NAME, path.as_str())
            .map_err(|error| Self::unavailable("open", path, error))
    }

    fn unavailable(operation: &str, path: &SecretPath, error: impl Display) -> SecretAccessError {
        warn!(
            operation,
            path = %path,
            error = %error,
            "OS credential store operation failed"
        );
        SecretAccessError::Unavailable
    }
}

impl SecretVault for SystemSecretVault {
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        Self::entry(path)?
            .set_password(value.expose_secret())
            .map_err(|error| Self::unavailable("store", path, error))
    }

    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        match Self::entry(path)?.get_password() {
            Ok(value) => Ok(Some(SecretValue::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Self::unavailable("load", path, error)),
        }
    }

    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        match Self::entry(path)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(Self::unavailable("delete", path, error)),
        }
    }
}

/// An in-memory vault intended only for tests and explicitly ephemeral hosts.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct InMemorySecretVault {
    secrets: RwLock<HashMap<SecretPath, SecretValue>>,
}

#[cfg(test)]
impl SecretVault for InMemorySecretVault {
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        let mut secrets = self
            .secrets
            .write()
            .map_err(|_| SecretAccessError::Unavailable)?;
        secrets.insert(path.clone(), SecretValue::new(value.expose_secret()));
        Ok(())
    }

    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        let secrets = self
            .secrets
            .read()
            .map_err(|_| SecretAccessError::Unavailable)?;
        Ok(secrets
            .get(path)
            .map(|value| SecretValue::new(value.expose_secret())))
    }

    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        let mut secrets = self
            .secrets
            .write()
            .map_err(|_| SecretAccessError::Unavailable)?;
        Ok(secrets.remove(path).is_some())
    }
}

/// Rintawa's trusted policy and storage facade for secrets.
///
/// Clones share both the policy and vault. This lets a WASM runtime retain a
/// host handle while policy changes immediately affect future reads.
#[derive(Clone)]
pub struct SecretManager {
    vault: Arc<dyn SecretVault>,
    read_grants: Arc<RwLock<HashMap<ComponentRef, Vec<SecretPathPattern>>>>,
    policy_revision: Arc<AtomicU64>,
}

impl Default for SecretManager {
    fn default() -> Self {
        Self::system()
    }
}

impl SecretManager {
    /// Creates a manager backed by the user's platform credential store.
    pub fn system() -> Self {
        Self::with_vault(Arc::new(SystemSecretVault))
    }

    /// Creates a manager with a host-supplied durable or test vault.
    pub(crate) fn with_vault(vault: Arc<dyn SecretVault>) -> Self {
        Self {
            vault,
            read_grants: Arc::new(RwLock::new(HashMap::new())),
            policy_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Stores a secret through the host's credential vault.
    ///
    /// This is a host configuration operation and must never be exposed to an
    /// extension context or WIT import.
    pub fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        self.vault.store(path, value)
    }

    /// Deletes a secret through the host's credential vault.
    ///
    /// This is a host configuration operation and must never be exposed to an
    /// extension context or WIT import.
    pub fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        self.vault.delete(path)
    }

    /// Grants one component read access to a requested exact path or domain.
    pub fn grant_read(
        &self,
        owner: ComponentRef,
        pattern: SecretPathPattern,
    ) -> Result<(), SecretAccessError> {
        let principal = owner;
        let mut grants = self
            .read_grants
            .write()
            .map_err(|_| SecretAccessError::Unavailable)?;
        let principal_grants = grants.entry(principal).or_default();
        if !principal_grants.contains(&pattern) {
            principal_grants.push(pattern);
            self.policy_revision.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    pub(crate) fn policy_revision(&self) -> u64 {
        self.policy_revision.load(Ordering::Relaxed)
    }

    /// Returns whether one component currently holds a grant covering `pattern`.
    pub(crate) fn has_read_grant(&self, owner: &ComponentRef, pattern: &SecretPathPattern) -> bool {
        let principal = owner.clone();
        self.read_grants
            .read()
            .ok()
            .and_then(|grants| {
                grants.get(&principal).map(|patterns| {
                    patterns
                        .iter()
                        .any(|granted| granted.allows_pattern(pattern))
                })
            })
            .unwrap_or(false)
    }

    /// Revokes every secret grant held by one component.
    pub fn revoke_component(&self, owner: &ComponentRef) {
        if let Ok(mut grants) = self.read_grants.write()
            && grants.remove(owner).is_some()
        {
            self.policy_revision.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn read_for_component(
        &self,
        owner: &ComponentRef,
        path: &SecretPath,
    ) -> Result<SecretValue, SecretAccessError> {
        let principal = owner.clone();
        let grants = self
            .read_grants
            .read()
            .map_err(|_| SecretAccessError::Unavailable)?;
        let allowed = grants
            .get(&principal)
            .is_some_and(|patterns| patterns.iter().any(|pattern| pattern.matches(path)));
        drop(grants);

        if !allowed {
            return Err(SecretAccessError::AccessDenied);
        }

        self.vault.load(path)?.ok_or(SecretAccessError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::types::ExtensionInstanceId;

    fn manager() -> SecretManager {
        SecretManager::with_vault(Arc::new(InMemorySecretVault::default()))
    }

    fn owner(instance: &str, component: &str) -> ComponentRef {
        ComponentRef::new(ExtensionInstanceId::new(instance), component)
    }

    #[test]
    fn test_should_allow_only_granted_component_and_domain() {
        let manager = manager();
        let allowed_path = SecretPath::parse("ai.api_keys.openai").unwrap();
        let denied_path = SecretPath::parse("ai.provider_tokens.openai").unwrap();
        let allowed_owner = owner("official_ai.instance-a", "provider");

        manager
            .store(&allowed_path, &SecretValue::new("test-key"))
            .unwrap();
        manager
            .grant_read(
                allowed_owner.clone(),
                SecretPathPattern::parse("ai.api_keys.*").unwrap(),
            )
            .unwrap();

        let value = manager
            .read_for_component(&allowed_owner, &allowed_path)
            .unwrap();
        assert_eq!(value.expose_secret(), "test-key");
        assert!(matches!(
            manager.read_for_component(&allowed_owner, &denied_path),
            Err(SecretAccessError::AccessDenied)
        ));
        assert!(matches!(
            manager.read_for_component(&owner("official_ai.instance-a", "other"), &allowed_path),
            Err(SecretAccessError::AccessDenied)
        ));
        assert!(matches!(
            manager.read_for_component(&owner("official_ai.instance-b", "provider"), &allowed_path),
            Err(SecretAccessError::AccessDenied)
        ));
    }

    #[test]
    fn test_should_revoke_component_secret_access() {
        let manager = manager();
        let path = SecretPath::parse("ai.api_keys.openai").unwrap();
        let owner = owner("official_ai.instance-a", "provider");

        manager.store(&path, &SecretValue::new("test-key")).unwrap();
        manager
            .grant_read(
                owner.clone(),
                SecretPathPattern::parse("ai.api_keys.*").unwrap(),
            )
            .unwrap();
        manager.revoke_component(&owner);

        assert!(matches!(
            manager.read_for_component(&owner, &path),
            Err(SecretAccessError::AccessDenied)
        ));
    }
}
