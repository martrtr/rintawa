//! Host-owned policy for coarse component runtime capabilities.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use rintawa_sdk::{contracts::ComponentRef, runtime_permissions::RuntimePermission};
use url::{Host, Url};

/// Host approval for an exact origin on the loopback HTTP transport.
///
/// This is intentionally separate from the broad HTTPS request capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpOriginGrantError {
    InvalidOrigin,
    PermissionDenied,
    Unavailable,
}

pub(crate) fn is_local_http_origin(url: &Url) -> bool {
    if url.scheme() != "http" || url.port_or_known_default().is_none() {
        return false;
    }
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

pub(crate) fn parse_exact_local_http_origin(value: &str) -> Result<String, HttpOriginGrantError> {
    let url = Url::parse(value).map_err(|_| HttpOriginGrantError::InvalidOrigin)?;
    if !is_local_http_origin(&url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port() == Some(0)
    {
        return Err(HttpOriginGrantError::InvalidOrigin);
    }
    // Do not accept spellings that could hide paths, whitespace or URL syntax.
    let normalized = url.origin().ascii_serialization();
    let normalized_input = value.to_ascii_lowercase();
    if normalized_input != normalized && normalized_input != format!("{normalized}/") {
        return Err(HttpOriginGrantError::InvalidOrigin);
    }
    Ok(normalized)
}

#[derive(Clone, Default)]
pub(crate) struct RuntimePermissionManager {
    grants: Arc<RwLock<HashMap<ComponentRef, HashSet<RuntimePermission>>>>,
    http_loopback_origins: Arc<RwLock<HashMap<ComponentRef, HashSet<String>>>>,
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

    /// Host-side approval of one exact local HTTP origin, after the component has
    /// already received its manifest-requested general HTTP permission.
    pub(crate) fn approve_http_loopback_origin(
        &self,
        owner: ComponentRef,
        origin: &str,
    ) -> Result<(), HttpOriginGrantError> {
        let canonical_origin = parse_exact_local_http_origin(origin)?;
        let granted = self
            .has_grant(&owner, RuntimePermission::HttpRequest)
            .map_err(|()| HttpOriginGrantError::Unavailable)?;
        if !granted {
            return Err(HttpOriginGrantError::PermissionDenied);
        }
        let mut approvals = self
            .http_loopback_origins
            .write()
            .map_err(|_| HttpOriginGrantError::Unavailable)?;
        approvals.entry(owner).or_default().insert(canonical_origin);
        Ok(())
    }

    /// Checks whether the exact origin remains host-approved for one principal.
    /// An unrelated loopback port or any LAN address never inherits this grant.
    pub(crate) fn has_http_loopback_origin(
        &self,
        owner: &ComponentRef,
        url: &Url,
    ) -> Result<bool, ()> {
        if !is_local_http_origin(url) {
            return Ok(false);
        }
        if !self.has_grant(owner, RuntimePermission::HttpRequest)? {
            return Ok(false);
        }
        let approvals = self.http_loopback_origins.read().map_err(|_| ())?;
        Ok(approvals
            .get(owner)
            .is_some_and(|origins| origins.contains(&url.origin().ascii_serialization())))
    }

    pub(crate) fn revoke_http_loopback_origin(
        &self,
        owner: &ComponentRef,
        origin: &str,
    ) -> Result<(), HttpOriginGrantError> {
        let origin = parse_exact_local_http_origin(origin)?;
        let mut approvals = self
            .http_loopback_origins
            .write()
            .map_err(|_| HttpOriginGrantError::Unavailable)?;
        if let Some(origins) = approvals.get_mut(owner) {
            origins.remove(&origin);
            if origins.is_empty() {
                approvals.remove(owner);
            }
        }
        Ok(())
    }

    pub(crate) fn revoke_component(&self, owner: &ComponentRef) {
        let mut grants = match self.grants.write() {
            Ok(grants) => grants,
            Err(poisoned) => poisoned.into_inner(),
        };
        grants.remove(owner);
        let mut approvals = match self.http_loopback_origins.write() {
            Ok(approvals) => approvals,
            Err(poisoned) => poisoned.into_inner(),
        };
        approvals.remove(owner);
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

#[cfg(test)]
mod http_origin_tests {
    use super::*;
    use rintawa_sdk::types::{ComponentId, ExtensionInstanceId};

    fn owner(name: &str) -> ComponentRef {
        ComponentRef::new(ExtensionInstanceId::new(name), ComponentId::new("runtime"))
    }

    #[test]
    fn test_should_reject_all_untrusted_http_origin_forms() {
        for input in [
            "http://example.com:11434",
            "https://localhost:11434",
            "http://10.0.0.1:11434",
            "http://169.254.169.254",
            "http://localhost:11434/v1",
            "http://localhost:11434?secret=1",
            "http://localhost:11434/#fragment",
            "http://user@localhost:11434",
            "http://user:pass@localhost:11434",
            "http://127.0.0.1:0",
            "http://localhost:11434///",
            "http://localhost:11434%2f@evil.example",
            " http://localhost:11434",
            "http://localhost:11434 ",
            "http://[::ffff:127.0.0.1]:11434",
        ] {
            assert!(
                matches!(
                    parse_exact_local_http_origin(input),
                    Err(HttpOriginGrantError::InvalidOrigin)
                ),
                "unexpectedly accepted {input:?}"
            );
        }
    }

    #[test]
    fn test_should_normalize_approved_loopback_origins_without_leaking_other_ports() {
        let policy = RuntimePermissionManager::default();
        let owner = owner("first");
        policy
            .grant(owner.clone(), RuntimePermission::HttpRequest)
            .expect("test runtime grant should succeed");
        policy
            .approve_http_loopback_origin(owner.clone(), "http://LOCALHOST:11434/")
            .expect("case normalized host origin should be accepted");
        let granted =
            Url::parse("http://localhost:11434/api/generate").expect("test URL should parse");
        let different_port =
            Url::parse("http://localhost:11435/api/generate").expect("test URL should parse");
        assert!(
            policy
                .has_http_loopback_origin(&owner, &granted)
                .expect("policy readable")
        );
        assert!(
            !policy
                .has_http_loopback_origin(&owner, &different_port)
                .expect("policy readable")
        );
        policy
            .revoke_http_loopback_origin(&owner, "http://localhost:11434")
            .expect("test origin revoke should work");
        assert!(
            !policy
                .has_http_loopback_origin(&owner, &granted)
                .expect("policy readable")
        );
    }

    #[test]
    fn test_should_reject_origin_approval_without_manifest_grant() {
        let policy = RuntimePermissionManager::default();
        let owner = owner("first");
        assert!(matches!(
            policy.approve_http_loopback_origin(owner, "http://127.0.0.1:11434"),
            Err(HttpOriginGrantError::PermissionDenied)
        ));
    }

    #[test]
    fn test_should_isolate_same_component_across_two_extension_instances() {
        let policy = RuntimePermissionManager::default();
        let first = owner("first");
        let second = owner("second");
        for principal in [&first, &second] {
            policy
                .grant(principal.clone(), RuntimePermission::HttpRequest)
                .expect("test runtime grant should succeed");
        }
        policy
            .approve_http_loopback_origin(first.clone(), "http://127.0.0.1:11434")
            .expect("first principal origin should be granted");
        let url = Url::parse("http://127.0.0.1:11434/").expect("test URL should parse");
        assert!(
            policy
                .has_http_loopback_origin(&first, &url)
                .expect("policy readable")
        );
        assert!(
            !policy
                .has_http_loopback_origin(&second, &url)
                .expect("policy readable")
        );
    }

    #[test]
    fn test_should_revoke_all_origins_when_owning_component_unregistered() {
        let policy = RuntimePermissionManager::default();
        let owner = owner("first");
        policy
            .grant(owner.clone(), RuntimePermission::HttpRequest)
            .expect("test runtime grant should succeed");
        for origin in ["http://localhost:11434", "http://127.0.0.1:5000"] {
            policy
                .approve_http_loopback_origin(owner.clone(), origin)
                .expect("origin approval should work");
        }
        policy.revoke_component(&owner);
        for origin in ["http://localhost:11434", "http://127.0.0.1:5000"] {
            assert!(
                !policy
                    .has_http_loopback_origin(
                        &owner,
                        &Url::parse(origin).expect("test URL should parse")
                    )
                    .expect("policy readable")
            );
        }
        assert!(
            !policy
                .has_grant(&owner, RuntimePermission::HttpRequest)
                .expect("policy readable")
        );
    }

    #[test]
    fn test_should_support_exact_ipv6_loopback_approval_without_lan_access() {
        let policy = RuntimePermissionManager::default();
        let owner = owner("first");
        policy
            .grant(owner.clone(), RuntimePermission::HttpRequest)
            .expect("test grant should succeed");
        policy
            .approve_http_loopback_origin(owner.clone(), "http://[::1]:11434")
            .expect("IPv6 loopback should be authorized");
        assert!(
            policy
                .has_http_loopback_origin(
                    &owner,
                    &Url::parse("http://[::1]:11434/api").expect("test URL should parse")
                )
                .expect("policy readable")
        );
        assert!(
            !policy
                .has_http_loopback_origin(
                    &owner,
                    &Url::parse("http://[fe80::1]:11434/api").expect("test URL should parse")
                )
                .expect("policy readable")
        );
    }
}
