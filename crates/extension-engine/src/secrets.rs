//! Host-owned secret storage and component-scoped access policy.
//!
//! Secrets are stored by their global [`SecretPath`], not under an extension
//! directory. An extension receives access only through an active component
//! context after Rintawa has granted a manifest-requested path pattern.

use std::{
    collections::HashMap,
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
pub trait SecretVault: Send + Sync {
    /// Stores a secret at the supplied path.
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError>;

    /// Loads one secret, returning `None` when it has not been configured.
    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError>;

    /// Loads a binding's storage key, distinct from a legacy direct path.
    /// Simple vaults may resolve both identically; managed vaults check local metadata.
    fn load_bound(&self, entry: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        self.load(entry)
    }

    /// Deletes one secret, returning whether it existed.
    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError>;
}

/// A platform credential-store implementation for production Rintawa hosts.
#[derive(Debug, Default)]
pub struct SystemSecretVault;

impl SystemSecretVault {
    fn entry(path: &SecretPath) -> Result<Entry, SecretAccessError> {
        Entry::new(KEYRING_SERVICE_NAME, path.as_str()).map_err(|_| Self::unavailable("open"))
    }

    fn unavailable(operation: &str) -> SecretAccessError {
        warn!(operation, "OS credential store operation failed");
        SecretAccessError::Unavailable
    }
}

impl SecretVault for SystemSecretVault {
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        Self::entry(path)?
            .set_password(value.expose_secret())
            .map_err(|_| Self::unavailable("store"))
    }

    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        match Self::entry(path)?.get_password() {
            Ok(value) => Ok(Some(SecretValue::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Self::unavailable("load")),
        }
    }

    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        match Self::entry(path)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => Err(Self::unavailable("delete")),
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
#[derive(Clone, Default)]
pub(crate) struct SecretGrants {
    pub(crate) patterns: Vec<SecretPathPattern>,
    pub(crate) bindings: HashMap<SecretPath, SecretPath>,
    pub(crate) revision: u64,
}

/// Shared system vault and owner-scoped live read policy.
#[derive(Clone)]
pub struct SecretManager {
    vault: Arc<dyn SecretVault>,
    pub(crate) read_grants: Arc<RwLock<HashMap<ComponentRef, SecretGrants>>>,
    pub(crate) policy_revision: Arc<AtomicU64>,
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
    pub fn with_vault(vault: Arc<dyn SecretVault>) -> Self {
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
        if !principal_grants.patterns.contains(&pattern) {
            principal_grants.patterns.push(pattern);
            principal_grants.revision = self.policy_revision.fetch_add(1, Ordering::Relaxed) + 1;
        }
        Ok(())
    }

    /// Binds an exact approved consumer slot to a vault entry.
    /// The caller must first validate the slot against the component manifest.
    pub fn bind_read(
        &self,
        owner: ComponentRef,
        slot: SecretPath,
        entry: SecretPath,
    ) -> Result<(), SecretAccessError> {
        let mut grants = self
            .read_grants
            .write()
            .map_err(|_| SecretAccessError::Unavailable)?;
        let principal = grants.entry(owner).or_default();
        let exact = SecretPathPattern::Exact(slot.clone());
        if !principal.patterns.contains(&exact) {
            principal.patterns.push(exact);
        }
        if principal.bindings.get(&slot) != Some(&entry) {
            principal.bindings.insert(slot, entry);
            principal.revision = self.policy_revision.fetch_add(1, Ordering::Relaxed) + 1;
        }
        Ok(())
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
                        .patterns
                        .iter()
                        .any(|granted| granted.allows_pattern(pattern))
                })
            })
            .unwrap_or(false)
    }

    /// Revokes every secret grant held by one component.
    pub fn revoke_component(&self, owner: &ComponentRef) {
        let mut grants = match self.read_grants.write() {
            Ok(grants) => grants,
            Err(poisoned) => poisoned.into_inner(),
        };
        if grants.remove(owner).is_some() {
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
        let granted = grants
            .get(&principal)
            .filter(|g| g.patterns.iter().any(|p| p.matches(path)))
            .ok_or(SecretAccessError::AccessDenied)?;
        let revision = granted.revision;
        let binding = granted.bindings.get(path).cloned();
        drop(grants);
        let value = match binding {
            Some(entry) => self.vault.load_bound(&entry)?,
            None => self.vault.load(path)?,
        };
        let grants = self
            .read_grants
            .read()
            .map_err(|_| SecretAccessError::Unavailable)?;
        if !grants
            .get(&principal)
            .is_some_and(|g| g.revision == revision && g.patterns.iter().any(|p| p.matches(path)))
        {
            return Err(SecretAccessError::AccessDenied);
        }
        value.ok_or(SecretAccessError::NotFound)
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
    fn test_shared_binding_selects_entry_without_granting_other_consumers() {
        let manager = manager();
        let slot = SecretPath::parse("ai.provider.key").unwrap();
        let entry = SecretPath::parse("managed.entry_1").unwrap();
        let second = SecretPath::parse("managed.entry_2").unwrap();
        let first_owner = owner("first", "backend");
        let second_owner = owner("second", "backend");
        manager
            .store(&entry, &SecretValue::new("first value"))
            .unwrap();
        manager
            .store(&second, &SecretValue::new("second value"))
            .unwrap();
        manager
            .bind_read(first_owner.clone(), slot.clone(), entry.clone())
            .unwrap();
        assert_eq!(
            manager
                .read_for_component(&first_owner, &slot)
                .unwrap()
                .expose_secret(),
            "first value"
        );
        assert!(matches!(
            manager.read_for_component(&second_owner, &slot),
            Err(SecretAccessError::AccessDenied)
        ));
        manager
            .bind_read(second_owner.clone(), slot.clone(), entry)
            .unwrap();
        manager
            .bind_read(first_owner.clone(), slot.clone(), second)
            .unwrap();
        assert_eq!(
            manager
                .read_for_component(&first_owner, &slot)
                .unwrap()
                .expose_secret(),
            "second value"
        );
        assert_eq!(
            manager
                .read_for_component(&second_owner, &slot)
                .unwrap()
                .expose_secret(),
            "first value"
        );
        manager.revoke_component(&first_owner);
        assert!(matches!(
            manager.read_for_component(&first_owner, &slot),
            Err(SecretAccessError::AccessDenied)
        ));
    }

    #[test]
    fn test_should_revoke_secret_grant_after_policy_lock_is_poisoned() {
        let manager = manager();
        let owner = owner("official_ai.instance-a", "provider");
        manager
            .grant_read(
                owner.clone(),
                SecretPathPattern::parse("ai.api_keys.*").unwrap(),
            )
            .unwrap();

        let poisoner = manager.clone();
        let _ = std::thread::spawn(move || {
            let _grants = poisoner
                .read_grants
                .write()
                .expect("test secret policy lock should start healthy");
            panic!("poison secret policy lock");
        })
        .join();

        manager.revoke_component(&owner);
        let grants = match manager.read_grants.read() {
            Ok(grants) => grants,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!grants.contains_key(&owner));
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
