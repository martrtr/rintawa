//! Host-owned live execution identities and a read-only composition view.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use rintawa_sdk::{
    contracts::{ComponentPrincipal, ComponentRef, ContractDefinition, ContractKey},
    runtime_permissions::RuntimePermission,
    types::{ExtensionInstanceId, RuntimeScopeId},
};

use crate::{
    composition::{OwnedContractProvider, resolve_contract_providers},
    host_access::{HostAccessError, HostAccessResult, UserResourceAccess, UserResourceRef},
    secrets::SecretManager,
    transport::TransportError,
};

static NEXT_ACTIVATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionContext {
    pub(crate) scope: RuntimeScopeId,
    pub(crate) principal: ComponentPrincipal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedComponent {
    pub(crate) digest: rintawa_artifacts::ArtifactDigest,
    pub(crate) published_key: Option<String>,
    pub(crate) permissions: rintawa_sdk::manifest::ComponentPermissions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishedPreferenceSource {
    pub(crate) consumer: ExecutionContext,
    pub(crate) source: ExecutionContext,
    pub(crate) digest: rintawa_artifacts::ArtifactDigest,
    pub(crate) key: String,
    pub(crate) write_requested: bool,
}

struct Binding {
    verified: Option<VerifiedComponent>,
    context: ExecutionContext,
    active: bool,
    provider: Option<ComponentRef>,
    resource_access: Option<Arc<dyn UserResourceAccess>>,
}

/// Detached cleanup runs after execution and transport locks have been released.
pub(crate) struct ResourceCleanup {
    principal: ComponentPrincipal,
    access: Arc<dyn UserResourceAccess>,
}
impl ResourceCleanup {
    pub(crate) fn dispose(self) {
        self.access.release_owned_resources(&self.principal);
    }
}
impl Binding {
    fn into_cleanup(self) -> Option<ResourceCleanup> {
        self.resource_access.map(|access| ResourceCleanup {
            principal: self.context.principal,
            access,
        })
    }
}
#[derive(Default)]
pub(crate) struct RetiredContexts {
    pub(crate) owners: Vec<ComponentRef>,
    resources: Vec<ResourceCleanup>,
}
impl RetiredContexts {
    pub(crate) fn dispose(self) {
        for cleanup in self.resources {
            cleanup.dispose();
        }
    }
}

#[derive(Default)]
pub(crate) struct ScopeSelection {
    pub(crate) definitions: HashMap<ContractKey, ContractDefinition>,
    pub(crate) providers: Vec<OwnedContractProvider>,
    pub(crate) preferred: HashMap<ContractKey, ComponentRef>,
}

#[derive(Default)]
struct View {
    bindings: HashMap<ComponentRef, Binding>,
    scopes: HashMap<RuntimeScopeId, ScopeSelection>,
}

#[derive(Clone, Default)]
pub(crate) struct ExecutionContexts(Arc<Mutex<View>>, pub(crate) Arc<Mutex<()>>);

impl ExecutionContexts {
    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        owner: ComponentRef,
        scope: RuntimeScopeId,
    ) -> Result<(), TransportError> {
        self.begin_verified(owner, scope, None)
    }

    pub(crate) fn begin_verified(
        &self,
        owner: ComponentRef,
        scope: RuntimeScopeId,
        verified: Option<VerifiedComponent>,
    ) -> Result<(), TransportError> {
        let lifecycle = self.1.lock().map_err(|_| TransportError::Unavailable)?;
        let activation = NEXT_ACTIVATION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map_err(|_| TransportError::LimitExceeded)?;
        let context = ExecutionContext {
            scope,
            principal: ComponentPrincipal {
                instance_id: owner.instance_id.clone(),
                component_id: owner.component_id.clone(),
                activation,
            },
        };
        let displaced = self
            .0
            .lock()
            .map_err(|_| TransportError::Unavailable)?
            .bindings
            .insert(
                owner,
                Binding {
                    verified,
                    context,
                    active: false,
                    provider: None,
                    resource_access: None,
                },
            );
        drop(lifecycle);
        if let Some(cleanup) = displaced.and_then(Binding::into_cleanup) {
            cleanup.dispose();
        }
        Ok(())
    }

    pub(crate) fn activate_instance(&self, instance: &ExtensionInstanceId) {
        let _lifecycle = self.1.lock().unwrap_or_else(|e| e.into_inner());
        let mut view = self.0.lock().unwrap_or_else(|error| error.into_inner());
        for (owner, binding) in &mut view.bindings {
            if &owner.instance_id == instance {
                binding.active = true;
            }
        }
    }

    pub(crate) fn context(
        &self,
        owner: &ComponentRef,
        starting: bool,
    ) -> Result<ExecutionContext, TransportError> {
        let view = self.0.lock().map_err(|_| TransportError::Unavailable)?;
        let binding = view
            .bindings
            .get(owner)
            .filter(|binding| starting || binding.active)
            .ok_or(TransportError::Inactive)?;
        Ok(binding.context.clone())
    }

    pub(crate) fn try_validate(&self, context: &ExecutionContext) -> Result<(), TransportError> {
        self.try_validate_execution(context, false)
    }

    pub(crate) fn try_validate_execution(
        &self,
        context: &ExecutionContext,
        starting: bool,
    ) -> Result<(), TransportError> {
        let view = self.0.try_lock().map_err(|_| TransportError::Unavailable)?;
        if !view
            .bindings
            .get(&context.principal.component())
            .is_some_and(|binding| (starting || binding.active) && binding.context == *context)
        {
            return Err(TransportError::Inactive);
        }
        Ok(())
    }

    pub(crate) fn validate(&self, context: &ExecutionContext) -> Result<(), TransportError> {
        if self.context(&context.principal.component(), false)? != *context {
            return Err(TransportError::Inactive);
        }
        Ok(())
    }

    pub(crate) fn policy_identity(
        &self,
        owner: &ComponentRef,
    ) -> Result<(ExecutionContext, VerifiedComponent), TransportError> {
        let view = self.0.try_lock().map_err(|_| TransportError::Unavailable)?;
        let b = view
            .bindings
            .get(owner)
            .filter(|b| b.active)
            .ok_or(TransportError::Inactive)?;
        Ok((
            b.context.clone(),
            b.verified.clone().ok_or(TransportError::Unavailable)?,
        ))
    }

    /// Resolve only Host-attested live artifacts. Preference I/O holds no view lock.
    pub(crate) fn published_preference_source(
        &self,
        consumer: &ExecutionContext,
    ) -> Result<PublishedPreferenceSource, TransportError> {
        let view = self.0.try_lock().map_err(|_| TransportError::Unavailable)?;
        let reader = view
            .bindings
            .get(&consumer.principal.component())
            .filter(|b| b.active && b.provider.is_none() && b.context == *consumer)
            .ok_or(TransportError::Inactive)?;
        let identity = reader
            .verified
            .as_ref()
            .ok_or(TransportError::Unavailable)?;
        let mut candidates = view.bindings.values().filter(|b| {
            b.active
                && b.provider.is_none()
                && b.context.scope.as_str() == "host"
                && b.context.principal.component_id == consumer.principal.component_id
                && b.verified
                    .as_ref()
                    .is_some_and(|v| v.digest == identity.digest && v.published_key.is_some())
        });
        let source = candidates.next().ok_or(TransportError::Unavailable)?;
        if candidates.next().is_some() {
            return Err(TransportError::Unavailable);
        }
        Ok(PublishedPreferenceSource {
            consumer: consumer.clone(),
            source: source.context.clone(),
            digest: identity.digest.clone(),
            key: source
                .verified
                .as_ref()
                .unwrap()
                .published_key
                .as_ref()
                .unwrap()
                .clone(),
            write_requested: identity
                .permissions
                .runtime
                .contains(&RuntimePermission::PublishedPreferenceWrite),
        })
    }

    pub(crate) fn validate_published_read(
        &self,
        read: &PublishedPreferenceSource,
    ) -> Result<(), TransportError> {
        if self.published_preference_source(&read.consumer)? != *read {
            return Err(TransportError::Unavailable);
        }
        Ok(())
    }

    pub(crate) fn bind_provider(
        &self,
        context: &ExecutionContext,
        provider: ComponentRef,
    ) -> Result<(), TransportError> {
        let mut view = self.0.lock().map_err(|_| TransportError::Unavailable)?;
        if !view
            .bindings
            .get(&provider)
            .is_some_and(|binding| binding.active)
        {
            return Err(TransportError::Inactive);
        }
        let binding = view
            .bindings
            .get_mut(&context.principal.component())
            .filter(|binding| binding.context == *context)
            .ok_or(TransportError::Inactive)?;
        binding.provider = Some(provider);
        Ok(())
    }

    pub(crate) fn revoke_instance_if(
        &self,
        instance: &ExtensionInstanceId,
        expected: Option<&ExecutionContext>,
    ) -> RetiredContexts {
        let _lifecycle = self.1.lock().unwrap_or_else(|e| e.into_inner());
        let mut view = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(expected) = expected
            && !view
                .bindings
                .get(&expected.principal.component())
                .is_some_and(|binding| binding.context == *expected)
        {
            return RetiredContexts::default();
        }
        let owners: Vec<_> = view
            .bindings
            .iter()
            .filter(|(owner, binding)| {
                &owner.instance_id == instance
                    || binding
                        .provider
                        .as_ref()
                        .is_some_and(|provider| &provider.instance_id == instance)
            })
            .map(|(owner, _)| owner.clone())
            .collect();
        let mut retired = RetiredContexts {
            owners,
            resources: Vec::new(),
        };
        for owner in &retired.owners {
            if let Some(cleanup) = view.bindings.remove(owner).and_then(Binding::into_cleanup) {
                retired.resources.push(cleanup);
            }
        }
        retired
    }

    /// The permission read guard is held by the caller through this transition.
    /// No guest callbacks, data copies, entropy or I/O may occur while View is held.
    pub(crate) fn take_resource(
        &self,
        context: &ExecutionContext,
        service: Arc<dyn UserResourceAccess>,
        reference: &UserResourceRef,
        replacement_id: &str,
    ) -> Result<HostAccessResult<UserResourceRef>, TransportError> {
        let mut view = self.0.try_lock().map_err(|_| TransportError::Unavailable)?;
        let binding = view
            .bindings
            .get_mut(&context.principal.component())
            .filter(|binding| binding.active && binding.context == *context)
            .ok_or(TransportError::Inactive)?;
        if binding
            .resource_access
            .as_ref()
            .is_some_and(|current| !Arc::ptr_eq(current, &service))
        {
            return Ok(Err(HostAccessError::Rejected));
        }
        let result = service.take_resource(reference, &context.principal, replacement_id);
        if result.is_ok() {
            binding.resource_access = Some(service);
        }
        Ok(result)
    }

    /// Effective permission revocation keeps its write guard through disposal.
    pub(crate) fn detach_resource_access(&self, owner: &ComponentRef) -> Option<ResourceCleanup> {
        let mut view = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let binding = view.bindings.get_mut(owner)?;
        binding
            .resource_access
            .take()
            .map(|access| ResourceCleanup {
                principal: binding.context.principal.clone(),
                access,
            })
    }

    pub(crate) fn replace_selection(&self, scopes: HashMap<RuntimeScopeId, ScopeSelection>) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .scopes = scopes;
    }

    pub(crate) fn selected(
        &self,
        executing: &ExecutionContext,
        contract: &ContractKey,
        secrets: &SecretManager,
    ) -> Result<Vec<ComponentPrincipal>, TransportError> {
        self.validate(executing)?;
        let view = self.0.lock().map_err(|_| TransportError::Unavailable)?;
        let scope = view
            .scopes
            .get(&executing.scope)
            .ok_or(TransportError::Unavailable)?;
        let definition = scope
            .definitions
            .get(contract)
            .ok_or(TransportError::InvalidName)?;
        let owners = resolve_contract_providers(
            contract,
            definition.resolution,
            &scope.providers,
            scope.preferred.get(contract),
            secrets,
        )
        .map_err(|_| TransportError::InvalidName)?;
        if owners.len() > 64 {
            return Err(TransportError::LimitExceeded);
        }
        let principals: Vec<_> = owners
            .iter()
            .map(|owner| {
                view.bindings
                    .get(owner)
                    .filter(|binding| binding.active && binding.context.scope == executing.scope)
                    .map(|binding| binding.context.principal.clone())
                    .ok_or(TransportError::InvalidName)
            })
            .collect::<Result<_, _>>()?;
        if serde_json::to_vec(&principals)
            .map_err(|_| TransportError::Unavailable)?
            .len()
            > 1024 * 1024
        {
            return Err(TransportError::LimitExceeded);
        }
        Ok(principals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::contracts::{ContractProvider, ContractResolutionPolicy, ContractVersion};

    #[test]
    fn test_should_never_reuse_activation_and_reject_stale_contexts() -> Result<(), TransportError>
    {
        let contexts = ExecutionContexts::default();
        let owner = ComponentRef::new("same-instance", "component");
        let scope = RuntimeScopeId::new("world-a");
        contexts.begin(owner.clone(), scope.clone())?;
        let first = contexts.context(&owner, true)?;
        assert_ne!(first.principal.activation, 0);
        assert!(contexts.validate(&first).is_err());
        contexts.activate_instance(&owner.instance_id);
        contexts.validate(&first)?;
        contexts
            .revoke_instance_if(&owner.instance_id, None)
            .dispose();
        contexts.begin(owner.clone(), scope.clone())?;
        contexts.activate_instance(&owner.instance_id);
        assert!(contexts.validate(&first).is_err());
        let second = contexts.context(&owner, false)?;
        assert_ne!(first.principal.activation, second.principal.activation);
        let another_engine = ExecutionContexts::default();
        another_engine.begin(owner.clone(), scope)?;
        assert_ne!(
            second.principal.activation,
            another_engine.context(&owner, true)?.principal.activation
        );
        Ok(())
    }

    #[test]
    fn test_should_reject_oversized_selected_sets_without_truncation() -> Result<(), TransportError>
    {
        let contexts = ExecutionContexts::default();
        let scope = RuntimeScopeId::new("world");
        let key = ContractKey::new("fixture.multiple", ContractVersion::new(1));
        let mut selection = ScopeSelection::default();
        selection.definitions.insert(
            key.clone(),
            ContractDefinition::new(key.clone(), ContractResolutionPolicy::Multiple),
        );
        let mut caller = None;
        for number in 0..65 {
            let owner = ComponentRef::new(format!("provider-{number:02}"), "runtime");
            contexts.begin(owner.clone(), scope.clone())?;
            contexts.activate_instance(&owner.instance_id);
            caller = Some(contexts.context(&owner, false)?);
            selection.providers.push(OwnedContractProvider {
                owner,
                provider: ContractProvider::new(key.clone()),
            });
        }
        contexts.replace_selection(HashMap::from([(scope, selection)]));
        assert!(matches!(
            contexts.selected(&caller.expect("caller"), &key, &SecretManager::default()),
            Err(TransportError::LimitExceeded)
        ));
        Ok(())
    }
}

#[cfg(test)]
mod resource_tests;

#[cfg(test)]
mod published_tests {
    use super::*;
    fn bind(
        contexts: &ExecutionContexts,
        name: &str,
        scope: &str,
        component: &str,
        digest: &[u8],
        published: bool,
    ) -> ExecutionContext {
        let owner = ComponentRef::new(name, component);
        contexts
            .begin_verified(
                owner.clone(),
                RuntimeScopeId::new(scope),
                Some(VerifiedComponent {
                    digest: rintawa_artifacts::ArtifactDigest::sha256(digest),
                    published_key: published.then(|| "fixture.shared.v1".into()),
                    permissions: Default::default(),
                }),
            )
            .unwrap();
        contexts.activate_instance(&owner.instance_id);
        contexts.context(&owner, false).unwrap()
    }
    #[test]
    fn test_published_preference_requires_live_exact_unambiguous_ordinary_source() {
        let c = ExecutionContexts::default();
        let a = bind(&c, "A", "world:a", "backend", b"exact", true);
        let b = bind(&c, "B", "world:b", "backend", b"exact", true);
        assert!(c.published_preference_source(&a).is_err());
        let source = bind(&c, "root", "host", "backend", b"exact", true);
        let read = c.published_preference_source(&a).unwrap();
        assert_eq!(read.source, source);
        assert_eq!(read.key, "fixture.shared.v1");
        let foreign = bind(&c, "foreign", "world:c", "backend", b"different", true);
        assert!(c.published_preference_source(&foreign).is_err());
        let wrong_component = bind(&c, "other-component", "world:d", "other", b"exact", true);
        assert!(c.published_preference_source(&wrong_component).is_err());
        let duplicate = bind(&c, "duplicate", "host", "backend", b"exact", true);
        assert!(c.published_preference_source(&a).is_err());
        c.revoke_instance_if(&duplicate.principal.instance_id, None)
            .dispose();
        c.validate_published_read(&read).unwrap();
        c.bind_provider(&a, source.principal.component()).unwrap();
        assert!(c.published_preference_source(&a).is_err());
        assert!(c.published_preference_source(&b).is_ok());
        c.revoke_instance_if(&source.principal.instance_id, None)
            .dispose();
        assert!(c.validate_published_read(&read).is_err());
        assert!(c.published_preference_source(&b).is_err());
        let replacement = bind(&c, "root", "host", "backend", b"exact", true);
        assert_ne!(
            replacement.principal.activation,
            source.principal.activation
        );
        assert!(c.validate_published_read(&read).is_err());
        assert_eq!(
            c.published_preference_source(&b).unwrap().source,
            replacement
        );
        c.revoke_instance_if(&replacement.principal.instance_id, None)
            .dispose();
        bind(&c, "unpublished", "host", "backend", b"exact", false);
        assert!(c.published_preference_source(&b).is_err());
        let unverified = ComponentRef::new("unverified", "backend");
        c.begin(unverified.clone(), RuntimeScopeId::new("world:u"))
            .unwrap();
        c.activate_instance(&unverified.instance_id);
        assert!(
            c.published_preference_source(&c.context(&unverified, false).unwrap())
                .is_err()
        );
    }
}
