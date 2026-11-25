//! Persisted and live secret binding fences shared by native administration operations.
use crate::SecretReadGrant;
use crate::runtime::{LocalHostAccess, map_host_access_error};
use rintawa_extension_engine::{HostAccessError, LivePolicyTarget, SecretPromptAuthority};
use rintawa_sdk::{contracts::ComponentRef, secrets::SecretPath, types::RuntimeScopeId};
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

impl LocalHostAccess {
    pub(super) fn bind_secret_selection(
        &self,
        target: &LivePolicyTarget,
        slot: SecretPath,
        entry: Uuid,
        prompt: Option<&SecretPromptAuthority>,
        cancelled: Option<&AtomicBool>,
    ) -> Result<(), HostAccessError> {
        let home = self.home()?;
        let scope = RuntimeScopeId::new(target.scope_id.as_str());
        let _lock = home
            .lock_composition_scope(&scope)
            .map_err(map_host_access_error)?;
        let mut profile = home
            .load_composition_for_scope_locked(&scope)
            .map_err(map_host_access_error)?;
        if !profile.activations.iter().any(|a| {
            a.enabled
                && a.scope_id == scope
                && a.instance_id.as_str() == target.instance_id
                && a.artifact.to_string() == target.artifact
        }) {
            return Err(HostAccessError::Rejected);
        }
        if let Some(prompt) = prompt {
            let requester = prompt.requester_target();
            if !profile.runtime_permissions.iter().any(|grant| {
                grant.scope_id == scope
                    && grant.instance_id.as_str() == requester.instance_id
                    && grant.component_id.as_str() == requester.component_id
                    && grant.permission
                        == rintawa_sdk::runtime_permissions::RuntimePermission::SecretPrompt
            }) {
                return Err(HostAccessError::Rejected);
            }
        }
        if !self
            .secrets
            .as_ref()
            .ok_or(HostAccessError::Unavailable)?
            .list()
            .map_err(|_| HostAccessError::Unavailable)?
            .iter()
            .any(|e| e.id == entry)
        {
            return Err(HostAccessError::NotFound);
        }
        profile
            .bind_secret_read(SecretReadGrant {
                scope_id: scope.clone(),
                instance_id: target.instance_id.as_str().into(),
                component_id: target.component_id.as_str().into(),
                path: slot.clone(),
                entry_id: Some(entry),
            })
            .map_err(map_host_access_error)?;
        // A worker may wait for a busy policy fence, but never replay persistence.
        // Scope policy remains locked; cancellation is checked again by the commit callback.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            let mut attempted = false;
            let persist = || {
                attempted = true;
                if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                    return Err(HostAccessError::Rejected);
                }
                home.save_composition_for_scope(&scope, &profile)
                    .map_err(map_host_access_error)
            };
            let result = match prompt {
                Some(prompt) => prompt.bind(crate::shared_secrets::entry_path(entry), persist),
                None => self.policy_control()?.bind_secret(
                    target,
                    slot.clone(),
                    crate::shared_secrets::entry_path(entry),
                    persist,
                ),
            };
            if attempted
                || result != Err(HostAccessError::Unavailable)
                || std::time::Instant::now() >= deadline
            {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    pub(super) fn disconnect_secret_consumer(
        &self,
        expected: &SecretReadGrant,
        cancelled: &AtomicBool,
    ) -> Result<(), HostAccessError> {
        let home = self.home()?;
        let scope = &expected.scope_id;
        let owner = ComponentRef::new(expected.instance_id.clone(), expected.component_id.clone());
        let control = self.policy_control()?;
        let live = match control.snapshot(
            scope.as_str(),
            owner.instance_id.as_str(),
            owner.component_id.as_str(),
        ) {
            Ok(snapshot) => Some(snapshot.target),
            Err(HostAccessError::Rejected) => None,
            Err(_) => return Err(HostAccessError::Unavailable),
        };
        // An inactive World cannot activate in this or another Host during the edit.
        let _lifecycle = if live.is_none() {
            if let crate::PersistedCompositionScope::World(id) = home
                .persisted_composition_scope(scope)
                .map_err(|_| HostAccessError::Rejected)?
            {
                let owned = self
                    .world_sessions
                    .state
                    .lock()
                    .map_err(|_| HostAccessError::Unavailable)?
                    .active
                    .contains(&id);
                if owned {
                    None
                } else {
                    Some(
                        home.lock_world_lifecycle(id)
                            .map_err(|_| HostAccessError::Rejected)?,
                    )
                }
            } else {
                None
            }
        } else {
            None
        };
        let _scope = home
            .lock_composition_scope(scope)
            .map_err(|_| HostAccessError::Unavailable)?;
        let mut profile = home
            .load_composition_for_scope_locked(scope)
            .map_err(|_| HostAccessError::Unavailable)?;
        if !profile.secret_read_grants.contains(expected) {
            return Err(HostAccessError::Rejected);
        }
        profile.revoke_secret_read(scope, &owner, &expected.path);
        let persist = || {
            if cancelled.load(Ordering::Acquire) {
                return Err(HostAccessError::Rejected);
            }
            home.save_composition_for_scope(scope, &profile)
                .map_err(map_host_access_error)
        };
        control.disconnect_secret(
            scope.as_str(),
            &owner,
            live.as_ref(),
            &expected.path,
            persist,
        )
    }
}
