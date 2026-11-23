//! Native administration of one attested live activation without Engine reentry.
use crate::{
    execution_context::ExecutionContexts,
    host_access::{HostAccessError, HostAccessResult},
    runtime_permissions::RuntimePermissionManager,
    secrets::{SecretGrants, SecretManager},
};
use rintawa_sdk::{
    contracts::ComponentRef,
    runtime_permissions::RuntimePermission,
    secrets::{SecretPath, SecretPathPattern},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::atomic::Ordering};

/// Host-attested identity captured before showing an administrative decision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LivePolicyTarget {
    /// Exact execution scope.
    pub scope_id: String,
    /// Concrete persisted instance.
    pub instance_id: String,
    /// Component within that instance.
    pub component_id: String,
    /// Verified selected RTW digest.
    pub artifact: String,
    /// Ephemeral activation, represented losslessly in browsers.
    pub activation: String,
}
/// Current requested and effective rights; contains no secret values.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePolicySnapshot {
    /// Captured live target.
    pub target: LivePolicyTarget,
    /// Manifest ceiling for runtime capabilities.
    pub requested: Vec<RuntimePermission>,
    /// Manifest ceiling for credential reads.
    pub requested_secret_read: Vec<String>,
    /// Currently effective runtime capabilities.
    pub granted: Vec<RuntimePermission>,
    /// Currently approved exact local origins.
    pub http_origins: Vec<String>,
    /// Currently effective credential paths/patterns.
    pub secret_read: Vec<String>,
}
/// One bounded consent decision. Every field is validated before persistence.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct LivePolicyDelta {
    /// Capabilities to grant.
    pub grant_runtime: Vec<RuntimePermission>,
    /// Capabilities to revoke.
    pub revoke_runtime: Vec<RuntimePermission>,
    /// Local origins to approve.
    pub allow_http_origins: Vec<String>,
    /// Local origins to revoke.
    pub deny_http_origins: Vec<String>,
    /// Exact credential reads to approve.
    pub allow_secret_read: Vec<SecretPath>,
    /// Exact credential reads to revoke.
    pub deny_secret_read: Vec<SecretPath>,
}
/// A decision bound to the activation originally inspected by the administrator.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LivePolicyChange {
    /// Exact target, never rebound to the current instance on a mismatch.
    pub target: LivePolicyTarget,
    /// Independent capability changes grouped into one decision.
    pub delta: LivePolicyDelta,
}
/// Shares only existing policy and live identities; no Engine mutex or guest code.
#[derive(Clone)]
pub struct LivePolicyControl {
    contexts: ExecutionContexts,
    runtime: RuntimePermissionManager,
    secrets: SecretManager,
}
impl LivePolicyControl {
    pub(crate) fn new(
        contexts: ExecutionContexts,
        runtime: RuntimePermissionManager,
        secrets: SecretManager,
    ) -> Self {
        Self {
            contexts,
            runtime,
            secrets,
        }
    }

