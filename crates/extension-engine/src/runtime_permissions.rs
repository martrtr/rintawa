//! Host-owned policy for coarse component runtime capabilities.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use rintawa_sdk::{contracts::ComponentRef, runtime_permissions::RuntimePermission};

#[derive(Clone, Default)]
pub(crate) struct RuntimePermissionManager {
    grants: Arc<RwLock<HashMap<ComponentRef, HashSet<RuntimePermission>>>>,
}

impl RuntimePermissionManager {
    pub(crate) fn grant(
        &self,
        owner: ComponentRef,
        permission: RuntimePermission,
    ) -> Result<(), ()> {
        let mut grants = self.grants.write().map_err(|_| ())?;
        grants.entry(owner).or_default().insert(permission);
        Ok(())
    }

    pub(crate) fn has_grant(
        &self,
        owner: &ComponentRef,
        permission: RuntimePermission,
    ) -> Result<bool, ()> {
        let grants = self.grants.read().map_err(|_| ())?;
        Ok(grants
            .get(owner)
            .is_some_and(|permissions| permissions.contains(&permission)))
    }

    pub(crate) fn revoke_component(&self, owner: &ComponentRef) {
        let mut grants = match self.grants.write() {
            Ok(grants) => grants,
            Err(poisoned) => poisoned.into_inner(),
        };
        grants.remove(owner);
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use rintawa_sdk::types::{ComponentId, ExtensionInstanceId};

    use super::*;

    #[test]
    fn test_should_revoke_permission_after_policy_lock_is_poisoned() {
        let manager = RuntimePermissionManager::default();
        let owner = ComponentRef::new(
            ExtensionInstanceId::new("example.runtime"),
            ComponentId::new("runtime"),
        );
        manager
            .grant(owner.clone(), RuntimePermission::BackgroundTask)
            .expect("test grant should succeed");

        let poisoner = manager.clone();
        let _ = thread::spawn(move || {
            let _grants = poisoner
                .grants
                .write()
                .expect("test lock should start healthy");
            panic!("poison runtime permission policy lock");
        })
        .join();

        manager.revoke_component(&owner);
        let grants = match manager.grants.read() {
            Ok(grants) => grants,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!grants.contains_key(&owner));
    }
}
