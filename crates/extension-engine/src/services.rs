//! Owner- and scope-isolated generic service routing.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock, TryLockError},
};

use rintawa_sdk::{
    api::LoggerApi,
    context::ComponentContext,
    contracts::{ComponentRef, ContractDefinition, ContractKey, ContractProtocol},
    errors::{ExtensionError, ExtensionResult},
    secrets::{SecretPath, SecretValue},
    services::{ServiceCallError, ServiceCallResult},
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};
use tracing::warn;

use crate::{
    composition::{
        OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider, resolve_contracts,
    },
    context::{ComponentIdentity, EngineLogger},
    secrets::SecretManager,
};

pub(crate) const DEFAULT_MAX_SERVICE_MESSAGE_BYTES: usize = 1024 * 1024;

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

#[derive(Default)]
struct ServiceRuntimeState {
    instances: HashMap<ExtensionInstanceId, ServiceExtensionInstance>,
    platform_definitions: HashMap<RuntimeScopeId, HashMap<ContractKey, ContractDefinition>>,
    preferred_providers: HashMap<RuntimeScopeId, HashMap<ContractKey, ComponentRef>>,
    topology_revision: u64,
    route_cache: HashMap<RouteKey, CachedRoute>,
}

impl ServiceRuntimeState {
    fn topology_changed(&mut self) {
        self.topology_revision = self.topology_revision.wrapping_add(1);
        self.route_cache.clear();
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

/// Shared runtime used by native and WASM components for unary service calls.
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

impl BoundServiceCaller {
    pub(crate) fn new(services: ServiceRuntime, consumer: ComponentRef) -> Self {
        Self { services, consumer }
    }

    /// Calls one versioned unary service as the bound component principal.
    pub fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self.services.call(&self.consumer, contract, request)
    }

    /// Returns the immutable component principal represented by this handle.
    pub const fn consumer(&self) -> &ComponentRef {
        &self.consumer
    }
}

impl ServiceRuntime {
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

        if definitions
            .iter()
            .find(|entry| entry.contract == *contract)
            .is_some_and(|entry| entry.protocol != ContractProtocol::Service)
        {
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
        if binding.providers.len() != 1 {
            return Err(ServiceCallError::UnsupportedResolution);
        }
        Ok(binding.providers[0].clone())
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
