//! Owner- and scope-isolated generic service routing.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock, TryLockError},
};

use rintawa_sdk::{
    api::LoggerApi,
    context::ComponentContext,
    contracts::{
        ComponentRef, ContractDefinition, ContractKey, ContractProtocol, ContractResolutionPolicy,
    },
    errors::{ExtensionError, ExtensionResult},
    secrets::{SecretPath, SecretValue},
    services::{ServiceCallError, ServiceCallResult, ServiceProviderHandle},
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};
use tracing::warn;

use crate::{
    composition::{
        OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider,
        resolve_contract_providers, resolve_contracts,
    },
    context::{ComponentIdentity, EngineLogger},
    secrets::SecretManager,
};

pub(crate) const DEFAULT_MAX_SERVICE_MESSAGE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_SERVICE_PROVIDERS_PER_CONTRACT: usize = 64;

pub(crate) type ComponentHandle = Arc<Mutex<Box<dyn Component>>>;

#[derive(Clone)]
struct ServiceExtensionInstance {
    extension_id: ExtensionId,
    scope_id: RuntimeScopeId,
    is_active: bool,
    definitions: Vec<OwnedContractDefinition>,
    providers: Vec<OwnedContractProvider>,
    consumers: Vec<OwnedContractConsumer>,
    components: HashMap<ComponentId, ComponentHandle>,
}

/// Complete metadata staged before one service runtime instance is registered.
pub(crate) struct ServiceInstanceRegistration {
    pub(crate) instance_id: ExtensionInstanceId,
    pub(crate) extension_id: ExtensionId,
    pub(crate) scope_id: RuntimeScopeId,
    pub(crate) definitions: Vec<OwnedContractDefinition>,
    pub(crate) providers: Vec<OwnedContractProvider>,
    pub(crate) consumers: Vec<OwnedContractConsumer>,
    pub(crate) components: HashMap<ComponentId, ComponentHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    caller: ComponentRef,
    contract: ContractKey,
    allow_inactive_caller: bool,
}

#[derive(Clone)]
struct CachedRoute {
    topology_revision: u64,
    policy_revision: u64,
    result: ServiceCallResult<ComponentRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderHandleKey {
    caller: ComponentRef,
    contract: ContractKey,
    provider: ComponentRef,
    allow_inactive_caller: bool,
}

#[derive(Clone)]
struct ProviderHandleBinding {
    key: ProviderHandleKey,
}

struct ResolvedServiceBinding {
    resolution: ContractResolutionPolicy,
    providers: Vec<ComponentRef>,
}

#[derive(Default)]
struct ServiceRuntimeState {
    instances: HashMap<ExtensionInstanceId, ServiceExtensionInstance>,
    platform_definitions: HashMap<RuntimeScopeId, HashMap<ContractKey, ContractDefinition>>,
    preferred_providers: HashMap<RuntimeScopeId, HashMap<ContractKey, ComponentRef>>,
    topology_revision: u64,
    route_cache: HashMap<RouteKey, CachedRoute>,
    next_provider_handle: u64,
    provider_handle_policy_revision: u64,
    provider_handles: HashMap<ServiceProviderHandle, ProviderHandleBinding>,
    provider_handle_cache: HashMap<ProviderHandleKey, ServiceProviderHandle>,
}

impl ServiceRuntimeState {
    fn topology_changed(&mut self) {
        self.topology_revision = self.topology_revision.wrapping_add(1);
        self.route_cache.clear();
        self.provider_handles.clear();
        self.provider_handle_cache.clear();
    }

    fn sync_provider_handle_policy(&mut self, policy_revision: u64) {
        if self.provider_handle_policy_revision != policy_revision {
            self.provider_handle_policy_revision = policy_revision;
            self.provider_handles.clear();
            self.provider_handle_cache.clear();
        }
    }

    fn issue_provider_handle(
        &mut self,
        key: ProviderHandleKey,
    ) -> ServiceCallResult<ServiceProviderHandle> {
        if let Some(handle) = self.provider_handle_cache.get(&key) {
            return Ok(*handle);
        }
        let raw = self
            .next_provider_handle
            .checked_add(1)
            .ok_or(ServiceCallError::Unavailable)?;
        self.next_provider_handle = raw;
        let handle = ServiceProviderHandle::from_raw(raw);
        self.provider_handles
            .insert(handle, ProviderHandleBinding { key: key.clone() });
        self.provider_handle_cache.insert(key, handle);
        Ok(handle)
    }