    /// Inspects one active verified component in an exact scope.
    pub fn snapshot(
        &self,
        scope: &str,
        instance: &str,
        component: &str,
    ) -> HostAccessResult<LivePolicySnapshot> {
        let _lifecycle = self
            .contexts
            .1
            .try_lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let owner = ComponentRef::new(instance, component);
        let (context, verified) = self
            .contexts
            .policy_identity(&owner)
            .map_err(|_| HostAccessError::Rejected)?;
        if context.scope.as_str() != scope {
            return Err(HostAccessError::Rejected);
        }
        let grants = self
            .runtime
            .grants
            .try_read()
            .map_err(|_| HostAccessError::Unavailable)?;
        let origins = self
            .runtime
            .http_loopback_origins
            .try_read()
            .map_err(|_| HostAccessError::Unavailable)?;
        let secrets = self
            .secrets
            .read_grants
            .try_read()
            .map_err(|_| HostAccessError::Unavailable)?;
        let mut granted = grants
            .get(&owner)
            .map(|g| g.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        granted.sort_by_key(|g| g.to_string());
        let mut http_origins = origins
            .get(&owner)
            .map(|g| g.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        http_origins.sort();
        Ok(LivePolicySnapshot {
            target: LivePolicyTarget {
                scope_id: scope.into(),
                instance_id: instance.into(),
                component_id: component.into(),
                artifact: verified.digest.to_string(),
                activation: context.principal.activation.to_string(),
            },
            requested: verified.permissions.runtime,
            requested_secret_read: verified
                .permissions
                .secret_read
                .iter()
                .map(ToString::to_string)
                .collect(),
            granted,
            http_origins,
            secret_read: secrets
                .get(&owner)
                .map(|g| g.patterns.iter().map(ToString::to_string).collect())
                .unwrap_or_default(),
        })
    }

    /// Validates, durably saves, then publishes under a lifecycle fence.
    ///
    /// The Host holds its existing scope file lock around this call. No live state
    /// changes if validation, lock acquisition or persistence fails. After the
    /// persistence callback succeeds, publication needs no further fallible locks.
    pub fn apply(
        &self,
        change: &LivePolicyChange,
        persist: impl FnOnce() -> HostAccessResult<()>,
    ) -> HostAccessResult<()> {
        let _lifecycle = self
            .contexts
            .1
            .try_lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let t = &change.target;
        let d = &change.delta;
        let owner = ComponentRef::new(t.instance_id.as_str(), t.component_id.as_str());
        let (context, verified) = self
            .contexts
            .policy_identity(&owner)
            .map_err(|_| HostAccessError::Rejected)?;
        if context.scope.as_str() != t.scope_id
            || context.principal.activation.to_string() != t.activation
            || verified.digest.to_string() != t.artifact
        {
            return Err(HostAccessError::Rejected);
        }
        let count = d.grant_runtime.len()
            + d.revoke_runtime.len()
            + d.allow_http_origins.len()
            + d.deny_http_origins.len()
            + d.allow_secret_read.len()
            + d.deny_secret_read.len();
        if count > 64
            || duplicate_or_conflict(&d.grant_runtime, &d.revoke_runtime)
            || duplicate_or_conflict(&d.allow_http_origins, &d.deny_http_origins)
            || duplicate_or_conflict(&d.allow_secret_read, &d.deny_secret_read)
        {
            return Err(HostAccessError::Rejected);
        }
        if d.grant_runtime
            .iter()
            .any(|g| !verified.permissions.runtime.contains(g))
            || d.allow_secret_read.iter().any(|p| {
                !verified
                    .permissions
                    .secret_read
                    .iter()
                    .any(|g| g.matches(p))
            })
        {
            return Err(HostAccessError::Rejected);
        }
        for origin in d.allow_http_origins.iter().chain(&d.deny_http_origins) {
            if crate::canonical_http_loopback_origin(origin).as_deref() != Some(origin) {
                return Err(HostAccessError::Rejected);
            }
        }
        let mut grants = self
            .runtime
            .grants
            .try_write()
            .map_err(|_| HostAccessError::Unavailable)?;
        let mut origins = self
            .runtime
            .http_loopback_origins
            .try_write()
            .map_err(|_| HostAccessError::Unavailable)?;
        let mut secrets = self
            .secrets
            .read_grants
            .try_write()
            .map_err(|_| HostAccessError::Unavailable)?;
        let mut next_grants = grants.get(&owner).cloned().unwrap_or_default();
        for g in &d.grant_runtime {
            next_grants.insert(*g);
        }
        for g in &d.revoke_runtime {
            next_grants.remove(g);
        }
        let mut next_origins = origins.get(&owner).cloned().unwrap_or_default();
        if !d.allow_http_origins.is_empty()
            && !next_grants.contains(&RuntimePermission::HttpRequest)
        {
            return Err(HostAccessError::Rejected);
        }
        for origin in &d.allow_http_origins {
            next_origins.insert(origin.clone());
        }
        for origin in &d.deny_http_origins {
            next_origins.remove(origin);
        }
        if !next_grants.contains(&RuntimePermission::HttpRequest) {
            next_origins.clear();
        }
        let mut next_secrets = secrets
            .get(&owner)
            .map(|g| g.patterns.clone())
            .unwrap_or_default();
        for p in &d.allow_secret_read {
            let g = SecretPathPattern::Exact(p.clone());
            if !next_secrets.contains(&g) {
                next_secrets.push(g);
            }
        }
        for p in &d.deny_secret_read {
            next_secrets.retain(|g| g != &SecretPathPattern::Exact(p.clone()));
        }
        if d.deny_secret_read
            .iter()
            .any(|p| next_secrets.iter().any(|g| g.matches(p)))
        {
            return Err(HostAccessError::Rejected);
        }
        persist()?;
        grants.insert(owner.clone(), next_grants);
        origins.insert(owner.clone(), next_origins);
        if secrets
            .get(&owner)
            .map(|g| g.patterns.as_slice())
            .unwrap_or(&[])
            != next_secrets.as_slice()
        {
            let revision = self.secrets.policy_revision.fetch_add(1, Ordering::Relaxed) + 1;
            secrets.insert(
                owner.clone(),
                SecretGrants {
                    patterns: next_secrets,
                    revision,
                },
            );
        }
        drop(secrets);
        drop(origins);
        for permission in &d.revoke_runtime {
            self.runtime
                .cancel_resources(&owner, Some(*permission), None);
        }
        for origin in &d.deny_http_origins {
            self.runtime.cancel_http(&owner, Some(origin));
        }
        if d.revoke_runtime
            .contains(&RuntimePermission::UserResourceRead)
            && let Some(cleanup) = self.contexts.detach_resource_access(&owner)
        {
            cleanup.dispose();
        }
        drop(grants);
        Ok(())
    }
}
fn duplicate_or_conflict<T: Eq + std::hash::Hash>(a: &[T], b: &[T]) -> bool {
    let mut seen = HashSet::new();
    a.iter().chain(b).any(|v| !seen.insert(v))
}

#[cfg(test)]
mod tests;
