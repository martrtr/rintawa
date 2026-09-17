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
        if let Ok(mut grants) = self.grants.write() {
            grants.remove(owner);
        }
    }
}