    fn component(&self, owner: &ComponentRef) -> Option<ComponentHandle> {
        self.instances
            .get(&owner.instance_id)
            .and_then(|instance| instance.components.get(&owner.component_id))
            .cloned()
    }

    fn component_identity(&self, owner: &ComponentRef) -> Option<ComponentIdentity> {
        let instance = self.instances.get(&owner.instance_id)?;
        instance
            .components
            .contains_key(&owner.component_id)
            .then(|| {
                ComponentIdentity::new(
                    instance.extension_id.clone(),
                    owner.instance_id.clone(),
                    instance.scope_id.clone(),
                    owner.component_id.clone(),
                )
            })
    }
}

/// Shared runtime used by native and WASM components for routed service calls.
///
/// Providers are currently visible only inside the caller's exact runtime scope.
/// A future scope-import policy can widen visibility without changing component
/// principals or route-cache identity.
#[derive(Clone)]
pub(crate) struct ServiceRuntime {
    state: Arc<RwLock<ServiceRuntimeState>>,
    secrets: SecretManager,
    max_message_bytes: usize,
}

/// Cloneable service caller permanently bound to one component principal.
///
/// Creating this handle grants no service capability by itself. Every call still
/// resolves through the ordinary service runtime and therefore requires the bound
/// component to be an active declared consumer in the exact runtime scope.
#[derive(Clone)]
pub struct BoundServiceCaller {
    services: ServiceRuntime,
    consumer: ComponentRef,
}

/// Cloneable host caller restricted to platform-owned services in one exact scope.
#[derive(Clone)]
pub struct PlatformServiceCaller {
    services: ServiceRuntime,
    scope_id: RuntimeScopeId,
    provider_extension_id: Option<ExtensionId>,
}

impl BoundServiceCaller {
    pub(crate) fn new(services: ServiceRuntime, consumer: ComponentRef) -> Self {
        Self { services, consumer }
    }

    /// Calls one versioned unary service as the bound component principal.
    pub fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self.services.call(&self.consumer, contract, request)
    }

    /// Lists eligible providers for one bound `Multiple` service contract.
    pub fn list_providers(
        &self,
        contract: &ContractKey,
    ) -> ServiceCallResult<Vec<ServiceProviderHandle>> {
        self.services.list_providers(&self.consumer, contract)
    }

    /// Calls one provider previously returned by [`Self::list_providers`].
    pub fn call_provider(
        &self,
        provider: ServiceProviderHandle,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.services
            .call_provider(&self.consumer, provider, request)
    }

    /// Returns the immutable component principal represented by this handle.
    pub const fn consumer(&self) -> &ComponentRef {
        &self.consumer
    }
}

impl PlatformServiceCaller {
    pub(crate) fn new(services: ServiceRuntime, scope_id: RuntimeScopeId) -> Self {
        Self {
            services,
            scope_id,
            provider_extension_id: None,
        }
    }

    pub(crate) fn for_extension(
        services: ServiceRuntime,
        scope_id: RuntimeScopeId,
        provider_extension_id: ExtensionId,
    ) -> Self {
        Self {
            services,
            scope_id,
            provider_extension_id: Some(provider_extension_id),
        }
    }

    /// Calls one platform-owned unary service in the bound runtime scope.
    ///
    /// Extension-defined contracts are rejected even if they share an active provider.
    pub fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self.services.call_platform(
            &self.scope_id,
            self.provider_extension_id.as_ref(),
            contract,
            request,
        )
    }

    /// Returns the exact composition scope represented by this handle.
    pub const fn scope_id(&self) -> &RuntimeScopeId {
        &self.scope_id
    }

    /// Returns the required provider extension when this caller is owner-pinned.
    pub const fn provider_extension_id(&self) -> Option<&ExtensionId> {
        self.provider_extension_id.as_ref()
    }
}

impl ServiceRuntime {
    pub(crate) fn component_scope_id(&self, owner: &ComponentRef) -> Option<RuntimeScopeId> {
        self.state
            .read()
            .ok()?
            .instances
            .get(&owner.instance_id)
            .map(|instance| instance.scope_id.clone())
    }

