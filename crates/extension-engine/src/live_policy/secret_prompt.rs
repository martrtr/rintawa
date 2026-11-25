//! Opaque native picker authority tied to a caller, sibling and permission lease.

use crate::{
    execution_context::ExecutionContext,
    host_access::{HostAccessError, HostAccessResult},
    live_policy::{LivePolicyControl, LivePolicyTarget},
    runtime_permissions::RuntimeCancellation,
    transport::TransportError,
};
use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, secrets::SecretPath,
    types::ComponentId,
};
use std::sync::{Arc, atomic::Ordering};

#[cfg(test)]
mod tests;

/// Native-only proof of an admitted picker. Never serialize it into a bridge frame.
#[derive(Clone)]
pub struct SecretPromptAuthority {
    control: LivePolicyControl,
    requester: ExecutionContext,
    artifact: String,
    target: LivePolicyTarget,
    slot: SecretPath,
    permission: Arc<RuntimeCancellation>,
}

impl SecretPromptAuthority {
    /// Verified live sibling selected from the same artifact and instance.
    pub fn target(&self) -> &LivePolicyTarget {
        &self.target
    }

    /// Exact logical slot covered by the sibling's manifest.
    pub fn slot(&self) -> &SecretPath {
        &self.slot
    }

    pub(super) fn requester(&self) -> &ExecutionContext {
        &self.requester
    }

    /// Captured caller identity for the native Host's persisted-grant revalidation.
    pub fn requester_target(&self) -> LivePolicyTarget {
        LivePolicyTarget {
            scope_id: self.requester.scope.to_string(),
            instance_id: self.requester.principal.instance_id.to_string(),
            component_id: self.requester.principal.component_id.to_string(),
            artifact: self.artifact.clone(),
            activation: self.requester.principal.activation.to_string(),
        }
    }

    /// Rejects stale activation or revoked/regranted permission; contention is unavailable.
    pub fn validate(&self) -> HostAccessResult<()> {
        let _lifecycle = self
            .control
            .contexts
            .1
            .try_lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        self.validate_locked()?;
        if !self
            .control
            .runtime
            .has_grant(
                &self.requester.principal.component(),
                RuntimePermission::SecretPrompt,
            )
            .map_err(|_| HostAccessError::Unavailable)?
        {
            return Err(HostAccessError::Rejected);
        }
        Ok(())
    }

    pub(super) fn validate_locked(&self) -> HostAccessResult<()> {
        if self.permission.cancelled.load(Ordering::Acquire)
            || self.permission.revoked.load(Ordering::Acquire)
        {
            return Err(HostAccessError::Rejected);
        }
        let (caller, artifact) = self
            .control
            .contexts
            .policy_identity(&self.requester.principal.component())
            .map_err(identity_error)?;
        if caller != self.requester || artifact.digest.to_string() != self.artifact {
            return Err(HostAccessError::Rejected);
        }
        let (target, verified) = self
            .control
            .contexts
            .policy_identity(&ComponentRef::new(
                self.target.instance_id.as_str(),
                self.target.component_id.as_str(),
            ))
            .map_err(identity_error)?;
        if target.scope.as_str() != self.target.scope_id
            || target.principal.activation.to_string() != self.target.activation
            || verified.digest.to_string() != self.target.artifact
        {
            return Err(HostAccessError::Rejected);
        }
        Ok(())
    }

    /// Publishes an exact grant only while both captured activations and prompt grant survive.
    pub fn bind(
        &self,
        entry: SecretPath,
        persist: impl FnOnce() -> HostAccessResult<()>,
    ) -> HostAccessResult<()> {
        self.control.bind_secret_checked(
            Some(self),
            &self.target,
            self.slot.clone(),
            entry,
            persist,
        )
    }
}

impl LivePolicyControl {
    /// Admits only a manifest-covered live sibling; the native caller supplies its attested target.
    /// No scope, foreign instance or physical entry can be chosen by an extension intent.
    pub fn admit_secret_prompt(
        &self,
        requester: &LivePolicyTarget,
        sibling: &ComponentId,
        slot: SecretPath,
    ) -> HostAccessResult<SecretPromptAuthority> {
        let _lifecycle = self
            .contexts
            .1
            .try_lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let owner = ComponentRef::new(
            requester.instance_id.as_str(),
            requester.component_id.as_str(),
        );
        let (caller, verified) = self
            .contexts
            .policy_identity(&owner)
            .map_err(identity_error)?;
        if caller.scope.as_str() != requester.scope_id
            || caller.principal.activation.to_string() != requester.activation
            || verified.digest.to_string() != requester.artifact
            || !verified
                .permissions
                .runtime
                .contains(&RuntimePermission::SecretPrompt)
        {
            return Err(HostAccessError::Rejected);
        }
        let target_owner = ComponentRef::new(owner.instance_id.clone(), sibling.clone());
        let (target, target_verified) = self
            .contexts
            .policy_identity(&target_owner)
            .map_err(identity_error)?;
        if target.scope != caller.scope
            || target_verified.digest != verified.digest
            || !target_verified
                .permissions
                .secret_read
                .iter()
                .any(|p| p.matches(&slot))
        {
            return Err(HostAccessError::Rejected);
        }
        let permission = self
            .runtime
            .register_resource(
                &owner,
                caller.principal.activation,
                RuntimePermission::SecretPrompt,
                None,
            )
            .map_err(|_| HostAccessError::Rejected)?;
        Ok(SecretPromptAuthority {
            control: self.clone(),
            requester: caller,
            artifact: verified.digest.to_string(),
            target: LivePolicyTarget {
                scope_id: target.scope.to_string(),
                instance_id: target_owner.instance_id.to_string(),
                component_id: sibling.to_string(),
                artifact: target_verified.digest.to_string(),
                activation: target.principal.activation.to_string(),
            },
            slot,
            permission,
        })
    }
}

fn identity_error(error: TransportError) -> HostAccessError {
    match error {
        TransportError::Inactive => HostAccessError::Rejected,
        _ => HostAccessError::Unavailable,
    }
}
