//! Owner-scoped generic service routing.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock, TryLockError},
};

use rintawa_sdk::{
    api::LoggerApi,
    context::ComponentContext,
    contracts::{ComponentRef, ContractKey, ContractProtocol},
    errors::{ExtensionError, ExtensionResult},
    secrets::{SecretPath, SecretValue},
    services::{ServiceCallError, ServiceCallResult},
    traits::Component,
    types::{ComponentId, ExtensionId},
};
use tracing::warn;

use crate::{
    composition::{
        OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider, resolve_contracts,
    },
    context::EngineLogger,
    secrets::SecretManager,
};

pub(crate) const DEFAULT_MAX_SERVICE_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) type ComponentHandle = Arc<Mutex<Box<dyn Component>>>;

#[derive(Clone)]
struct ServiceExtension {
    is_active: bool,
    definitions: Vec<OwnedContractDefinition>,
    providers: Vec<OwnedContractProvider>,
    consumers: Vec<OwnedContractConsumer>,
    components: HashMap<ComponentId, ComponentHandle>,
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
    extensions: HashMap<ExtensionId, ServiceExtension>,
    preferred_providers: HashMap<ContractKey, ComponentRef>,
    topology_revision: u64,
    route_cache: HashMap<RouteKey, CachedRoute>,
}

impl ServiceRuntimeState {
    fn topology_changed(&mut self) {
        self.topology_revision = self.topology_revision.wrapping_add(1);
        self.route_cache.clear();
    }

    fn component(&self, owner: &ComponentRef) -> Option<ComponentHandle> {
        self.extensions
            .get(&owner.extension_id)
            .and_then(|extension| extension.components.get(&owner.component_id))
            .cloned()
    }
}

/// Shared runtime used by native and WASM components for unary service calls.
#[derive(Clone)]
pub(crate) struct ServiceRuntime {
    state: Arc<RwLock<ServiceRuntimeState>>,
    secrets: SecretManager,
    max_message_bytes: usize,
}

impl ServiceRuntime {
    pub(crate) fn new(secrets: SecretManager) -> Self {
        Self {
            state: Arc::new(RwLock::new(ServiceRuntimeState::default())),
            secrets,
            max_message_bytes: DEFAULT_MAX_SERVICE_MESSAGE_BYTES,
        }
    }

    pub(crate) fn register_extension(
        &self,
        extension_id: ExtensionId,
        definitions: Vec<OwnedContractDefinition>,
        providers: Vec<OwnedContractProvider>,
        consumers: Vec<OwnedContractConsumer>,
        components: impl IntoIterator<Item = (ComponentId, ComponentHandle)>,
    ) -> Result<(), ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        state.extensions.insert(
            extension_id,
            ServiceExtension {
                is_active: false,
                definitions,
                providers,
                consumers,
                components: components.into_iter().collect(),
            },
        );
        state.topology_changed();
        Ok(())
    }

    pub(crate) fn unregister_extension(&self, extension_id: &ExtensionId) {
        if let Ok(mut state) = self.state.write() {
            let removed = state.extensions.remove(extension_id).is_some();
            let preferred_count = state.preferred_providers.len();
            state
                .preferred_providers
                .retain(|_, provider| &provider.extension_id != extension_id);
            if removed || state.preferred_providers.len() != preferred_count {
                state.topology_changed();
            }
        }
    }

    pub(crate) fn set_active(&self, extension_id: &ExtensionId, is_active: bool) {
        if let Ok(mut state) = self.state.write()
            && let Some(extension) = state.extensions.get_mut(extension_id)
            && extension.is_active != is_active
        {
            extension.is_active = is_active;
            state.topology_changed();
        }
    }

    pub(crate) fn set_preferred_provider(&self, contract: ContractKey, provider: ComponentRef) {
        if let Ok(mut state) = self.state.write()
            && state.preferred_providers.get(&contract) != Some(&provider)
        {
            state.preferred_providers.insert(contract, provider);
            state.topology_changed();
        }
    }

    pub(crate) fn clear_preferred_provider(&self, contract: &ContractKey) {
        if let Ok(mut state) = self.state.write()
            && state.preferred_providers.remove(contract).is_some()
        {
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

        let (provider, component) =
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
                    extension = %provider.extension_id,
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
    ) -> ServiceCallResult<(ComponentRef, ComponentHandle)> {
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
                    return Ok((provider, component));
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
                return Ok((provider, component));
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
            return Ok((provider, component));
        }
    }

    fn resolve_uncached(
        &self,
        state: &ServiceRuntimeState,
        caller: &ComponentRef,
        contract: &ContractKey,
        allow_inactive_caller: bool,
    ) -> ServiceCallResult<ComponentRef> {
        let mut definitions = Vec::new();
        let mut providers = Vec::new();
        let mut consumers = Vec::new();

        for (extension_id, extension) in &state.extensions {
            if extension.is_active {
                definitions.extend(extension.definitions.iter().cloned());
                providers.extend(extension.providers.iter().cloned());
                consumers.extend(extension.consumers.iter().cloned());
            } else if allow_inactive_caller && extension_id == &caller.extension_id {
                definitions.extend(extension.definitions.iter().cloned());
                consumers.extend(
                    extension
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
            .find(|entry| entry.definition.contract == *contract)
            .is_some_and(|entry| entry.definition.protocol != ContractProtocol::Service)
        {
            return Err(ServiceCallError::NotServiceContract);
        }

        let snapshot = resolve_contracts(
            &definitions,
            &providers,
            &consumers,
            &state.preferred_providers,
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
    logger: EngineLogger,
    services: ServiceRuntime,
    secrets: SecretManager,
    call_stack: Vec<ComponentRef>,
}

impl ServiceComponentContext {
    fn new(
        owner: ComponentRef,
        services: ServiceRuntime,
        secrets: SecretManager,
        call_stack: Vec<ComponentRef>,
    ) -> Self {
        Self {
            logger: EngineLogger::new(owner.extension_id.clone(), owner.component_id.clone()),
            owner,
            services,
            secrets,
            call_stack,
        }
    }
}

impl ComponentContext for ServiceComponentContext {
    fn extension_id(&self) -> &ExtensionId {
        &self.owner.extension_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.owner.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }

    fn read_secret(&self, path: &SecretPath) -> ExtensionResult<SecretValue> {
        self.secrets
            .read_for_component(&self.owner.extension_id, &self.owner.component_id, path)
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