    pub(crate) fn new(secrets: SecretManager) -> Self {
        Self {
            state: Arc::new(RwLock::new(ServiceRuntimeState::default())),
            secrets,
            max_message_bytes: DEFAULT_MAX_SERVICE_MESSAGE_BYTES,
        }
    }

    pub(crate) fn define_platform_contract(
        &self,
        scope_id: RuntimeScopeId,
        definition: ContractDefinition,
    ) -> Result<(), ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        let definitions = state.platform_definitions.entry(scope_id).or_default();
        if let Some(existing) = definitions.get(&definition.contract) {
            if existing != &definition {
                return Err(());
            }
            return Ok(());
        }
        definitions.insert(definition.contract.clone(), definition);
        state.topology_changed();
        Ok(())
    }

    pub(crate) fn clear_platform_contracts_in_scope(&self, scope_id: &RuntimeScopeId) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.platform_definitions.remove(scope_id).is_some() {
            state.topology_changed();
        }
    }

    pub(crate) fn register_instance(
        &self,
        registration: ServiceInstanceRegistration,
    ) -> Result<(), ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        if state.instances.contains_key(&registration.instance_id) {
            return Err(());
        }
        state.instances.insert(
            registration.instance_id,
            ServiceExtensionInstance {
                extension_id: registration.extension_id,
                scope_id: registration.scope_id,
                is_active: false,
                definitions: registration.definitions,
                providers: registration.providers,
                consumers: registration.consumers,
                components: registration.components,
            },
        );
        state.topology_changed();
        Ok(())
    }

    pub(crate) fn unregister_instance(&self, instance_id: &ExtensionInstanceId) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.instances.remove(instance_id).is_some() {
            state.topology_changed();
        }
    }

    pub(crate) fn activate_instance(&self, instance_id: &ExtensionInstanceId) -> Result<(), ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        let instance = state.instances.get_mut(instance_id).ok_or(())?;
        if !instance.is_active {
            instance.is_active = true;
            state.topology_changed();
        }
        Ok(())
    }

    pub(crate) fn deactivate_instance(&self, instance_id: &ExtensionInstanceId) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(instance) = state.instances.get_mut(instance_id)
            && instance.is_active
        {
            instance.is_active = false;
            state.topology_changed();
        }
    }

    pub(crate) fn set_preferred_provider(
        &self,
        scope_id: RuntimeScopeId,
        contract: ContractKey,
        provider: ComponentRef,
    ) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let providers = state.preferred_providers.entry(scope_id).or_default();
        if providers.get(&contract) != Some(&provider) {
            providers.insert(contract, provider);
            state.topology_changed();
        }
    }

    pub(crate) fn clear_preferred_provider(
        &self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let removed = state
            .preferred_providers
            .get_mut(scope_id)
            .is_some_and(|providers| providers.remove(contract).is_some());
        if removed {
            if state
                .preferred_providers
                .get(scope_id)
                .is_some_and(HashMap::is_empty)
            {
                state.preferred_providers.remove(scope_id);
            }
            state.topology_changed();
        }
    }

    pub(crate) fn call(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.call_with_stack(caller, contract, request, false, &[])
    }

    pub(crate) fn call_from_execution(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.call_with_stack(caller, contract, request, true, &[])
    }

    pub(crate) fn list_providers(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
    ) -> ServiceCallResult<Vec<ServiceProviderHandle>> {
        self.list_providers_with_mode(caller, contract, false)
    }

    pub(crate) fn list_providers_from_execution(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
    ) -> ServiceCallResult<Vec<ServiceProviderHandle>> {
        self.list_providers_with_mode(caller, contract, true)
    }

    pub(crate) fn call_provider(
        &self,
        caller: &ComponentRef,
        provider: ServiceProviderHandle,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.call_provider_with_stack(caller, provider, request, false, &[])
    }

    pub(crate) fn call_provider_from_execution(
        &self,
        caller: &ComponentRef,
        provider: ServiceProviderHandle,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.call_provider_with_stack(caller, provider, request, true, &[])
    }

    pub(crate) fn call_platform(
        &self,
        scope_id: &RuntimeScopeId,
        provider_extension_id: Option<&ExtensionId>,
        contract: &ContractKey,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        if request.len() > self.max_message_bytes {
            return Err(ServiceCallError::RequestTooLarge);
        }
        let (provider, component, provider_identity) =
            self.resolve_platform_provider(scope_id, provider_extension_id, contract)?;
        self.invoke_provider(
            provider,
            component,
            provider_identity,
            contract,
            request,
            &[],
        )
    }

    fn list_providers_with_mode(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<Vec<ServiceProviderHandle>> {
        loop {
            let policy_revision = self.secrets.policy_revision();
            let mut state = self
                .state
                .write()
                .map_err(|_| ServiceCallError::Unavailable)?;
            state.sync_provider_handle_policy(policy_revision);
            let binding =
                self.resolve_binding_uncached(&state, caller, contract, allow_inactive_caller)?;
            if binding.resolution != ContractResolutionPolicy::Multiple {
                return Err(ServiceCallError::UnsupportedResolution);
            }
            if binding.providers.len() > DEFAULT_MAX_SERVICE_PROVIDERS_PER_CONTRACT {
                return Err(ServiceCallError::ResponseTooLarge);
            }
            if self.secrets.policy_revision() != policy_revision {
                drop(state);
                continue;
            }

            let mut handles = Vec::with_capacity(binding.providers.len());
            for provider in binding.providers {
                handles.push(state.issue_provider_handle(ProviderHandleKey {
                    caller: caller.clone(),
                    contract: contract.clone(),
                    provider,
                    allow_inactive_caller,
                })?);
            }
            return Ok(handles);
        }
    }

    fn call_provider_with_stack(
        &self,
        caller: &ComponentRef,
        provider_handle: ServiceProviderHandle,
        request: &[u8],
        allow_inactive_caller: bool,
        call_stack: &[ComponentRef],
    ) -> ServiceCallResult<Vec<u8>> {
        if request.len() > self.max_message_bytes {
            return Err(ServiceCallError::RequestTooLarge);
        }
        let (contract, provider, component, provider_identity) =
            self.resolve_provider_handle(caller, provider_handle, allow_inactive_caller)?;
        self.invoke_provider(
            provider,
            component,
            provider_identity,
            &contract,
            request,
            call_stack,
        )
    }

    fn resolve_provider_handle(
        &self,
        caller: &ComponentRef,
        provider_handle: ServiceProviderHandle,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<(
        ContractKey,
        ComponentRef,
        ComponentHandle,
        ComponentIdentity,
    )> {
        let policy_revision = self.secrets.policy_revision();
        let state = self
            .state
            .read()
            .map_err(|_| ServiceCallError::Unavailable)?;
        if state.provider_handle_policy_revision != policy_revision {
            return Err(ServiceCallError::Unavailable);
        }
        let binding = state
            .provider_handles
            .get(&provider_handle)
            .ok_or(ServiceCallError::Unavailable)?;
        if binding.key.caller != *caller
            || binding.key.allow_inactive_caller != allow_inactive_caller
        {
            return Err(ServiceCallError::Unavailable);
        }
        let provider = binding.key.provider.clone();
        let component = state
            .component(&provider)
            .ok_or(ServiceCallError::Unavailable)?;
        let identity = state
            .component_identity(&provider)
            .ok_or(ServiceCallError::Unavailable)?;
        Ok((binding.key.contract.clone(), provider, component, identity))
    }

    fn call_with_stack(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
        request: &[u8],
        allow_inactive_caller: bool,
        call_stack: &[ComponentRef],
    ) -> ServiceCallResult<Vec<u8>> {
        if request.len() > self.max_message_bytes {
            return Err(ServiceCallError::RequestTooLarge);
        }

        let (provider, component, provider_identity) =
            self.resolve_provider(caller, contract, allow_inactive_caller)?;
        self.invoke_provider(
            provider,
            component,
            provider_identity,
            contract,
            request,
            call_stack,
        )
    }

    fn invoke_provider(
        &self,
        provider: ComponentRef,
        component: ComponentHandle,
        provider_identity: ComponentIdentity,
        contract: &ContractKey,
        request: &[u8],
        call_stack: &[ComponentRef],
    ) -> ServiceCallResult<Vec<u8>> {
        if call_stack.contains(&provider) {
            return Err(ServiceCallError::CyclicCall);
        }
        let mut component = match component.try_lock() {
            Ok(component) => component,
            Err(TryLockError::WouldBlock) => return Err(ServiceCallError::ProviderBusy),
            Err(TryLockError::Poisoned(_)) => return Err(ServiceCallError::ProviderFailed),
        };
        let provider_limit = component
            .service_message_limit()
            .unwrap_or(self.max_message_bytes)
            .min(self.max_message_bytes);
        if request.len() > provider_limit {
            return Err(ServiceCallError::RequestTooLarge);
        }

        let mut next_call_stack = call_stack.to_vec();
        next_call_stack.push(provider.clone());
        let mut context = ServiceComponentContext::new(
            provider.clone(),
            provider_identity.clone(),
            self.clone(),
            self.secrets.clone(),
            next_call_stack,
        );
        let response = component
            .handle_service(&mut context, contract, request)
            .map_err(|error| {
                let transport_error = match &error {
                    ExtensionError::HostMessageTooLarge {
                        operation: "service request",
                        ..
                    } => ServiceCallError::RequestTooLarge,
                    ExtensionError::HostMessageTooLarge {
                        operation: "service response",
                        ..
                    } => ServiceCallError::ResponseTooLarge,
                    _ => ServiceCallError::ProviderFailed,
                };
                warn!(
                    extension = %provider_identity.extension_id,
                    instance = %provider.instance_id,
                    scope = %provider_identity.scope_id,
                    component = %provider.component_id,
                    contract = %contract,
                    error = %error,
                    "service provider callback failed"
                );
                transport_error
            })?;
        if response.len() > provider_limit {
            return Err(ServiceCallError::ResponseTooLarge);
        }
        Ok(response)
    }

    fn resolve_platform_provider(
        &self,
        scope_id: &RuntimeScopeId,
        provider_extension_id: Option<&ExtensionId>,
        contract: &ContractKey,
    ) -> ServiceCallResult<(ComponentRef, ComponentHandle, ComponentIdentity)> {
        loop {
            let policy_revision = self.secrets.policy_revision();
            let state = self
                .state
                .read()
                .map_err(|_| ServiceCallError::Unavailable)?;
            let definition = state
                .platform_definitions
                .get(scope_id)
                .and_then(|definitions| definitions.get(contract))
                .ok_or(ServiceCallError::Unavailable)?;
            if definition.protocol != ContractProtocol::Service {
                return Err(ServiceCallError::NotServiceContract);
            }

            let providers = state
                .instances
                .values()
                .filter(|instance| {
                    instance.is_active
                        && &instance.scope_id == scope_id
                        && provider_extension_id
                            .is_none_or(|expected| &instance.extension_id == expected)
                })
                .flat_map(|instance| instance.providers.iter().cloned())
                .collect::<Vec<_>>();
            let preferred = state
                .preferred_providers
                .get(scope_id)
                .and_then(|providers| providers.get(contract));
            let resolved = resolve_contract_providers(
                contract,
                definition.resolution,
                &providers,
                preferred,
                &self.secrets,
            )
            .map_err(|_| ServiceCallError::Unavailable)?;
            if self.secrets.policy_revision() != policy_revision {
                drop(state);
                continue;
            }
            if resolved.len() != 1 {
                return Err(ServiceCallError::UnsupportedResolution);
            }
            let provider = resolved[0].clone();
            let component = state
                .component(&provider)
                .ok_or(ServiceCallError::Unavailable)?;
            let identity = state
                .component_identity(&provider)
                .ok_or(ServiceCallError::Unavailable)?;
            return Ok((provider, component, identity));
        }
    }

    fn resolve_provider(
        &self,
        caller: &ComponentRef,
        contract: &ContractKey,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<(ComponentRef, ComponentHandle, ComponentIdentity)> {
        let key = RouteKey {
            caller: caller.clone(),
            contract: contract.clone(),
            allow_inactive_caller,
        };

        loop {
            let policy_revision = self.secrets.policy_revision();
            {
                let state = self
                    .state
                    .read()
                    .map_err(|_| ServiceCallError::Unavailable)?;
                if let Some(cached) = state.route_cache.get(&key)
                    && cached.topology_revision == state.topology_revision
                    && cached.policy_revision == policy_revision
                {
                    let provider = cached.result.clone()?;
                    let component = state
                        .component(&provider)
                        .ok_or(ServiceCallError::Unavailable)?;
                    let identity = state
                        .component_identity(&provider)
                        .ok_or(ServiceCallError::Unavailable)?;
                    return Ok((provider, component, identity));
                }
            }

            let mut state = self
                .state
                .write()
                .map_err(|_| ServiceCallError::Unavailable)?;
            let policy_revision = self.secrets.policy_revision();
            if let Some(cached) = state.route_cache.get(&key)
                && cached.topology_revision == state.topology_revision
                && cached.policy_revision == policy_revision
            {
                let provider = cached.result.clone()?;
                let component = state
                    .component(&provider)
                    .ok_or(ServiceCallError::Unavailable)?;
                let identity = state
                    .component_identity(&provider)
                    .ok_or(ServiceCallError::Unavailable)?;
                return Ok((provider, component, identity));
            }

            let result = self.resolve_uncached(&state, caller, contract, allow_inactive_caller);
            if self.secrets.policy_revision() != policy_revision {
                continue;
            }
            let topology_revision = state.topology_revision;
            state.route_cache.insert(
                key.clone(),
                CachedRoute {
                    topology_revision,
                    policy_revision,
                    result: result.clone(),
                },
            );
            let provider = result?;
            let component = state
                .component(&provider)
                .ok_or(ServiceCallError::Unavailable)?;
            let identity = state
                .component_identity(&provider)
                .ok_or(ServiceCallError::Unavailable)?;
            return Ok((provider, component, identity));
        }
    }

    fn resolve_uncached(
        &self,
        state: &ServiceRuntimeState,
        caller: &ComponentRef,
        contract: &ContractKey,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<ComponentRef> {
        let binding =
            self.resolve_binding_uncached(state, caller, contract, allow_inactive_caller)?;
        if binding.providers.len() != 1 {
            return Err(ServiceCallError::UnsupportedResolution);
        }
        Ok(binding.providers[0].clone())
    }

    fn resolve_binding_uncached(
        &self,
        state: &ServiceRuntimeState,
        caller: &ComponentRef,
        contract: &ContractKey,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<ResolvedServiceBinding> {
        let caller_instance = state
            .instances
            .get(&caller.instance_id)
            .ok_or(ServiceCallError::Unavailable)?;
        let caller_scope = &caller_instance.scope_id;

        let mut definitions = state
            .platform_definitions
            .get(caller_scope)
            .into_iter()
            .flat_map(|definitions| definitions.values().cloned())
            .collect::<Vec<_>>();
        let mut providers = Vec::new();
        let mut consumers = Vec::new();

        for (instance_id, instance) in &state.instances {
            if &instance.scope_id != caller_scope {
                continue;
            }
            if instance.is_active {
                definitions.extend(
                    instance
                        .definitions
                        .iter()
                        .map(|owned| owned.definition.clone()),
                );
                providers.extend(instance.providers.iter().cloned());
                consumers.extend(instance.consumers.iter().cloned());
            } else if allow_inactive_caller && instance_id == &caller.instance_id {
                definitions.extend(
                    instance
                        .definitions
                        .iter()
                        .map(|owned| owned.definition.clone()),
                );
                consumers.extend(
                    instance
                        .consumers
                        .iter()
                        .filter(|entry| entry.owner == *caller)
                        .cloned(),
                );
            }
        }

        if !consumers
            .iter()
            .any(|entry| entry.owner == *caller && entry.consumer.contract == *contract)
        {
            return Err(ServiceCallError::NotConsumer);
        }

        let definition = definitions
            .iter()
            .find(|entry| entry.contract == *contract)
            .ok_or(ServiceCallError::Unavailable)?;
        if definition.protocol != ContractProtocol::Service {
            return Err(ServiceCallError::NotServiceContract);
        }

        let empty_preferences = HashMap::new();
        let preferences = state
            .preferred_providers
            .get(caller_scope)
            .unwrap_or(&empty_preferences);
        let snapshot = resolve_contracts(
            &definitions,
            &providers,
            &consumers,
            preferences,
            &self.secrets,
        );
        let binding = snapshot
            .bindings
            .iter()
            .find(|entry| entry.consumer == *caller && entry.contract == *contract)
            .ok_or(ServiceCallError::Unavailable)?;
        Ok(ResolvedServiceBinding {
            resolution: definition.resolution,
            providers: binding.providers.clone(),
        })
    }
}

struct ServiceComponentContext {
    owner: ComponentRef,
    identity: ComponentIdentity,
    logger: EngineLogger,
    services: ServiceRuntime,
    secrets: SecretManager,
    call_stack: Vec<ComponentRef>,
}

impl ServiceComponentContext {
    fn new(
        owner: ComponentRef,
        identity: ComponentIdentity,
        services: ServiceRuntime,
        secrets: SecretManager,
        call_stack: Vec<ComponentRef>,
    ) -> Self {
        Self {
            logger: EngineLogger::new(identity.clone()),
            owner,
            identity,
            services,
            secrets,
            call_stack,
        }
    }
}

impl ComponentContext for ServiceComponentContext {
    fn extension_id(&self) -> &ExtensionId {
        &self.identity.extension_id
    }

    fn extension_instance_id(&self) -> &ExtensionInstanceId {
        &self.identity.instance_id
    }

    fn runtime_scope_id(&self) -> &RuntimeScopeId {
        &self.identity.scope_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.owner.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }

    fn read_secret(&self, path: &SecretPath) -> ExtensionResult<SecretValue> {
        self.secrets
            .read_for_component(&self.owner, path)
            .map_err(Into::into)
    }

    fn call_service(
        &mut self,
        contract: &ContractKey,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.services
            .call_with_stack(&self.owner, contract, request, true, &self.call_stack)
    }

    fn list_service_providers(
        &mut self,
        contract: &ContractKey,
    ) -> ServiceCallResult<Vec<ServiceProviderHandle>> {
        self.services
            .list_providers_from_execution(&self.owner, contract)
    }

    fn call_service_provider(
        &mut self,
        provider: ServiceProviderHandle,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        self.services.call_provider_with_stack(
            &self.owner,
            provider,
            request,
            true,
            &self.call_stack,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use rintawa_sdk::contracts::ContractVersion;

    use super::*;

    fn registration(instance_id: ExtensionInstanceId) -> ServiceInstanceRegistration {
        ServiceInstanceRegistration {
            instance_id,
            extension_id: ExtensionId::new("example.service"),
            scope_id: RuntimeScopeId::new("host"),
            definitions: Vec::new(),
            providers: Vec::new(),
            consumers: Vec::new(),
            components: HashMap::new(),
        }
    }

    #[test]
    fn test_should_apply_preferred_provider_policy_after_service_state_lock_is_poisoned() {
        let runtime = ServiceRuntime::new(SecretManager::system());
        let scope_id = RuntimeScopeId::new("host");
        let contract = ContractKey::new("example.policy", ContractVersion::new(1));
        let provider = ComponentRef::new("example.provider", "runtime");

        let poisoner = runtime.clone();
        let _ = thread::spawn(move || {
            let _state = poisoner
                .state
                .write()
                .expect("test service state lock should start healthy");
            panic!("poison service runtime state lock");
        })
        .join();

        runtime.set_preferred_provider(scope_id.clone(), contract.clone(), provider.clone());
        {
            let state = match runtime.state.read() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            assert_eq!(
                state
                    .preferred_providers
                    .get(&scope_id)
                    .and_then(|providers| providers.get(&contract)),
                Some(&provider)
            );
        }

        runtime.clear_preferred_provider(&scope_id, &contract);
        let state = match runtime.state.read() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(
            state
                .preferred_providers
                .get(&scope_id)
                .is_none_or(HashMap::is_empty)
        );
    }

    #[test]
    fn test_should_fail_activation_after_service_state_lock_is_poisoned() {
        let runtime = ServiceRuntime::new(SecretManager::system());
        let instance_id = ExtensionInstanceId::new("example.service");
        runtime
            .register_instance(registration(instance_id.clone()))
            .expect("test service instance should register");

        let poisoner = runtime.clone();
        let _ = thread::spawn(move || {
            let _state = poisoner
                .state
                .write()
                .expect("test service state lock should start healthy");
            panic!("poison service runtime state lock");
        })
        .join();

        assert_eq!(runtime.activate_instance(&instance_id), Err(()));
        runtime.deactivate_instance(&instance_id);
        let state = match runtime.state.read() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!state.instances[&instance_id].is_active);
    }
}
