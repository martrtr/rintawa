//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Rintawa WASM extensions.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contracts::{
        ContractConsumer, ContractDefinition, ContractGrantRequirement, ContractKey,
        ContractProtocol, ContractProvider, ContractResolutionPolicy, ContractVersion,
    },
    contributions::{ContributionDescriptor, ContributionKind},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    runtime_permissions::RuntimePermission,
    secrets::{SecretAccessError, SecretPath, SecretPathPattern},
    services::ServiceCallError,
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeEffectId, RuntimeScopeId},
    ui::{
        UiActionEvent, UiCapabilityId, UiError, UiPatchBatch, UiPlacementHint,
        UiSurfaceContribution, UiSurfaceId, UiSurfaceSnapshot,
    },
};

use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Engine, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Component as WasmtimeComponent, Instance, Linker, Resource, ResourceTable},
};

use crate::{
    artifact_host::{
        RtwComponentHost, RtwComponentHostError, RtwComponentHostResult, RtwComponentSource,
    },
    errors::{EngineError, EngineResult},
    execution_targets::ExecutionTargetRegistry,
    runtime_permissions::RuntimePermissionManager,
    secrets::SecretManager,
    services::ServiceRuntime,
};
use rintawa_ui_runtime::UiRuntime;

const TARGET_PROVIDER_EXPORT_NAME: &str = "rintawa:engine/target-provider@0.0.1";
const TASK_HANDLER_EXPORT_NAME: &str = "rintawa:engine/task-handler@0.0.1";

#[derive(Default)]
struct JsonSizeCounter {
    bytes: usize,
}

impl Write for JsonSizeCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Host-owned resource limits for one WASM component instance.
///
/// Each lifecycle callback receives a fresh `fuel_per_callback` allowance;
/// unused fuel is discarded before the next callback. Memory and table limits
/// are enforced by Wasmtime for the lifetime of the component store. The
/// defaults are a deliberately conservative local-host baseline, not a public
/// package ABI: a production supervisor may choose a stricter policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmExecutionBudget {
    /// Largest accepted compiled component artifact in bytes.
    pub max_component_bytes: usize,
    /// Maximum size of each guest linear memory in bytes.
    pub max_memory_bytes: usize,
    /// Maximum number of elements in each guest table.
    pub max_table_elements: usize,
    /// Maximum core instances allocated by one component store.
    pub max_instances: usize,
    /// Maximum tables allocated by one component store.
    pub max_tables: usize,
    /// Maximum linear memories allocated by one component store.
    pub max_memories: usize,
    /// Fuel made available before each guest callback and instantiation.
    pub fuel_per_callback: u64,
    /// Maximum byte length of an inbound event topic or payload.
    pub max_host_message_bytes: usize,
    /// Maximum bytes returned by one bounded target-artifact resource read.
    pub max_artifact_read_bytes: usize,
    /// Maximum cooperative background tasks owned by one WASM component.
    pub max_background_tasks: usize,
    /// Smallest periodic task interval accepted from a guest.
    pub min_background_task_interval_ms: u32,
    /// Maximum live loopback listener/stream handles owned by one component.
    pub max_network_handles: usize,
    /// Maximum payload accepted by one loopback read or write.
    pub max_network_io_bytes: usize,
    /// Maximum time a loopback connect host call may block.
    pub loopback_connect_timeout_ms: u64,
}

impl Default for WasmExecutionBudget {
    fn default() -> Self {
        Self {
            max_component_bytes: 32 * 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            max_table_elements: 100_000,
            max_instances: 32,
            max_tables: 16,
            max_memories: 8,
            fuel_per_callback: 10_000_000,
            max_host_message_bytes: 1024 * 1024,
            max_artifact_read_bytes: 8 * 1024 * 1024,
            max_background_tasks: 8,
            min_background_task_interval_ms: 10,
            max_network_handles: 64,
            max_network_io_bytes: 64 * 1024,
            loopback_connect_timeout_ms: 250,
        }
    }
}

impl WasmExecutionBudget {
    fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_memory_bytes)
            .table_elements(self.max_table_elements)
            .instances(self.max_instances)
            .tables(self.max_tables)
            .memories(self.max_memories)
            .build()
    }
}

#[allow(missing_docs)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "plugin",
        async: false,
        with: {
            "rintawa:engine/execution-targets/artifact-source":
                crate::artifact_host::OwnedRtwComponentSource,
        },
    });
}

#[allow(missing_docs)]
mod target_provider_bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "target-provider-plugin",
        async: false,
        with: {
            "rintawa:engine/execution-targets/artifact-source":
                crate::artifact_host::OwnedRtwComponentSource,
        },
    });
}

#[allow(missing_docs)]
mod task_bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "task-plugin",
        async: false,
    });
}

use bindings::Plugin;
use bindings::rintawa::engine::{
    execution_targets::{
        ArtifactError as TargetArtifactError, Host as ExecutionTargetsHost, HostArtifactSource,
        RegistrationError as TargetRegistrationError,
    },
    host::{Host as HostOperations, LogLevel, PublishError},
    network::{
        Error as NetworkError, Host as NetworkHost, Listener as WitNetworkListener,
        ReadResult as WitNetworkReadResult,
    },
    portable_ui::{
        Error as PortableUiError, Host as PortableUiHost, PlacementHint as WitPlacementHint,
    },
    registration::{
        ContractProtocol as WitContractProtocol, Error as RegistrationError,
        Host as RegistrationHost, ResolutionPolicy as WitResolutionPolicy,
    },
    runtime_effects::{Error as RuntimeEffectError, Host as RuntimeEffectsHost},
    runtime_tasks::{Error as RuntimeTaskError, Host as RuntimeTasksHost},
    secrets::{Error as SecretError, Host as SecretsHost},
    services::{Error as ServiceTransportError, Host as ServicesHost},
};
use target_provider_bindings::TargetProviderPlugin;
use target_provider_bindings::exports::rintawa::engine::target_provider::{
    ComponentDescriptor as WitTargetComponentDescriptor, Error as WitTargetError,
};
use task_bindings::TaskPlugin;

/// Internal host state stored inside the Wasmtime Store context.
pub struct WasmHostState {
    component_id: ComponentId,
    extension_id: Option<ExtensionId>,
    instance_id: Option<ExtensionInstanceId>,
    scope_id: Option<RuntimeScopeId>,
    execution_owner: Option<rintawa_sdk::contracts::ComponentRef>,
    registration_scope: Option<WasmRegistrationScope>,
    runtime_effects_active: bool,
    next_effect_handle: u64,
    effect_handles: HashMap<String, ActiveWasmRuntimeEffect>,
    pending_effects: Vec<WasmRuntimeEffectOperation>,
    pending_revocations: HashSet<String>,
    runtime_permissions: RuntimePermissionManager,
    task_access_active: bool,
    task_handler_available: bool,
    next_task_handle: u64,
    active_tasks: HashMap<u64, ActiveWasmTask>,
    network_access_active: bool,
    next_network_handle: u64,
    network_handles: HashMap<u64, OwnedNetworkHandle>,
    max_background_tasks: usize,
    min_background_task_interval_ms: u32,
    max_network_handles: usize,
    max_network_io_bytes: usize,
    loopback_connect_timeout_ms: u64,
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    secret_access_active: bool,
    service_access_active: bool,
    ui_access_active: bool,
    max_host_message_bytes: usize,
    max_artifact_read_bytes: usize,
    execution_target_registration_active: bool,
    pending_execution_targets: Vec<String>,
    resource_limits: StoreLimits,
    resource_table: ResourceTable,
}

/// Registrations produced by one guest `register` invocation before the host
/// commits them to the Extension Engine.
struct WasmRegistrationScope {
    contributions: Vec<ContributionDescriptor>,
    capabilities: HashSet<String>,
    definitions: Vec<ContractDefinition>,
    definition_keys: HashSet<ContractKey>,
    providers: Vec<ContractProvider>,
    provider_keys: HashSet<ContractKey>,
    consumers: Vec<ContractConsumer>,
    consumer_keys: HashSet<ContractKey>,
    ui_surfaces: Vec<UiSurfaceContribution>,
    ui_surface_ids: HashSet<UiSurfaceId>,
}

struct WasmRegistrations {
    contributions: Vec<ContributionDescriptor>,
    definitions: Vec<ContractDefinition>,
    providers: Vec<ContractProvider>,
    consumers: Vec<ContractConsumer>,
    ui_surfaces: Vec<UiSurfaceContribution>,
}

/// A guest request that is committed through the Engine-owned effect registry.
enum WasmRuntimeEffectOperation {
    Subscribe {
        owner: rintawa_sdk::contracts::ComponentRef,
        handle: String,
        topic: String,
    },
    Unsubscribe {
        handle: String,
        effect: ActiveWasmRuntimeEffect,
    },
}

/// The Engine effect currently associated with one opaque guest handle.
#[derive(Clone)]
struct ActiveWasmRuntimeEffect {
    owner: rintawa_sdk::contracts::ComponentRef,
    effect_id: RuntimeEffectId,
    effect: RuntimeEffect,
}

struct ActiveWasmTask {
    owner: rintawa_sdk::contracts::ComponentRef,
    interval: Duration,
    next_due: Instant,
}

enum NetworkHandleKind {
    Listener(TcpListener),
    Stream(TcpStream),
}

struct OwnedNetworkHandle {
    owner: rintawa_sdk::contracts::ComponentRef,
    kind: NetworkHandleKind,
}

#[derive(Debug, Clone, Copy)]
enum RuntimePermissionCheck {
    Denied,
    Unavailable,
}

impl WasmHostState {
    /// Creates a new host state instance.
    pub fn new(component_id: ComponentId) -> Self {
        let secrets = SecretManager::system();
        let services = ServiceRuntime::new(secrets.clone());
        Self::with_host_services_and_budget(
            component_id,
            secrets,
            services,
            UiRuntime::new(),
            RuntimePermissionManager::default(),
            false,
            &WasmExecutionBudget::default(),
        )
    }

    /// Creates host state with the Rintawa secret manager shared by the runtime.
    pub fn with_secret_manager(component_id: ComponentId, secrets: SecretManager) -> Self {
        let services = ServiceRuntime::new(secrets.clone());
        Self::with_host_services_and_budget(
            component_id,
            secrets,
            services,
            UiRuntime::new(),
            RuntimePermissionManager::default(),
            false,
            &WasmExecutionBudget::default(),
        )
    }

    fn with_host_services_and_budget(
        component_id: ComponentId,
        secrets: SecretManager,
        services: ServiceRuntime,
        ui: UiRuntime,
        runtime_permissions: RuntimePermissionManager,
        task_handler_available: bool,
        budget: &WasmExecutionBudget,
    ) -> Self {
        Self {
            component_id,
            extension_id: None,
            instance_id: None,
            scope_id: None,
            execution_owner: None,
            registration_scope: None,
            runtime_effects_active: false,
            next_effect_handle: 0,
            effect_handles: HashMap::new(),
            pending_effects: Vec::new(),
            pending_revocations: HashSet::new(),
            runtime_permissions,
            task_access_active: false,
            task_handler_available,
            next_task_handle: 0,
            active_tasks: HashMap::new(),
            network_access_active: false,
            next_network_handle: 0,
            network_handles: HashMap::new(),
            max_background_tasks: budget.max_background_tasks,
            min_background_task_interval_ms: budget.min_background_task_interval_ms,
            max_network_handles: budget.max_network_handles,
            max_network_io_bytes: budget.max_network_io_bytes,
            loopback_connect_timeout_ms: budget.loopback_connect_timeout_ms,
            secrets,
            services,
            ui,
            secret_access_active: false,
            service_access_active: false,
            ui_access_active: false,
            max_host_message_bytes: budget.max_host_message_bytes,
            max_artifact_read_bytes: budget.max_artifact_read_bytes,
            execution_target_registration_active: false,
            pending_execution_targets: Vec::new(),
            resource_limits: budget.store_limits(),
            resource_table: ResourceTable::new(),
        }
    }

    /// Returns the active component ID.
    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn begin_registration(
        &mut self,
        extension_id: ExtensionId,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> ExtensionResult<()> {
        if self.registration_scope.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM component registration is already in progress",
            )));
        }

        if self.extension_id.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM component has already completed registration",
            )));
        }

        self.registration_scope = Some(WasmRegistrationScope {
            contributions: Vec::new(),
            capabilities: HashSet::new(),
            definitions: Vec::new(),
            definition_keys: HashSet::new(),
            providers: Vec::new(),
            provider_keys: HashSet::new(),
            consumers: Vec::new(),
            consumer_keys: HashSet::new(),
            ui_surfaces: Vec::new(),
            ui_surface_ids: HashSet::new(),
        });
        self.extension_id = Some(extension_id);
        self.instance_id = Some(instance_id);
        self.scope_id = Some(scope_id);
        Ok(())
    }

    fn begin_target_component_registration(&mut self) -> ExtensionResult<()> {
        self.execution_owner = None;
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = false;
        self.service_access_active = false;
        self.ui_access_active = false;
        self.execution_target_registration_active = false;
        if self.registration_scope.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM target-component registration is already in progress",
            )));
        }
        self.registration_scope = Some(WasmRegistrationScope {
            contributions: Vec::new(),
            capabilities: HashSet::new(),
            definitions: Vec::new(),
            definition_keys: HashSet::new(),
            providers: Vec::new(),
            provider_keys: HashSet::new(),
            consumers: Vec::new(),
            consumer_keys: HashSet::new(),
            ui_surfaces: Vec::new(),
            ui_surface_ids: HashSet::new(),
        });
        Ok(())
    }

    fn cancel_target_component_registration(&mut self) {
        self.registration_scope = None;
    }

    fn finish_registration(&mut self) -> ExtensionResult<WasmRegistrations> {
        let scope = self.registration_scope.take().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM component registration is not active"))
        })?;

        Ok(WasmRegistrations {
            contributions: scope.contributions,
            definitions: scope.definitions,
            providers: scope.providers,
            consumers: scope.consumers,
            ui_surfaces: scope.ui_surfaces,
        })
    }

    fn cancel_registration(&mut self) {
        self.registration_scope = None;
        self.extension_id = None;
        self.instance_id = None;
        self.scope_id = None;
    }

    fn registered_owner(&self) -> Option<rintawa_sdk::contracts::ComponentRef> {
        self.instance_id.as_ref().map(|instance_id| {
            rintawa_sdk::contracts::ComponentRef::new(
                instance_id.clone(),
                self.component_id.clone(),
            )
        })
    }

    fn current_execution_owner(&self) -> Option<&rintawa_sdk::contracts::ComponentRef> {
        self.execution_owner.as_ref()
    }

    fn validate_execution_owner(&self, ctx: &dyn ComponentContext) -> ExtensionResult<()> {
        let (Some(extension_id), Some(instance_id), Some(scope_id)) =
            (&self.extension_id, &self.instance_id, &self.scope_id)
        else {
            return Err(ExtensionError::Message(String::from(
                "WASM component has not completed registration",
            )));
        };

        if ctx.extension_id() != extension_id
            || ctx.extension_instance_id() != instance_id
            || ctx.runtime_scope_id() != scope_id
            || ctx.component_id() != &self.component_id
        {
            return Err(ExtensionError::Message(String::from(
                "WASM component received a context for a different owner",
            )));
        }

        Ok(())
    }

    fn discard_guest_execution(&mut self) {
        self.registration_scope = None;
        self.execution_owner = None;
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.pending_effects.clear();
        self.pending_revocations.clear();
        self.secret_access_active = false;
        self.service_access_active = false;
        self.ui_access_active = false;
        self.execution_target_registration_active = false;
        self.pending_execution_targets.clear();
    }

    fn queue_capability(&mut self, name: String) -> Result<(), RegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Rejecting WASM capability registration outside guest register callback"
            );
            return Err(RegistrationError::RegistrationNotActive);
        };

        if !scope.capabilities.insert(name.clone()) {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Ignoring duplicate WASM capability registration"
            );
            return Ok(());
        }

        scope.contributions.push(ContributionDescriptor::new(
            name,
            ContributionKind::capability(),
        ));
        Ok(())
    }

    fn remove_queued_capability(&mut self, name: &str) -> Result<(), RegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Rejecting WASM capability removal outside guest register callback"
            );
            return Err(RegistrationError::RegistrationNotActive);
        };

        if scope.capabilities.remove(name) {
            scope.contributions.retain(|contribution| {
                contribution.kind != ContributionKind::capability()
                    || contribution.id.as_str() != name
            });
        }
        Ok(())
    }

    fn queue_contract_definition(
        &mut self,
        contract: ContractKey,
        resolution: ContractResolutionPolicy,
        protocol: ContractProtocol,
    ) -> Result<(), RegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(RegistrationError::RegistrationNotActive);
        };
        if !scope.definition_keys.insert(contract.clone()) {
            return Err(RegistrationError::DuplicateContract);
        }
        scope.definitions.push(ContractDefinition {
            contract,
            resolution,
            protocol,
        });
        Ok(())
    }

    fn queue_contract_provider(
        &mut self,
        contract: ContractKey,
        required_secret_read: Vec<String>,
    ) -> Result<(), RegistrationError> {
        let requirements = Self::secret_requirements(required_secret_read)?;
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(RegistrationError::RegistrationNotActive);
        };
        if !scope.provider_keys.insert(contract.clone()) {
            return Err(RegistrationError::DuplicateContract);
        }
        let mut provider = ContractProvider::new(contract);
        provider.required_grants = requirements;
        scope.providers.push(provider);
        Ok(())
    }

    fn queue_contract_consumer(
        &mut self,
        contract: ContractKey,
        required: bool,
        required_secret_read: Vec<String>,
    ) -> Result<(), RegistrationError> {
        let requirements = Self::secret_requirements(required_secret_read)?;
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(RegistrationError::RegistrationNotActive);
        };
        if !scope.consumer_keys.insert(contract.clone()) {
            return Err(RegistrationError::DuplicateContract);
        }
        let mut consumer = ContractConsumer::new(contract, required);
        consumer.required_grants = requirements;
        scope.consumers.push(consumer);
        Ok(())
    }

    fn secret_requirements(
        patterns: Vec<String>,
    ) -> Result<Vec<ContractGrantRequirement>, RegistrationError> {
        patterns
            .into_iter()
            .map(|pattern| {
                SecretPathPattern::parse(pattern)
                    .map(|pattern| ContractGrantRequirement::SecretRead { pattern })
                    .map_err(|_| RegistrationError::InvalidSecretPattern)
            })
            .collect()
    }

    fn subscribe_event(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }
        if topic.trim().is_empty() {
            return Err(RuntimeEffectError::InvalidTopic);
        }

        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(RuntimeEffectError::RuntimeNotActive)?;
        let handle = format!("effect-{}", self.next_effect_handle);
        self.next_effect_handle = self
            .next_effect_handle
            .checked_add(1)
            .ok_or(RuntimeEffectError::RuntimeNotActive)?;
        self.pending_effects
            .push(WasmRuntimeEffectOperation::Subscribe {
                owner,
                handle: handle.clone(),
                topic,
            });
        Ok(handle)
    }

    fn unsubscribe_event(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }

        if let Some(index) = self.pending_effects.iter().position(|operation| {
            matches!(operation, WasmRuntimeEffectOperation::Subscribe { handle: pending, .. } if pending == &handle)
        }) {
            self.pending_effects.remove(index);
            return Ok(());
        }

        if self.pending_revocations.contains(&handle) {
            return Err(RuntimeEffectError::UnknownEffect);
        }

        let Some(effect) = self.effect_handles.get(&handle).cloned() else {
            return Err(RuntimeEffectError::UnknownEffect);
        };
        if self.current_execution_owner() != Some(&effect.owner) {
            return Err(RuntimeEffectError::UnknownEffect);
        }
        self.pending_revocations.insert(handle.clone());
        self.pending_effects
            .push(WasmRuntimeEffectOperation::Unsubscribe { handle, effect });
        Ok(())
    }

    /// Opens the normal guest execution scope for the provider component itself.
    fn begin_guest_execution(&mut self) {
        self.execution_owner = self.registered_owner();
        self.runtime_effects_active = true;
        self.task_access_active = true;
        self.network_access_active = true;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
        self.execution_target_registration_active = false;
    }

    fn begin_delegated_guest_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        self.runtime_effects_active = true;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
        self.execution_target_registration_active = false;
    }

    fn begin_start_execution(&mut self) {
        self.begin_guest_execution();
        self.execution_target_registration_active = true;
        self.pending_execution_targets.clear();
    }

    fn take_pending_execution_targets(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_execution_targets)
    }

    fn begin_service_execution(&mut self) {
        self.execution_owner = self.registered_owner();
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
    }

    fn begin_delegated_service_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
    }

    fn finish_service_execution(&mut self) {
        self.execution_owner = None;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = false;
        self.service_access_active = false;
        self.ui_access_active = false;
        self.execution_target_registration_active = false;
    }

    /// Closes guest access before committing its queued runtime effects.
    fn finish_guest_execution(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.secret_access_active = false;
        self.service_access_active = false;
        self.ui_access_active = false;
        self.execution_target_registration_active = false;

        let result = self.commit_runtime_effects(ctx);
        self.execution_owner = None;
        result
    }

    /// Commits reversible effects requested by the just-completed guest callback.
    fn commit_runtime_effects(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let mut registered_handles = Vec::new();
        let mut revoked_effects = Vec::new();
        for operation in std::mem::take(&mut self.pending_effects) {
            let operation_result = match operation {
                WasmRuntimeEffectOperation::Subscribe {
                    owner,
                    handle,
                    topic,
                } => {
                    let effect = RuntimeEffect::event_subscription(topic);
                    ctx.register_runtime_effect(effect.clone())
                        .map(|effect_id| {
                            self.effect_handles.insert(
                                handle.clone(),
                                ActiveWasmRuntimeEffect {
                                    owner,
                                    effect_id,
                                    effect,
                                },
                            );
                            registered_handles.push(handle);
                        })
                }
                WasmRuntimeEffectOperation::Unsubscribe { handle, effect } => {
                    ctx.revoke_runtime_effect(&effect.effect_id).map(|()| {
                        self.effect_handles.remove(&handle);
                        self.pending_revocations.remove(&handle);
                        revoked_effects.push((handle, effect));
                    })
                }
            };

            if let Err(error) = operation_result {
                if let Err(rollback_error) =
                    self.rollback_runtime_effects(ctx, registered_handles, revoked_effects)
                {
                    return Err(ExtensionError::RuntimeEffectRollbackFailed {
                        operation: error.to_string(),
                        rollback: rollback_error.to_string(),
                    });
                }
                return Err(error);
            }
        }

        Ok(())
    }

    fn rollback_runtime_effects(
        &mut self,
        ctx: &mut dyn ComponentContext,
        registered_handles: Vec<String>,
        revoked_effects: Vec<(String, ActiveWasmRuntimeEffect)>,
    ) -> ExtensionResult<()> {
        let mut rollback_errors = Vec::new();

        for handle in registered_handles.iter().rev() {
            if let Some(effect) = self.effect_handles.remove(handle)
                && let Err(error) = ctx.revoke_runtime_effect(&effect.effect_id)
            {
                self.effect_handles.insert(handle.clone(), effect);
                rollback_errors.push(error.to_string());
            }
        }

        for (handle, effect) in revoked_effects.into_iter().rev() {
            match ctx.register_runtime_effect(effect.effect.clone()) {
                Ok(effect_id) => {
                    self.effect_handles.insert(
                        handle.clone(),
                        ActiveWasmRuntimeEffect {
                            owner: effect.owner,
                            effect_id,
                            effect: effect.effect,
                        },
                    );
                }
                Err(error) => rollback_errors.push(error.to_string()),
            }
        }

        self.pending_revocations.clear();

        if rollback_errors.is_empty() {
            Ok(())
        } else {
            Err(ExtensionError::Message(rollback_errors.join("; ")))
        }
    }

    fn abort_guest_execution(
        &mut self,
        ctx: &mut dyn ComponentContext,
        operation_error: ExtensionError,
    ) -> ExtensionError {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        self.discard_guest_execution();
        self.effect_handles
            .retain(|_, effect| effect.owner != owner);
        self.revoke_runtime_resources_for_owner(&owner);
        if let Err(cleanup_error) = ctx.revoke_all_runtime_effects() {
            return ExtensionError::RuntimeEffectCleanupFailed {
                operation: operation_error.to_string(),
                cleanup: cleanup_error.to_string(),
            };
        }
        operation_error
    }

    fn forget_effect_handles_for_owner(&mut self, owner: &rintawa_sdk::contracts::ComponentRef) {
        self.effect_handles
            .retain(|_, effect| &effect.owner != owner);
        self.pending_revocations
            .retain(|handle| self.effect_handles.contains_key(handle));
    }

    fn root_runtime_owner(&self) -> Option<rintawa_sdk::contracts::ComponentRef> {
        let current = self.current_execution_owner()?;
        let registered = self.registered_owner()?;
        (current == &registered).then_some(registered)
    }

    fn runtime_permission_owner(
        &self,
        permission: RuntimePermission,
    ) -> Result<rintawa_sdk::contracts::ComponentRef, RuntimePermissionCheck> {
        let owner = self
            .root_runtime_owner()
            .ok_or(RuntimePermissionCheck::Denied)?;
        match self.runtime_permissions.has_grant(&owner, permission) {
            Ok(true) => Ok(owner),
            Ok(false) => Err(RuntimePermissionCheck::Denied),
            Err(()) => Err(RuntimePermissionCheck::Unavailable),
        }
    }

    fn allocate_network_handle(
        &mut self,
        owner: rintawa_sdk::contracts::ComponentRef,
        kind: NetworkHandleKind,
    ) -> Result<u64, NetworkError> {
        let owned_count = self
            .network_handles
            .values()
            .filter(|resource| resource.owner == owner)
            .count();
        if owned_count >= self.max_network_handles {
            return Err(NetworkError::LimitExceeded);
        }
        let handle = self.next_network_handle;
        self.next_network_handle = self
            .next_network_handle
            .checked_add(1)
            .ok_or(NetworkError::Unavailable)?;
        self.network_handles
            .insert(handle, OwnedNetworkHandle { owner, kind });
        Ok(handle)
    }

    fn revoke_runtime_resources_for_owner(&mut self, owner: &rintawa_sdk::contracts::ComponentRef) {
        self.active_tasks.retain(|_, task| &task.owner != owner);
        self.network_handles
            .retain(|_, resource| &resource.owner != owner);
    }

    fn next_task_due_in(
        &self,
        owner: &rintawa_sdk::contracts::ComponentRef,
        now: Instant,
    ) -> Option<Duration> {
        self.active_tasks
            .values()
            .filter(|task| &task.owner == owner)
            .map(|task| task.next_due.saturating_duration_since(now))
            .min()
    }

    fn due_task_handles(
        &self,
        owner: &rintawa_sdk::contracts::ComponentRef,
        now: Instant,
    ) -> Vec<u64> {
        let mut handles: Vec<_> = self
            .active_tasks
            .iter()
            .filter(|(_, task)| &task.owner == owner && task.next_due <= now)
            .map(|(handle, _)| *handle)
            .collect();
        handles.sort_unstable();
        handles
    }

    fn reschedule_task(&mut self, handle: u64, now: Instant) {
        if let Some(task) = self.active_tasks.get_mut(&handle) {
            task.next_due = now + task.interval;
        }
    }

    fn queue_ui_surface(
        &mut self,
        id: String,
        placement: WitPlacementHint,
        semantic_contract: Option<String>,
        semantic_version: Option<u32>,
        required_capabilities: Vec<String>,
    ) -> Result<(), PortableUiError> {
        let semantic = match (semantic_contract, semantic_version) {
            (Some(contract), Some(version)) => {
                Some(ContractKey::new(contract, ContractVersion::new(version)))
            }
            (None, None) => None,
            _ => return Err(PortableUiError::InvalidPayload),
        };
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(PortableUiError::RegistrationNotActive);
        };
        let id = UiSurfaceId::new(id);
        if !scope.ui_surface_ids.insert(id.clone()) {
            return Err(PortableUiError::DuplicateSurface);
        }
        let placement = match placement {
            WitPlacementHint::Primary => UiPlacementHint::Primary,
            WitPlacementHint::Secondary => UiPlacementHint::Secondary,
            WitPlacementHint::Sidebar => UiPlacementHint::Sidebar,
            WitPlacementHint::Settings => UiPlacementHint::Settings,
            WitPlacementHint::Dialog => UiPlacementHint::Dialog,
            WitPlacementHint::Status => UiPlacementHint::Status,
            WitPlacementHint::Overlay => UiPlacementHint::Overlay,
        };
        scope.ui_surfaces.push(UiSurfaceContribution {
            id,
            placement,
            semantic,
            required_capabilities: required_capabilities
                .into_iter()
                .map(UiCapabilityId::new)
                .collect(),
        });
        Ok(())
    }

    fn ui_owner(&self) -> Result<rintawa_sdk::contracts::ComponentRef, PortableUiError> {
        self.current_execution_owner()
            .cloned()
            .ok_or(PortableUiError::Unavailable)
    }

    fn validate_ui_message_size(&self, message_bytes: usize) -> Result<(), PortableUiError> {
        if message_bytes > self.max_host_message_bytes {
            return Err(PortableUiError::MessageTooLarge);
        }
        Ok(())
    }

    fn validate_ui_registration_size(
        &self,
        id: &str,
        placement: WitPlacementHint,
        semantic_contract: Option<&str>,
        semantic_version: Option<u32>,
        required_capabilities: &[String],
    ) -> Result<(), PortableUiError> {
        let placement = match placement {
            WitPlacementHint::Primary => "primary",
            WitPlacementHint::Secondary => "secondary",
            WitPlacementHint::Sidebar => "sidebar",
            WitPlacementHint::Settings => "settings",
            WitPlacementHint::Dialog => "dialog",
            WitPlacementHint::Status => "status",
            WitPlacementHint::Overlay => "overlay",
        };
        let mut counter = JsonSizeCounter::default();
        serde_json::to_writer(
            &mut counter,
            &(
                id,
                placement,
                semantic_contract,
                semantic_version,
                required_capabilities,
            ),
        )
        .map_err(|_| PortableUiError::InvalidPayload)?;
        self.validate_ui_message_size(counter.bytes)
    }

    fn map_ui_error(error: UiError) -> PortableUiError {
        match error {
            UiError::UnsupportedCapability(capability) => {
                PortableUiError::UnsupportedCapability(capability)
            }
            UiError::RuntimeUnavailable => PortableUiError::Unavailable,
            _ => PortableUiError::Rejected,
        }
    }

    fn read_secret(&self, path: String) -> Result<String, SecretError> {
        if !self.secret_access_active {
            return Err(SecretError::AccessNotActive);
        }

        let owner = self
            .current_execution_owner()
            .ok_or(SecretError::AccessNotActive)?;
        let path = SecretPath::parse(path).map_err(|_| SecretError::InvalidPath)?;

        self.secrets
            .read_for_component(owner, &path)
            .map(|value| value.expose_secret().to_string())
            .map_err(|error| match error {
                SecretAccessError::InvalidPath => SecretError::InvalidPath,
                SecretAccessError::AccessDenied => SecretError::AccessDenied,
                SecretAccessError::NotFound => SecretError::NotFound,
                SecretAccessError::Unavailable => SecretError::Unavailable,
            })
    }
}

impl ExecutionTargetsHost for WasmHostState {
    fn register_target(&mut self, target: String) -> Result<(), TargetRegistrationError> {
        if !self.execution_target_registration_active {
            return Err(TargetRegistrationError::RuntimeNotActive);
        }
        if rintawa_sdk::manifest::validate_component_target(&target).is_err()
            || target == rintawa_sdk::manifest::WASM_COMPONENT_TARGET_V1
        {
            return Err(TargetRegistrationError::InvalidTarget);
        }
        if self
            .pending_execution_targets
            .iter()
            .any(|existing| existing == &target)
        {
            return Err(TargetRegistrationError::TargetConflict);
        }
        self.pending_execution_targets.push(target);
        Ok(())
    }
}

impl HostArtifactSource for WasmHostState {
    fn paths(
        &mut self,
        source: Resource<crate::artifact_host::OwnedRtwComponentSource>,
    ) -> Vec<String> {
        match self.resource_table.get(&source) {
            Ok(source) => source
                .paths()
                .into_iter()
                .map(|path| path.to_string())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn resolve_component_entry(
        &mut self,
        source: Resource<crate::artifact_host::OwnedRtwComponentSource>,
        entry: String,
    ) -> Result<String, TargetArtifactError> {
        let source = self
            .resource_table
            .get(&source)
            .map_err(|_| TargetArtifactError::Unavailable)?;
        source
            .resolve_component_entry(&entry)
            .map(|path| path.to_string())
            .map_err(map_target_artifact_error)
    }

    fn resolve_relative(
        &mut self,
        source: Resource<crate::artifact_host::OwnedRtwComponentSource>,
        base_file: String,
        entry: String,
    ) -> Result<String, TargetArtifactError> {
        let base_file =
            rintawa_artifacts::ArtifactPath::parse(base_file).map_err(map_target_artifact_error)?;
        let source = self
            .resource_table
            .get(&source)
            .map_err(|_| TargetArtifactError::Unavailable)?;
        source
            .resolve_relative_to(&base_file, &entry)
            .map(|path| path.to_string())
            .map_err(map_target_artifact_error)
    }

    fn read(
        &mut self,
        source: Resource<crate::artifact_host::OwnedRtwComponentSource>,
        path: String,
    ) -> Result<Vec<u8>, TargetArtifactError> {
        let path =
            rintawa_artifacts::ArtifactPath::parse(path).map_err(map_target_artifact_error)?;
        let source = self
            .resource_table
            .get_mut(&source)
            .map_err(|_| TargetArtifactError::Unavailable)?;
        let maximum_bytes = u64::try_from(self.max_artifact_read_bytes).unwrap_or(u64::MAX);
        source
            .read_with_limit(&path, maximum_bytes)
            .map_err(map_target_artifact_error)
    }

    fn drop(
        &mut self,
        source: Resource<crate::artifact_host::OwnedRtwComponentSource>,
    ) -> wasmtime::Result<()> {
        Ok(self.resource_table.delete(source).map(|_| ())?)
    }
}

fn map_target_artifact_error(error: rintawa_artifacts::RtwError) -> TargetArtifactError {
    match error {
        rintawa_artifacts::RtwError::InvalidPath { .. } => TargetArtifactError::InvalidPath,
        rintawa_artifacts::RtwError::EntryNotFound(_) => TargetArtifactError::NotFound,
        _ => TargetArtifactError::Unavailable,
    }
}

impl PortableUiHost for WasmHostState {
    fn register_surface(
        &mut self,
        id: String,
        placement: WitPlacementHint,
        semantic_contract: Option<String>,
        semantic_version: Option<u32>,
        required_capabilities: Vec<String>,
    ) -> Result<(), PortableUiError> {
        self.validate_ui_registration_size(
            &id,
            placement,
            semantic_contract.as_deref(),
            semantic_version,
            &required_capabilities,
        )?;
        self.queue_ui_surface(
            id,
            placement,
            semantic_contract,
            semantic_version,
            required_capabilities,
        )
    }

    fn mount_surface(&mut self, snapshot_json: Vec<u8>) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(snapshot_json.len())?;
        let snapshot: UiSurfaceSnapshot =
            serde_json::from_slice(&snapshot_json).map_err(|error| {
                warn!(
                    plugin = %self.component_id,
                    error = %error,
                    "Rejecting invalid portable UI snapshot JSON"
                );
                PortableUiError::InvalidPayload
            })?;
        let owner = self.ui_owner()?;
        self.ui
            .mount_surface(&owner, snapshot)
            .map_err(Self::map_ui_error)
    }

    fn patch_surface(&mut self, batch_json: Vec<u8>) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(batch_json.len())?;
        let batch: UiPatchBatch = serde_json::from_slice(&batch_json).map_err(|error| {
            warn!(
                plugin = %self.component_id,
                error = %error,
                "Rejecting invalid portable UI patch JSON"
            );
            PortableUiError::InvalidPayload
        })?;
        let owner = self.ui_owner()?;
        self.ui
            .apply_patches(&owner, batch)
            .map_err(Self::map_ui_error)
    }

    fn unmount_surface(&mut self, surface_id: String) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(surface_id.len())?;
        let owner = self.ui_owner()?;
        self.ui
            .unmount_surface(&owner, &UiSurfaceId::new(surface_id))
            .map_err(Self::map_ui_error)
    }
}

impl HostOperations for WasmHostState {
    fn log(&mut self, level: LogLevel, message: String) {
        let id = &self.component_id;
        match level {
            LogLevel::Trace => trace!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Debug => debug!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Info => info!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Warn => warn!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Error => error!(target: "wasm_plugin", plugin = %id, "{message}"),
        }
    }

    fn publish_event(&mut self, _topic: String, _payload: Vec<u8>) -> Result<(), PublishError> {
        Err(PublishError::Unavailable)
    }
}

impl RuntimeEffectsHost for WasmHostState {
    fn subscribe_event(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        debug!(
            plugin = %self.component_id,
            topic = %topic,
            "WASM plugin subscribed to topic"
        );
        self.subscribe_event(topic)
    }

    fn unsubscribe_event(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        self.unsubscribe_event(handle)
    }
}

impl RuntimeTasksHost for WasmHostState {
    fn spawn_periodic(&mut self, interval_ms: u32) -> Result<u64, RuntimeTaskError> {
        if !self.task_access_active {
            return Err(RuntimeTaskError::RuntimeNotActive);
        }
        if !self.task_handler_available {
            return Err(RuntimeTaskError::Unavailable);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::BackgroundTask)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => RuntimeTaskError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => RuntimeTaskError::Unavailable,
            })?;
        if interval_ms < self.min_background_task_interval_ms || interval_ms == 0 {
            return Err(RuntimeTaskError::InvalidInterval);
        }
        let owned_count = self
            .active_tasks
            .values()
            .filter(|task| task.owner == owner)
            .count();
        if owned_count >= self.max_background_tasks {
            return Err(RuntimeTaskError::LimitExceeded);
        }
        let handle = self.next_task_handle;
        self.next_task_handle = self
            .next_task_handle
            .checked_add(1)
            .ok_or(RuntimeTaskError::Unavailable)?;
        let interval = Duration::from_millis(u64::from(interval_ms));
        self.active_tasks.insert(
            handle,
            ActiveWasmTask {
                owner,
                interval,
                next_due: Instant::now() + interval,
            },
        );
        Ok(handle)
    }

    fn cancel(&mut self, handle: u64) -> Result<(), RuntimeTaskError> {
        if !self.task_access_active {
            return Err(RuntimeTaskError::RuntimeNotActive);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::BackgroundTask)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => RuntimeTaskError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => RuntimeTaskError::Unavailable,
            })?;
        let Some(task) = self.active_tasks.get(&handle) else {
            return Err(RuntimeTaskError::UnknownTask);
        };
        if task.owner != owner {
            return Err(RuntimeTaskError::UnknownTask);
        }
        self.active_tasks.remove(&handle);
        Ok(())
    }
}

impl NetworkHost for WasmHostState {
    fn listen_loopback(&mut self, port: u16) -> Result<WitNetworkListener, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::LoopbackListen)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => NetworkError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => NetworkError::Unavailable,
            })?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let listener = TcpListener::bind(address).map_err(|_| NetworkError::Unavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Unavailable)?;
        let bound_port = listener
            .local_addr()
            .map_err(|_| NetworkError::Unavailable)?
            .port();
        let handle = self.allocate_network_handle(owner, NetworkHandleKind::Listener(listener))?;
        Ok(WitNetworkListener {
            handle,
            port: bound_port,
        })
    }

    fn connect_loopback(&mut self, port: u16) -> Result<u64, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        if port == 0 {
            return Err(NetworkError::InvalidPort);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::LoopbackConnect)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => NetworkError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => NetworkError::Unavailable,
            })?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let stream = TcpStream::connect_timeout(
            &address.into(),
            Duration::from_millis(self.loopback_connect_timeout_ms),
        )
        .map_err(|_| NetworkError::Unavailable)?;
        stream
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Unavailable)?;
        self.allocate_network_handle(owner, NetworkHandleKind::Stream(stream))
    }

    fn accept(&mut self, listener: u64) -> Result<Option<u64>, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .root_runtime_owner()
            .ok_or(NetworkError::PermissionDenied)?;
        if self
            .network_handles
            .values()
            .filter(|resource| resource.owner == owner)
            .count()
            >= self.max_network_handles
        {
            return Err(NetworkError::LimitExceeded);
        }
        let accepted = {
            let Some(resource) = self.network_handles.get(&listener) else {
                return Err(NetworkError::UnknownHandle);
            };
            if resource.owner != owner {
                return Err(NetworkError::UnknownHandle);
            }
            let NetworkHandleKind::Listener(listener) = &resource.kind else {
                return Err(NetworkError::WrongKind);
            };
            match listener.accept() {
                Ok((stream, _)) => Some(stream),
                Err(error) if error.kind() == ErrorKind::WouldBlock => None,
                Err(_) => return Err(NetworkError::Unavailable),
            }
        };
        let Some(stream) = accepted else {
            return Ok(None);
        };
        stream
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Unavailable)?;
        self.allocate_network_handle(owner, NetworkHandleKind::Stream(stream))
            .map(Some)
    }

    fn read(&mut self, socket: u64, max_bytes: u32) -> Result<WitNetworkReadResult, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .root_runtime_owner()
            .ok_or(NetworkError::PermissionDenied)?;
        let requested = usize::try_from(max_bytes).map_err(|_| NetworkError::MessageTooLarge)?;
        if requested > self.max_network_io_bytes {
            return Err(NetworkError::MessageTooLarge);
        }
        let Some(resource) = self.network_handles.get_mut(&socket) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        let NetworkHandleKind::Stream(stream) = &mut resource.kind else {
            return Err(NetworkError::WrongKind);
        };
        if requested == 0 {
            return Ok(WitNetworkReadResult {
                data: Vec::new(),
                eof: false,
            });
        }
        let mut data = vec![0_u8; requested];
        match stream.read(&mut data) {
            Ok(0) => Ok(WitNetworkReadResult {
                data: Vec::new(),
                eof: true,
            }),
            Ok(read) => {
                data.truncate(read);
                Ok(WitNetworkReadResult { data, eof: false })
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(NetworkError::WouldBlock),
            Err(_) => Err(NetworkError::Unavailable),
        }
    }

    fn write(&mut self, socket: u64, data: Vec<u8>) -> Result<u32, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .root_runtime_owner()
            .ok_or(NetworkError::PermissionDenied)?;
        if data.len() > self.max_network_io_bytes {
            return Err(NetworkError::MessageTooLarge);
        }
        let Some(resource) = self.network_handles.get_mut(&socket) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        let NetworkHandleKind::Stream(stream) = &mut resource.kind else {
            return Err(NetworkError::WrongKind);
        };
        match stream.write(&data) {
            Ok(written) => u32::try_from(written).map_err(|_| NetworkError::MessageTooLarge),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(NetworkError::WouldBlock),
            Err(_) => Err(NetworkError::Unavailable),
        }
    }

    fn close(&mut self, handle: u64) -> Result<(), NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .root_runtime_owner()
            .ok_or(NetworkError::PermissionDenied)?;
        let Some(resource) = self.network_handles.get(&handle) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        self.network_handles.remove(&handle);
        Ok(())
    }
}

impl SecretsHost for WasmHostState {
    fn read(&mut self, path: String) -> Result<String, SecretError> {
        self.read_secret(path)
    }
}

impl RegistrationHost for WasmHostState {
    fn define_contract(
        &mut self,
        name: String,
        version: u32,
        resolution: WitResolutionPolicy,
        protocol: WitContractProtocol,
    ) -> Result<(), RegistrationError> {
        let resolution = match resolution {
            WitResolutionPolicy::Single => ContractResolutionPolicy::Single,
            WitResolutionPolicy::Multiple => ContractResolutionPolicy::Multiple,
        };
        let protocol = match protocol {
            WitContractProtocol::Binding => ContractProtocol::Binding,
            WitContractProtocol::Service => ContractProtocol::Service,
        };
        self.queue_contract_definition(
            ContractKey::new(name, ContractVersion::new(version)),
            resolution,
            protocol,
        )
    }

    fn provide_contract(
        &mut self,
        name: String,
        version: u32,
        required_secret_read: Vec<String>,
    ) -> Result<(), RegistrationError> {
        self.queue_contract_provider(
            ContractKey::new(name, ContractVersion::new(version)),
            required_secret_read,
        )
    }

    fn consume_contract(
        &mut self,
        name: String,
        version: u32,
        required: bool,
        required_secret_read: Vec<String>,
    ) -> Result<(), RegistrationError> {
        self.queue_contract_consumer(
            ContractKey::new(name, ContractVersion::new(version)),
            required,
            required_secret_read,
        )
    }

    fn register_capability(
        &mut self,
        name: String,
        _schema: String,
    ) -> Result<(), RegistrationError> {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin registered capability"
        );
        self.queue_capability(name)
    }

    fn unregister_capability(&mut self, name: String) -> Result<(), RegistrationError> {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin unregistered capability"
        );
        self.remove_queued_capability(&name)
    }
}

impl ServicesHost for WasmHostState {
    fn call(
        &mut self,
        contract: String,
        version: u32,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, ServiceTransportError> {
        if !self.service_access_active {
            return Err(ServiceTransportError::Unavailable);
        }
        let caller = self
            .current_execution_owner()
            .cloned()
            .ok_or(ServiceTransportError::Unavailable)?;
        let contract = ContractKey::new(contract, ContractVersion::new(version));
        self.services
            .call_from_execution(&caller, &contract, &payload)
            .map_err(|error| match error {
                ServiceCallError::Unavailable => ServiceTransportError::Unavailable,
                ServiceCallError::NotConsumer => ServiceTransportError::NotConsumer,
                ServiceCallError::NotServiceContract => ServiceTransportError::NotServiceContract,
                ServiceCallError::UnsupportedResolution => {
                    ServiceTransportError::UnsupportedResolution
                }
                ServiceCallError::CyclicCall => ServiceTransportError::CyclicCall,
                ServiceCallError::ProviderBusy => ServiceTransportError::ProviderBusy,
                ServiceCallError::ProviderFailed => ServiceTransportError::ProviderFailed,
                ServiceCallError::RequestTooLarge => ServiceTransportError::RequestTooLarge,
                ServiceCallError::ResponseTooLarge => ServiceTransportError::ResponseTooLarge,
            })
    }
}

/// The engine manager for compiled WebAssembly components.
#[derive(Clone)]
pub struct WasmRuntimeEngine {
    engine: Engine,
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    execution_targets: ExecutionTargetRegistry,
    runtime_permissions: RuntimePermissionManager,
    budget: WasmExecutionBudget,
}

impl WasmRuntimeEngine {
    /// Creates a runtime using the system secret manager and default resource budget.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn new() -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(
            SecretManager::system(),
            WasmExecutionBudget::default(),
        )
    }

    /// Creates a runtime that shares the supplied Rintawa secret policy.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_secret_manager(secrets: SecretManager) -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(secrets, WasmExecutionBudget::default())
    }

    /// Creates a runtime using the system secret manager and the supplied budget.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_execution_budget(budget: WasmExecutionBudget) -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(SecretManager::system(), budget)
    }

    /// Creates a runtime with host-owned secret and resource policies.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_secret_manager_and_budget(
        secrets: SecretManager,
        budget: WasmExecutionBudget,
    ) -> EngineResult<Self> {
        let services = ServiceRuntime::new(secrets.clone());
        Self::with_host_services_and_budget(
            secrets,
            services,
            UiRuntime::new(),
            ExecutionTargetRegistry::default(),
            RuntimePermissionManager::default(),
            budget,
        )
    }

    pub(crate) fn with_host_services(
        secrets: SecretManager,
        services: ServiceRuntime,
        ui: UiRuntime,
        execution_targets: ExecutionTargetRegistry,
        runtime_permissions: RuntimePermissionManager,
    ) -> EngineResult<Self> {
        Self::with_host_services_and_budget(
            secrets,
            services,
            ui,
            execution_targets,
            runtime_permissions,
            WasmExecutionBudget::default(),
        )
    }

    fn with_host_services_and_budget(
        secrets: SecretManager,
        services: ServiceRuntime,
        ui: UiRuntime,
        execution_targets: ExecutionTargetRegistry,
        runtime_permissions: RuntimePermissionManager,
        budget: WasmExecutionBudget,
    ) -> EngineResult<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(false);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;

        Ok(Self {
            engine,
            secrets,
            services,
            ui,
            execution_targets,
            runtime_permissions,
            budget,
        })
    }

    /// Loads and compiles a WASM component from binary bytes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmArtifactTooLarge`] when `bytes` exceed the
    /// configured artifact budget, or [`EngineError::WasmRuntime`] if
    /// compilation or linker binding fails.
    pub fn load_component_from_bytes(
        &self,
        id: ComponentId,
        bytes: &[u8],
    ) -> EngineResult<WasmComponent> {
        self.ensure_component_size(&id, bytes.len())?;
        let component = WasmtimeComponent::new(&self.engine, bytes)?;
        let task_handler_available = component
            .component_type()
            .get_export(&self.engine, TASK_HANDLER_EXPORT_NAME)
            .is_some();
        let mut linker = Linker::new(&self.engine);

        Plugin::add_to_linker(&mut linker, |state: &mut WasmHostState| state)?;

        Ok(WasmComponent {
            id,
            engine: self.engine.clone(),
            component,
            linker: Arc::new(linker),
            secrets: self.secrets.clone(),
            services: self.services.clone(),
            ui: self.ui.clone(),
            execution_targets: self.execution_targets.clone(),
            runtime_permissions: self.runtime_permissions.clone(),
            task_handler_available,
            budget: self.budget.clone(),
            runtime: Arc::new(Mutex::new(WasmSharedRuntime {
                instance: None,
                failed_lifecycle_callback: None,
            })),
        })
    }

    /// Reads, bounds, and compiles a WASM component artifact from disk.
    ///
    /// The method intentionally reads no more than one byte above the configured
    /// limit, so a package artifact cannot make the loader allocate its full
    /// untrusted file size before the limit is checked.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmArtifactTooLarge`] when the artifact exceeds
    /// the configured byte limit, [`EngineError::Io`] when it cannot be read,
    /// or [`EngineError::WasmRuntime`] when Wasmtime cannot compile it.
    pub fn load_component_from_file(
        &self,
        id: ComponentId,
        path: &Path,
    ) -> EngineResult<WasmComponent> {
        let read_limit = u64::try_from(self.budget.max_component_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::new();
        File::open(path)?.take(read_limit).read_to_end(&mut bytes)?;
        self.load_component_from_bytes(id, &bytes)
    }

    fn ensure_component_size(&self, id: &ComponentId, observed_bytes: usize) -> EngineResult<()> {
        if observed_bytes > self.budget.max_component_bytes {
            return Err(EngineError::WasmArtifactTooLarge {
                component_id: id.as_str().to_string(),
                observed_bytes,
                maximum_bytes: self.budget.max_component_bytes,
            });
        }

        Ok(())
    }
}

/// A WASM component implementing the public SDK [`Component`] trait.
///
/// A successful `stop` keeps the guest instance warm for restart. If guest
/// `start` or `stop` execution fails, the instance is discarded and the
/// component must be reloaded before it can execute again.
pub struct WasmComponent {
    id: ComponentId,
    engine: Engine,
    component: WasmtimeComponent,
    linker: Arc<Linker<WasmHostState>>,
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    execution_targets: ExecutionTargetRegistry,
    runtime_permissions: RuntimePermissionManager,
    task_handler_available: bool,
    budget: WasmExecutionBudget,
    runtime: Arc<Mutex<WasmSharedRuntime>>,
}

impl WasmComponent {
    pub(crate) fn supports_execution_target_provider(&self) -> bool {
        self.component
            .component_type()
            .get_export(&self.engine, TARGET_PROVIDER_EXPORT_NAME)
            .is_some()
    }

    fn ensure_task_handler(instance: &mut WasmInstance) -> ExtensionResult<()> {
        if instance.task_handler.is_none() {
            let handler = TaskPlugin::new(&mut instance.store, &instance.instance).map_err(|error| {
                ExtensionError::Message(format!(
                    "WASM component scheduled a runtime task without task-handler exports: {error}"
                ))
            })?;
            instance.task_handler = Some(handler);
        }
        Ok(())
    }
}

struct WasmSharedRuntime {
    instance: Option<WasmInstance>,
    failed_lifecycle_callback: Option<&'static str>,
}

/// A live guest instance and its host state for one component lifecycle.
///
/// The store owns guest linear memory and globals, so it must live as long as
/// the guest instance. Re-instantiating per lifecycle callback would reset
/// guest state and invalidate component-owned runtime resources.
struct WasmInstance {
    store: Store<WasmHostState>,
    instance: Instance,
    plugin: Plugin,
    target_provider: Option<TargetProviderPlugin>,
    task_handler: Option<TaskPlugin>,
}

#[derive(Clone)]
struct WasmTargetProviderEndpoint {
    runtime: Arc<Mutex<WasmSharedRuntime>>,
    budget: WasmExecutionBudget,
}

struct WasmExecutionTargetHost {
    target: String,
    provider: WasmTargetProviderEndpoint,
}

struct WasmExecutionTargetProxy {
    id: ComponentId,
    handle: u64,
    provider: WasmTargetProviderEndpoint,
}

impl WasmTargetProviderEndpoint {
    fn runtime(&self) -> RtwComponentHostResult<MutexGuard<'_, WasmSharedRuntime>> {
        match self.runtime.try_lock() {
            Ok(runtime) => Ok(runtime),
            Err(TryLockError::WouldBlock) => Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime is busy",
            ))),
            Err(TryLockError::Poisoned(_)) => Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime lock was poisoned",
            ))),
        }
    }

    fn live_instance(runtime: &mut WasmSharedRuntime) -> RtwComponentHostResult<&mut WasmInstance> {
        if runtime.failed_lifecycle_callback.is_some() {
            return Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime is unavailable after lifecycle failure",
            )));
        }
        runtime.instance.as_mut().ok_or_else(|| {
            RtwComponentHostError::Host(String::from("WASM target-provider is not running"))
        })
    }

    fn ensure_target_provider(instance: &mut WasmInstance) -> ExtensionResult<()> {
        if instance.target_provider.is_none() {
            let provider = TargetProviderPlugin::new(&mut instance.store, &instance.instance)
                .map_err(|error| {
                    ExtensionError::Message(format!(
                        "WASM component published an execution target without target-provider exports: {error}"
                    ))
                })?;
            instance.target_provider = Some(provider);
        }
        Ok(())
    }

    fn load_component(
        &self,
        target: &str,
        source: &RtwComponentSource<'_>,
        descriptor: &rintawa_sdk::manifest::ComponentDescriptor,
    ) -> RtwComponentHostResult<u64> {
        let owned_source = source.fork_owned()?;
        let mut runtime = self.runtime()?;
        let instance = Self::live_instance(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target load")
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        Self::ensure_target_provider(instance)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        let source_resource = instance
            .store
            .data_mut()
            .resource_table
            .push(owned_source)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        let borrowed_source = Resource::new_borrow(source_resource.rep());
        let kind = match descriptor.kind {
            rintawa_sdk::manifest::ComponentKind::Runtime => String::from("runtime"),
            rintawa_sdk::manifest::ComponentKind::Ui => String::from("ui"),
        };
        let descriptor = WitTargetComponentDescriptor {
            id: descriptor.id.to_string(),
            kind,
            target: descriptor.target.to_string(),
            entry: descriptor.entry.clone(),
            required: descriptor.required,
        };
        let provider = instance.target_provider.as_ref().ok_or_else(|| {
            RtwComponentHostError::Host(String::from("target-provider export view is unavailable"))
        })?;
        let result = provider
            .rintawa_engine_target_provider()
            .call_load_component(&mut instance.store, target, &descriptor, borrowed_source)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()));
        let cleanup = instance
            .store
            .data_mut()
            .resource_table
            .delete(source_resource)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()));
        let provider_result = result?;
        cleanup?;
        provider_result.map_err(|error| {
            RtwComponentHostError::Host(format!(
                "WASM target-provider load callback failed: {error:?}"
            ))
        })
    }
}

fn map_wit_target_host_error(operation: &'static str, error: WitTargetError) -> ExtensionError {
    ExtensionError::Message(format!(
        "WASM target-provider `{operation}` callback failed: {error:?}"
    ))
}

impl WasmTargetProviderEndpoint {
    fn runtime_for_callback(&self) -> ExtensionResult<MutexGuard<'_, WasmSharedRuntime>> {
        match self.runtime.try_lock() {
            Ok(runtime) => Ok(runtime),
            Err(TryLockError::WouldBlock) => Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime is busy",
            ))),
            Err(TryLockError::Poisoned(_)) => Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime lock was poisoned",
            ))),
        }
    }

    fn live_instance_for_callback(
        runtime: &mut WasmSharedRuntime,
    ) -> ExtensionResult<&mut WasmInstance> {
        if runtime.failed_lifecycle_callback.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime is unavailable after lifecycle failure",
            )));
        }
        runtime.instance.as_mut().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM target-provider is not running"))
        })
    }

    fn register_component(&self, handle: u64) -> ExtensionResult<WasmRegistrations> {
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target register")?;
        instance
            .store
            .data_mut()
            .begin_target_component_registration()?;
        let result = instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_register_component(&mut instance.store, handle)
            .map_err(|error| WasmComponent::execution_error("target register", error));
        match result {
            Ok(Ok(())) => instance.store.data_mut().finish_registration(),
            Ok(Err(error)) => {
                instance
                    .store
                    .data_mut()
                    .cancel_target_component_registration();
                Err(map_wit_target_host_error("register", error))
            }
            Err(error) => {
                instance
                    .store
                    .data_mut()
                    .cancel_target_component_registration();
                Err(error)
            }
        }
    }

    fn start_component(&self, handle: u64, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target start")?;
        instance
            .store
            .data_mut()
            .begin_delegated_guest_execution(owner);
        let result = instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_start_component(&mut instance.store, handle)
            .map_err(|error| WasmComponent::execution_error("target start", error));
        match result {
            Ok(Ok(())) => instance.store.data_mut().finish_guest_execution(ctx),
            Ok(Err(error)) => {
                let error = map_wit_target_host_error("start", error);
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            }
            Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
        }
    }

    fn stop_component(&self, handle: u64, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target stop")?;
        let result = instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_stop_component(&mut instance.store, handle)
            .map_err(|error| WasmComponent::execution_error("target stop", error))?
            .map_err(|error| map_wit_target_host_error("stop", error));
        instance
            .store
            .data_mut()
            .forget_effect_handles_for_owner(&owner);
        instance
            .store
            .data_mut()
            .revoke_runtime_resources_for_owner(&owner);
        result
    }

    fn handle_ui_action(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        if payload.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target UI action",
                actual_bytes: payload.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target UI action")?;
        instance
            .store
            .data_mut()
            .begin_delegated_guest_execution(owner);
        let result = instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_handle_ui_action(&mut instance.store, handle, payload)
            .map_err(|error| WasmComponent::execution_error("target UI action", error));
        match result {
            Ok(Ok(())) => instance.store.data_mut().finish_guest_execution(ctx),
            Ok(Err(error)) => {
                let error = map_wit_target_host_error("UI action", error);
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            }
            Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
        }
    }

    fn handle_service(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        if request.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target service request",
                actual_bytes: request.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target service")?;
        instance
            .store
            .data_mut()
            .begin_delegated_service_execution(owner);
        let result = instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_handle_service(
                &mut instance.store,
                handle,
                contract.id.as_str(),
                contract.version.major(),
                request,
            )
            .map_err(|error| WasmComponent::execution_error("target service", error));
        instance.store.data_mut().finish_service_execution();
        let response = result?.map_err(|error| map_wit_target_host_error("service", error))?;
        if response.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target service response",
                actual_bytes: response.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        Ok(response)
    }

    fn drop_component(&self, handle: u64) -> ExtensionResult<()> {
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target drop")?;
        instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_drop_component(&mut instance.store, handle)
            .map_err(|error| WasmComponent::execution_error("target drop", error))?
            .map_err(|error| map_wit_target_host_error("drop", error))
    }
}

impl RtwComponentHost for WasmExecutionTargetHost {
    fn target(&self) -> &str {
        &self.target
    }

    fn load_component(
        &self,
        source: &mut RtwComponentSource<'_>,
        descriptor: &rintawa_sdk::manifest::ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        let handle = self
            .provider
            .load_component(&self.target, source, descriptor)?;
        Ok(Box::new(WasmExecutionTargetProxy {
            id: descriptor.id.clone(),
            handle,
            provider: self.provider.clone(),
        }))
    }
}

impl Component for WasmExecutionTargetProxy {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let registrations = self.provider.register_component(self.handle)?;
        apply_wasm_registrations(registrations, ctx)
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.provider.start_component(self.handle, ctx)
    }

    fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.provider.stop_component(self.handle, ctx)
    }

    fn handle_ui_action(
        &mut self,
        ctx: &mut dyn ComponentContext,
        event: &UiActionEvent,
    ) -> ExtensionResult<()> {
        let payload = serde_json::to_vec(event).map_err(|error| {
            ExtensionError::Message(format!("could not encode target UI action: {error}"))
        })?;
        self.provider.handle_ui_action(self.handle, ctx, &payload)
    }

    fn service_message_limit(&self) -> Option<usize> {
        Some(self.provider.budget.max_host_message_bytes)
    }

    fn handle_service(
        &mut self,
        ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        self.provider
            .handle_service(self.handle, ctx, contract, request)
    }
}

impl Drop for WasmExecutionTargetProxy {
    fn drop(&mut self) {
        if let Err(error) = self.provider.drop_component(self.handle) {
            warn!(component = %self.id, error = %error, "WASM target-provider drop callback failed");
        }
    }
}

fn apply_wasm_registrations(
    registrations: WasmRegistrations,
    ctx: &mut dyn RegistrationContext,
) -> ExtensionResult<()> {
    for contribution in registrations.contributions {
        ctx.register(contribution)?;
    }
    for definition in registrations.definitions {
        ctx.define_contract(definition)?;
    }
    for provider in registrations.providers {
        ctx.provide_contract(provider)?;
    }
    for consumer in registrations.consumers {
        ctx.consume_contract(consumer)?;
    }
    for surface in registrations.ui_surfaces {
        ctx.register_ui_surface(surface)?;
    }
    Ok(())
}

impl WasmComponent {
    fn runtime(&self) -> ExtensionResult<MutexGuard<'_, WasmSharedRuntime>> {
        match self.runtime.try_lock() {
            Ok(runtime) => Ok(runtime),
            Err(TryLockError::WouldBlock) => Err(ExtensionError::Message(String::from(
                "WASM runtime state is busy",
            ))),
            Err(TryLockError::Poisoned(_)) => Err(ExtensionError::Message(String::from(
                "WASM runtime state lock was poisoned",
            ))),
        }
    }

    /// Instantiates the guest component once and returns its live instance.
    ///
    /// # Errors
    ///
    /// Returns an error when Wasmtime cannot instantiate the component or when
    /// it does not satisfy the generated WIT world contract.
    fn ensure_instance<'a>(
        &self,
        runtime: &'a mut WasmSharedRuntime,
    ) -> ExtensionResult<&'a mut WasmInstance> {
        if let Some(operation) = runtime.failed_lifecycle_callback {
            return Err(ExtensionError::Message(format!(
                "WASM component instance was discarded after failed `{operation}` callback; reload required"
            )));
        }

        if runtime.instance.is_none() {
            let host_state = WasmHostState::with_host_services_and_budget(
                self.id.clone(),
                self.secrets.clone(),
                self.services.clone(),
                self.ui.clone(),
                self.runtime_permissions.clone(),
                self.task_handler_available,
                &self.budget,
            );
            let mut store = Store::new(&self.engine, host_state);
            store.limiter(|state| &mut state.resource_limits);
            Self::set_callback_fuel(&mut store, &self.budget, "instantiate")?;
            let instance = self
                .linker
                .instantiate(&mut store, &self.component)
                .map_err(|err| Self::execution_error("instantiate", err))?;
            let plugin = Plugin::new(&mut store, &instance)
                .map_err(|err| Self::execution_error("bind plugin exports", err))?;

            runtime.instance = Some(WasmInstance {
                store,
                instance,
                plugin,
                target_provider: None,
                task_handler: None,
            });
        }

        runtime.instance.as_mut().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM component instance was not initialized"))
        })
    }

    fn set_callback_fuel(
        store: &mut Store<WasmHostState>,
        budget: &WasmExecutionBudget,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        store.set_fuel(budget.fuel_per_callback).map_err(|error| {
            ExtensionError::Message(format!("could not set WASM fuel for {operation}: {error}"))
        })
    }

    fn execution_error(operation: &'static str, error: wasmtime::Error) -> ExtensionError {
        if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
            return ExtensionError::ExecutionBudgetExceeded {
                resource: "fuel",
                operation,
            };
        }

        ExtensionError::Message(format!("{operation} failed: {error}"))
    }

    fn validate_inbound_message(
        &self,
        operation: &'static str,
        message_bytes: usize,
    ) -> ExtensionResult<()> {
        if message_bytes > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation,
                actual_bytes: message_bytes,
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }

        Ok(())
    }

    /// Triggers an incoming event dispatch into the WASM guest instance.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::Message`] if WASM instantiation or execution fails.
    pub fn dispatch_event(
        &mut self,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        self.validate_inbound_message("event topic", topic.len())?;
        self.validate_inbound_message("event payload", payload.len())?;
        let budget = self.budget.clone();
        let mut runtime = self.runtime()?;
        let instance = self.ensure_instance(&mut runtime)?;

        instance.store.data().validate_execution_owner(ctx)?;
        Self::set_callback_fuel(&mut instance.store, &budget, "event dispatch")?;

        instance.store.data_mut().begin_guest_execution();

        let dispatch_result = instance
            .plugin
            .rintawa_engine_guest()
            .call_on_event(&mut instance.store, topic, payload)
            .map_err(|err| Self::execution_error("event dispatch", err));

        if let Err(error) = dispatch_result {
            return Err(instance.store.data_mut().abort_guest_execution(ctx, error));
        }

        if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
            return Err(instance.store.data_mut().abort_guest_execution(ctx, error));
        }

        Ok(())
    }
}

impl Component for WasmComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn service_message_limit(&self) -> Option<usize> {
        Some(self.budget.max_host_message_bytes)
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        let registrations = {
            let mut runtime = self.runtime()?;
            let instance = self.ensure_instance(&mut runtime)?;
            Self::set_callback_fuel(&mut instance.store, &budget, "register")?;
            instance.store.data_mut().begin_registration(
                ctx.extension_id().clone(),
                ctx.extension_instance_id().clone(),
                ctx.runtime_scope_id().clone(),
            )?;

            let registration_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_register(&mut instance.store);

            if let Err(err) = registration_result {
                instance.store.data_mut().cancel_registration();
                return Err(Self::execution_error("register", err));
            }

            instance.store.data_mut().finish_registration()?
        };

        apply_wasm_registrations(registrations, ctx)
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        let mut guest_failed = false;
        let mut pending_targets = Vec::new();
        let mut runtime = self.runtime()?;
        let result = {
            let instance = self.ensure_instance(&mut runtime)?;

            instance.store.data().validate_execution_owner(ctx)?;
            Self::set_callback_fuel(&mut instance.store, &budget, "start")?;

            instance.store.data_mut().begin_start_execution();

            let start_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_start(&mut instance.store)
                .map_err(|err| Self::execution_error("start", err));

            if let Err(error) = start_result {
                instance.store.data_mut().discard_guest_execution();
                guest_failed = true;
                Err(error)
            } else if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
                instance.store.data_mut().discard_guest_execution();
                Err(error)
            } else {
                pending_targets = instance.store.data_mut().take_pending_execution_targets();
                if pending_targets.is_empty() {
                    Ok(())
                } else {
                    WasmTargetProviderEndpoint::ensure_target_provider(instance)
                }
            }
        };

        if guest_failed {
            runtime.instance = None;
            runtime.failed_lifecycle_callback = Some("start");
        }
        drop(runtime);
        result?;

        if pending_targets.is_empty() {
            return Ok(());
        }

        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let provider = WasmTargetProviderEndpoint {
            runtime: self.runtime.clone(),
            budget: self.budget.clone(),
        };
        for target in pending_targets {
            let host = Arc::new(WasmExecutionTargetHost {
                target,
                provider: provider.clone(),
            });
            if let Err(error) = self.execution_targets.register(owner.clone(), host) {
                self.execution_targets.revoke_component(&owner);
                return Err(ExtensionError::Message(format!(
                    "could not publish execution target: {error}"
                )));
            }
        }
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        let mut runtime = self.runtime()?;
        let stop_result = if let Some(instance) = runtime.instance.as_mut() {
            let result =
                Self::set_callback_fuel(&mut instance.store, &budget, "stop").and_then(|()| {
                    instance
                        .plugin
                        .rintawa_engine_guest()
                        .call_stop(&mut instance.store)
                        .map_err(|err| Self::execution_error("stop", err))
                });
            let owner = instance.store.data().registered_owner();
            instance.store.data_mut().discard_guest_execution();
            instance.store.data_mut().effect_handles.clear();
            if let Some(owner) = owner {
                instance
                    .store
                    .data_mut()
                    .revoke_runtime_resources_for_owner(&owner);
            }
            result
        } else {
            return Ok(());
        };

        if let Err(error) = stop_result {
            runtime.instance = None;
            runtime.failed_lifecycle_callback = Some("stop");
            return Err(error);
        }

        Ok(())
    }

    fn poll_runtime(
        &mut self,
        ctx: &mut dyn ComponentContext,
    ) -> ExtensionResult<Option<Duration>> {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let budget = self.budget.clone();
        let mut guest_failed = false;
        let mut runtime = self.runtime()?;
        let result = {
            let instance = self.ensure_instance(&mut runtime)?;
            instance.store.data().validate_execution_owner(ctx)?;
            let now = Instant::now();
            let due = instance.store.data().due_task_handles(&owner, now);
            if due.is_empty() {
                Ok(instance.store.data().next_task_due_in(&owner, now))
            } else {
                Self::ensure_task_handler(instance)?;
                let mut failure = None;
                for handle in due {
                    if !instance.store.data().active_tasks.contains_key(&handle) {
                        continue;
                    }
                    Self::set_callback_fuel(&mut instance.store, &budget, "runtime task")?;
                    instance.store.data_mut().begin_guest_execution();
                    let callback = match instance.task_handler.as_ref() {
                        Some(handler) => handler
                            .rintawa_engine_task_handler()
                            .call_on_task(&mut instance.store, handle)
                            .map_err(|error| Self::execution_error("runtime task", error)),
                        None => Err(ExtensionError::Message(String::from(
                            "task-handler export view is unavailable",
                        ))),
                    };
                    if let Err(error) = callback {
                        failure = Some(instance.store.data_mut().abort_guest_execution(ctx, error));
                        break;
                    }
                    if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
                        failure = Some(instance.store.data_mut().abort_guest_execution(ctx, error));
                        break;
                    }
                    instance
                        .store
                        .data_mut()
                        .reschedule_task(handle, Instant::now());
                }
                if let Some(error) = failure {
                    guest_failed = true;
                    Err(error)
                } else {
                    let now = Instant::now();
                    Ok(instance.store.data().next_task_due_in(&owner, now))
                }
            }
        };
        if guest_failed {
            runtime.instance = None;
            runtime.failed_lifecycle_callback = Some("runtime task");
        }
        result
    }

    fn handle_ui_action(
        &mut self,
        ctx: &mut dyn ComponentContext,
        event: &UiActionEvent,
    ) -> ExtensionResult<()> {
        let payload = serde_json::to_vec(event).map_err(|error| {
            ExtensionError::Message(format!("could not encode UI action: {error}"))
        })?;
        self.validate_inbound_message("UI action", payload.len())?;
        let budget = self.budget.clone();
        let mut guest_failed = false;
        let mut runtime = self.runtime()?;
        let result = {
            let instance = self.ensure_instance(&mut runtime)?;
            instance.store.data().validate_execution_owner(ctx)?;
            Self::set_callback_fuel(&mut instance.store, &budget, "UI action")?;
            instance.store.data_mut().begin_guest_execution();

            let action_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_handle_ui_action(&mut instance.store, &payload)
                .map_err(|error| Self::execution_error("UI action", error));

            if let Err(error) = action_result {
                guest_failed = true;
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            } else if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
                guest_failed = true;
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            } else {
                Ok(())
            }
        };

        if guest_failed {
            runtime.instance = None;
            runtime.failed_lifecycle_callback = Some("UI action");
        }
        result
    }

    fn handle_service(
        &mut self,
        ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        self.validate_inbound_message("service contract", contract.id.as_str().len())?;
        self.validate_inbound_message("service request", request.len())?;
        let budget = self.budget.clone();
        let mut runtime = self.runtime()?;
        let instance = self.ensure_instance(&mut runtime)?;

        instance.store.data().validate_execution_owner(ctx)?;
        Self::set_callback_fuel(&mut instance.store, &budget, "service request")?;
        instance.store.data_mut().begin_service_execution();

        let response = instance
            .plugin
            .rintawa_engine_guest()
            .call_handle_service(
                &mut instance.store,
                contract.id.as_str(),
                contract.version.major(),
                request,
            )
            .map_err(|error| Self::execution_error("service request", error));

        instance.store.data_mut().finish_service_execution();
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemorySecretVault;
    use rintawa_sdk::{
        api::{LogLevel, LoggerApi},
        secrets::{SecretPath, SecretPathPattern, SecretValue},
        types::ExtensionId,
    };
    use std::sync::{Arc, Mutex};

    fn test_instance_id() -> ExtensionInstanceId {
        ExtensionInstanceId::new("test-instance")
    }

    fn test_scope_id() -> RuntimeScopeId {
        RuntimeScopeId::new("test")
    }

    fn begin_test_registration(state: &mut WasmHostState, extension_id: ExtensionId) {
        state
            .begin_registration(extension_id, test_instance_id(), test_scope_id())
            .unwrap();
    }

    struct TestLogger;

    impl LoggerApi for TestLogger {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    struct TestRuntimeContext {
        extension_id: ExtensionId,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
        component_id: ComponentId,
        logger: TestLogger,
        effects: HashMap<RuntimeEffectId, RuntimeEffect>,
        next_effect_sequence: u64,
        registration_attempts: u64,
        failing_registration_attempts: HashSet<u64>,
    }

    impl TestRuntimeContext {
        fn new() -> Self {
            Self {
                extension_id: ExtensionId::new("rintawa.chat"),
                instance_id: test_instance_id(),
                scope_id: test_scope_id(),
                component_id: ComponentId::new("chat-runtime"),
                logger: TestLogger,
                effects: HashMap::new(),
                next_effect_sequence: 0,
                registration_attempts: 0,
                failing_registration_attempts: HashSet::new(),
            }
        }

        fn fail_registration_on(&mut self, attempt: u64) {
            self.failing_registration_attempts.insert(attempt);
        }
    }

    impl ComponentContext for TestRuntimeContext {
        fn extension_id(&self) -> &ExtensionId {
            &self.extension_id
        }

        fn extension_instance_id(&self) -> &ExtensionInstanceId {
            &self.instance_id
        }

        fn runtime_scope_id(&self) -> &RuntimeScopeId {
            &self.scope_id
        }

        fn component_id(&self) -> &ComponentId {
            &self.component_id
        }

        fn logger(&self) -> &dyn LoggerApi {
            &self.logger
        }

        fn register_runtime_effect(
            &mut self,
            effect: RuntimeEffect,
        ) -> ExtensionResult<RuntimeEffectId> {
            self.registration_attempts += 1;
            if self
                .failing_registration_attempts
                .contains(&self.registration_attempts)
            {
                return Err(ExtensionError::Message(String::from(
                    "simulated effect registration failure",
                )));
            }
            let effect_id = RuntimeEffectId::new(format!("effect-{}", self.next_effect_sequence));
            self.next_effect_sequence += 1;
            self.effects.insert(effect_id.clone(), effect);
            Ok(effect_id)
        }

        fn revoke_runtime_effect(&mut self, effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
            self.effects.remove(effect_id);
            Ok(())
        }

        fn revoke_all_runtime_effects(&mut self) -> ExtensionResult<()> {
            self.effects.clear();
            Ok(())
        }
    }

    #[test]
    fn test_should_abort_failed_guest_execution_and_revoke_effects() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();
        let effect_id = RuntimeEffectId::new("effect-active");
        let effect = RuntimeEffect::event_subscription("dialogue.message");
        context.effects.insert(effect_id.clone(), effect.clone());
        state.effect_handles.insert(
            String::from("guest-handle"),
            ActiveWasmRuntimeEffect {
                owner: rintawa_sdk::contracts::ComponentRef::new(
                    test_instance_id(),
                    ComponentId::new("chat-runtime"),
                ),
                effect_id,
                effect,
            },
        );
        state.begin_guest_execution();

        let error = state.abort_guest_execution(
            &mut context,
            ExtensionError::Message(String::from("simulated callback failure")),
        );

        assert_eq!(error.to_string(), "simulated callback failure");
        assert!(context.effects.is_empty());
        assert!(state.effect_handles.is_empty());
        assert!(!state.runtime_effects_active);
        assert!(!state.secret_access_active);
        assert!(!state.service_access_active);
        assert!(!state.ui_access_active);
    }

    #[test]
    fn test_should_enforce_wasm_ui_message_limit_for_registration_and_string_ids() {
        let budget = WasmExecutionBudget {
            max_host_message_bytes: 4,
            ..WasmExecutionBudget::default()
        };
        let secrets = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let services = ServiceRuntime::new(secrets.clone());
        let mut state = WasmHostState::with_host_services_and_budget(
            ComponentId::new("runtime"),
            secrets,
            services,
            UiRuntime::new(),
            RuntimePermissionManager::default(),
            false,
            &budget,
        );
        begin_test_registration(&mut state, ExtensionId::new("example.extension"));

        assert!(matches!(
            PortableUiHost::register_surface(
                &mut state,
                String::from("12345"),
                WitPlacementHint::Primary,
                None,
                None,
                Vec::new(),
            ),
            Err(PortableUiError::MessageTooLarge)
        ));

        state.finish_registration().unwrap();
        state.begin_guest_execution();
        assert!(matches!(
            PortableUiHost::unmount_surface(&mut state, String::from("12345")),
            Err(PortableUiError::MessageTooLarge)
        ));
    }

    #[test]
    fn test_should_count_ui_registration_structure_overhead() {
        let budget = WasmExecutionBudget {
            max_host_message_bytes: 64,
            ..WasmExecutionBudget::default()
        };
        let secrets = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let services = ServiceRuntime::new(secrets.clone());
        let mut state = WasmHostState::with_host_services_and_budget(
            ComponentId::new("runtime"),
            secrets,
            services,
            UiRuntime::new(),
            RuntimePermissionManager::default(),
            false,
            &budget,
        );
        begin_test_registration(&mut state, ExtensionId::new("example.extension"));

        assert!(matches!(
            PortableUiHost::register_surface(
                &mut state,
                String::from("x"),
                WitPlacementHint::Primary,
                None,
                None,
                vec![String::new(); 32],
            ),
            Err(PortableUiError::MessageTooLarge)
        ));
    }

    #[test]
    fn test_should_report_event_publication_as_unavailable() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));

        assert!(matches!(
            HostOperations::publish_event(
                &mut state,
                String::from("dialogue.message"),
                b"payload".to_vec(),
            ),
            Err(PublishError::Unavailable)
        ));
    }

    #[test]
    fn test_should_commit_wasm_capability_contributions_only_after_registration_finishes() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));

        RegistrationHost::register_capability(
            &mut state,
            String::from("rintawa.ai"),
            String::from("{}"),
        )
        .unwrap();

        let registrations = state.finish_registration().unwrap();

        assert_eq!(registrations.contributions.len(), 1);
        assert_eq!(registrations.contributions[0].id.as_str(), "rintawa.ai");
        assert_eq!(
            registrations.contributions[0].kind,
            ContributionKind::capability()
        );
    }

    #[test]
    fn test_should_stage_wasm_contract_registrations() {
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        begin_test_registration(&mut state, ExtensionId::new("example.extension"));

        RegistrationHost::define_contract(
            &mut state,
            String::from("example.service"),
            1,
            WitResolutionPolicy::Single,
            WitContractProtocol::Service,
        )
        .unwrap();
        RegistrationHost::provide_contract(
            &mut state,
            String::from("example.service"),
            1,
            vec![String::from("ai.api_keys.*")],
        )
        .unwrap();
        RegistrationHost::consume_contract(
            &mut state,
            String::from("example.dependency"),
            1,
            true,
            Vec::new(),
        )
        .unwrap();

        let registrations = state.finish_registration().unwrap();
        assert_eq!(registrations.definitions.len(), 1);
        assert_eq!(registrations.providers.len(), 1);
        assert_eq!(registrations.consumers.len(), 1);
        assert_eq!(
            registrations.definitions[0].contract.to_string(),
            "example.service@1"
        );
        assert_eq!(
            registrations.definitions[0].resolution,
            ContractResolutionPolicy::Single
        );
        assert_eq!(registrations.providers[0].required_grants.len(), 1);
        assert!(registrations.consumers[0].required);
    }

    #[test]
    fn test_should_stage_and_apply_wasm_portable_ui_operations() {
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        let extension_id = ExtensionId::new("example.extension");
        begin_test_registration(&mut state, extension_id.clone());

        PortableUiHost::register_surface(
            &mut state,
            String::from("example.main"),
            WitPlacementHint::Primary,
            None,
            None,
            vec![String::from(rintawa_sdk::ui::UI_CAPABILITY_TEXT)],
        )
        .unwrap();
        let registrations = state.finish_registration().unwrap();
        assert_eq!(registrations.ui_surfaces.len(), 1);
        assert_eq!(registrations.ui_surfaces[0].id.as_str(), "example.main");

        let owner = rintawa_sdk::contracts::ComponentRef::new(
            test_instance_id(),
            ComponentId::new("runtime"),
        );
        state
            .ui
            .register_instance(
                test_instance_id(),
                test_scope_id(),
                vec![rintawa_ui_runtime::OwnedUiSurfaceContribution {
                    owner: owner.clone(),
                    contribution: registrations.ui_surfaces[0].clone(),
                }],
            )
            .unwrap();

        let snapshot = UiSurfaceSnapshot {
            surface_id: UiSurfaceId::new("example.main"),
            revision: 1,
            root: rintawa_sdk::ui::UiNodeId::new("root"),
            nodes: vec![rintawa_sdk::ui::UiNode::new(
                "root",
                rintawa_sdk::ui::UiNodeKind::Text(rintawa_sdk::ui::UiTextNode {
                    text: String::from("hello"),
                }),
            )],
        };
        state.begin_guest_execution();
        PortableUiHost::mount_surface(&mut state, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        state.finish_service_execution();
        assert_eq!(state.ui.presentation_surfaces().len(), 0);

        state
            .ui
            .set_instance_active(&test_instance_id(), true)
            .unwrap();
        assert_eq!(state.ui.presentation_surfaces().len(), 1);

        let patch = UiPatchBatch {
            surface_id: UiSurfaceId::new("example.main"),
            base_revision: 1,
            next_revision: 2,
            patches: vec![rintawa_sdk::ui::UiPatch::UpsertNode {
                node: rintawa_sdk::ui::UiNode::new(
                    "root",
                    rintawa_sdk::ui::UiNodeKind::Text(rintawa_sdk::ui::UiTextNode {
                        text: String::from("updated"),
                    }),
                ),
            }],
        };
        state.begin_guest_execution();
        PortableUiHost::patch_surface(&mut state, serde_json::to_vec(&patch).unwrap()).unwrap();
        PortableUiHost::unmount_surface(&mut state, String::from("example.main")).unwrap();
        state.finish_service_execution();
        assert!(state.ui.presentation_surfaces().is_empty());
    }

    #[test]
    fn test_should_reject_wasm_service_calls_outside_execution_scope() {
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        begin_test_registration(&mut state, ExtensionId::new("example.extension"));
        state.finish_registration().unwrap();

        assert!(matches!(
            ServicesHost::call(
                &mut state,
                String::from("example.service"),
                1,
                b"payload".to_vec(),
            ),
            Err(ServiceTransportError::Unavailable)
        ));
    }

    #[test]
    fn test_should_install_and_revoke_wasm_runtime_effects_with_handles() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        let _ = state
            .finish_registration()
            .expect("test registration should finish");
        let mut context = TestRuntimeContext::new();

        state.begin_guest_execution();
        let handle =
            RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message"))
                .unwrap();
        state.finish_guest_execution(&mut context).unwrap();
        assert_eq!(context.effects.len(), 1);

        state.begin_guest_execution();
        RuntimeEffectsHost::unsubscribe_event(&mut state, handle).unwrap();
        state.finish_guest_execution(&mut context).unwrap();
        assert!(context.effects.is_empty());
    }

    #[test]
    fn test_should_reject_wasm_effect_or_registration_outside_its_lifecycle_scope() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));

        assert!(matches!(
            RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message")),
            Err(RuntimeEffectError::RuntimeNotActive)
        ));
        assert!(matches!(
            RegistrationHost::register_capability(
                &mut state,
                String::from("rintawa.ai"),
                String::from("{}"),
            ),
            Err(RegistrationError::RegistrationNotActive)
        ));
    }

    #[test]
    fn test_should_read_only_host_granted_wasm_secret_during_and_after_execution() {
        let manager = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let extension_id = ExtensionId::new("official_ai");
        let component_id = ComponentId::new("provider");
        let allowed_path = SecretPath::parse("ai.api_keys.openai").unwrap();

        manager
            .store(&allowed_path, &SecretValue::new("test-key"))
            .unwrap();
        manager
            .grant_read(
                rintawa_sdk::contracts::ComponentRef::new(test_instance_id(), component_id.clone()),
                SecretPathPattern::parse("ai.api_keys.*").unwrap(),
            )
            .unwrap();

        let mut state = WasmHostState::with_secret_manager(component_id, manager);
        begin_test_registration(&mut state, extension_id);
        state.finish_registration().unwrap();

        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Err(SecretError::AccessNotActive)
        ));

        state.begin_guest_execution();
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Ok(value) if value == "test-key"
        ));
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys_backup.openai")),
            Err(SecretError::AccessDenied)
        ));

        let mut context = TestRuntimeContext::new();
        state.finish_guest_execution(&mut context).unwrap();
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Err(SecretError::AccessNotActive)
        ));
    }

    #[test]
    fn test_should_roll_back_effects_when_a_callback_batch_fails() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        let _ = state
            .finish_registration()
            .expect("test registration should finish");
        let mut context = TestRuntimeContext::new();
        context.fail_registration_on(2);

        state.begin_guest_execution();
        state
            .subscribe_event(String::from("dialogue.first"))
            .unwrap();
        state
            .subscribe_event(String::from("dialogue.second"))
            .unwrap();

        assert!(state.finish_guest_execution(&mut context).is_err());
        assert!(context.effects.is_empty());
    }

    #[test]
    fn test_should_report_a_failed_rollback_without_retaining_a_stale_handle() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        let _ = state
            .finish_registration()
            .expect("test registration should finish");
        let mut context = TestRuntimeContext::new();

        state.begin_guest_execution();
        let active_handle = state
            .subscribe_event(String::from("dialogue.active"))
            .unwrap();
        state.finish_guest_execution(&mut context).unwrap();

        context.fail_registration_on(2);
        context.fail_registration_on(3);
        state.begin_guest_execution();
        state.unsubscribe_event(active_handle.clone()).unwrap();
        state.subscribe_event(String::from("dialogue.new")).unwrap();

        assert!(matches!(
            state.finish_guest_execution(&mut context),
            Err(ExtensionError::RuntimeEffectRollbackFailed { .. })
        ));
        assert!(context.effects.is_empty());

        state.begin_guest_execution();
        assert!(matches!(
            state.unsubscribe_event(active_handle),
            Err(RuntimeEffectError::UnknownEffect)
        ));
    }

    #[test]
    fn test_should_reject_unversioned_execution_target_from_guest_start() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        let _ = state
            .finish_registration()
            .expect("test registration should finish");

        state.begin_start_execution();
        assert!(matches!(
            ExecutionTargetsHost::register_target(&mut state, String::from("runtime")),
            Err(TargetRegistrationError::InvalidTarget)
        ));
        assert!(state.pending_execution_targets.is_empty());
    }

    #[test]
    fn test_should_discard_pending_execution_targets_when_start_scope_aborts() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        let _ = state
            .finish_registration()
            .expect("test registration should finish");

        state.begin_start_execution();
        assert!(
            ExecutionTargetsHost::register_target(&mut state, String::from("example.runtime@1"))
                .is_ok()
        );
        assert_eq!(
            state.pending_execution_targets,
            vec![String::from("example.runtime@1")]
        );

        state.discard_guest_execution();
        assert!(state.pending_execution_targets.is_empty());
    }

    #[test]
    fn test_should_require_explicit_runtime_permission_for_background_tasks() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        state.finish_registration().unwrap();
        state.task_handler_available = true;
        state.begin_guest_execution();

        assert!(matches!(
            RuntimeTasksHost::spawn_periodic(&mut state, 10),
            Err(RuntimeTaskError::PermissionDenied)
        ));

        let owner = state.registered_owner().unwrap();
        state
            .runtime_permissions
            .grant(owner, RuntimePermission::BackgroundTask)
            .unwrap();
        let handle = RuntimeTasksHost::spawn_periodic(&mut state, 10).unwrap();
        assert!(state.active_tasks.contains_key(&handle));
        RuntimeTasksHost::cancel(&mut state, handle).unwrap();
        assert!(state.active_tasks.is_empty());
    }

    #[test]
    fn test_should_bound_background_task_count_and_interval() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
        state.finish_registration().unwrap();
        state.task_handler_available = true;
        state.max_background_tasks = 1;
        state.min_background_task_interval_ms = 25;
        let owner = state.registered_owner().unwrap();
        state
            .runtime_permissions
            .grant(owner, RuntimePermission::BackgroundTask)
            .unwrap();
        state.begin_guest_execution();

        assert!(matches!(
            RuntimeTasksHost::spawn_periodic(&mut state, 10),
            Err(RuntimeTaskError::InvalidInterval)
        ));
        let _ = RuntimeTasksHost::spawn_periodic(&mut state, 25).unwrap();
        assert!(matches!(
            RuntimeTasksHost::spawn_periodic(&mut state, 25),
            Err(RuntimeTaskError::LimitExceeded)
        ));
    }

    #[test]
    fn test_should_not_expose_task_or_network_capabilities_to_delegated_principal() {
        let mut state = WasmHostState::new(ComponentId::new("provider-runtime"));
        begin_test_registration(&mut state, ExtensionId::new("runtime.provider"));
        state.finish_registration().unwrap();
        state.task_handler_available = true;
        let root_owner = state.registered_owner().unwrap();
        state
            .runtime_permissions
            .grant(root_owner.clone(), RuntimePermission::BackgroundTask)
            .unwrap();
        state
            .runtime_permissions
            .grant(root_owner, RuntimePermission::LoopbackListen)
            .unwrap();

        state.task_access_active = true;
        state.network_access_active = true;
        state.begin_delegated_guest_execution(rintawa_sdk::contracts::ComponentRef::new(
            "dependent-instance",
            "hosted-component",
        ));
        assert!(matches!(
            RuntimeTasksHost::spawn_periodic(&mut state, 10),
            Err(RuntimeTaskError::RuntimeNotActive)
        ));
        assert!(matches!(
            NetworkHost::listen_loopback(&mut state, 0),
            Err(NetworkError::AccessNotActive)
        ));
    }

    #[test]
    fn test_should_exchange_bounded_bytes_through_owner_scoped_loopback_handles() {
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        begin_test_registration(&mut state, ExtensionId::new("runtime.provider"));
        state.finish_registration().unwrap();
        let owner = state.registered_owner().unwrap();
        state
            .runtime_permissions
            .grant(owner.clone(), RuntimePermission::LoopbackListen)
            .unwrap();
        state
            .runtime_permissions
            .grant(owner.clone(), RuntimePermission::LoopbackConnect)
            .unwrap();
        state.begin_guest_execution();

        let listener = NetworkHost::listen_loopback(&mut state, 0).unwrap();
        assert_ne!(listener.port, 0);
        let client = NetworkHost::connect_loopback(&mut state, listener.port).unwrap();
        let server = (0..50)
            .find_map(|_| match NetworkHost::accept(&mut state, listener.handle) {
                Ok(Some(handle)) => Some(handle),
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(1));
                    None
                }
                Err(error) => panic!("loopback accept failed: {error:?}"),
            })
            .expect("loopback connection should become acceptable");

        let empty_read = NetworkHost::read(&mut state, server, 0)
            .expect("zero-length read should validate the handle without consuming the stream");
        assert!(empty_read.data.is_empty());
        assert!(!empty_read.eof);

        assert!(matches!(
            NetworkHost::write(&mut state, client, b"ping".to_vec()),
            Ok(4)
        ));
        let received = (0..50)
            .find_map(|_| match NetworkHost::read(&mut state, server, 16) {
                Ok(result) if !result.data.is_empty() => Some(result),
                Ok(_) | Err(NetworkError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                    None
                }
                Err(error) => panic!("loopback read failed: {error:?}"),
            })
            .expect("loopback payload should become readable");
        assert_eq!(received.data, b"ping");
        assert!(!received.eof);

        state.max_network_io_bytes = 3;
        assert!(matches!(
            NetworkHost::write(&mut state, client, b"four".to_vec()),
            Err(NetworkError::MessageTooLarge)
        ));
        state.revoke_runtime_resources_for_owner(&owner);
        assert!(state.network_handles.is_empty());
    }

    #[test]
    fn test_should_reject_target_publication_without_provider_exports() {
        let runtime_engine = WasmRuntimeEngine::new().expect("test WASM runtime should initialize");
        let component = runtime_engine
            .load_component_from_bytes(
                ComponentId::new("ordinary-component"),
                include_str!("../../tests/fixtures/stateful_component.wat").as_bytes(),
            )
            .expect("ordinary test component should compile");
        let mut runtime = component
            .runtime()
            .expect("test runtime lock should be available");
        let instance = component
            .ensure_instance(&mut runtime)
            .expect("ordinary test component should instantiate");

        let error = WasmTargetProviderEndpoint::ensure_target_provider(instance)
            .expect_err("ordinary component must not satisfy target-provider exports");
        assert!(
            error
                .to_string()
                .contains("without target-provider exports")
        );
    }

    #[test]
    fn test_should_fail_fast_when_target_provider_runtime_is_reentered() {
        let runtime = Arc::new(Mutex::new(WasmSharedRuntime {
            instance: None,
            failed_lifecycle_callback: None,
        }));
        let endpoint = WasmTargetProviderEndpoint {
            runtime: runtime.clone(),
            budget: WasmExecutionBudget::default(),
        };
        let guard = runtime
            .lock()
            .expect("test runtime lock should be available");

        let error = match endpoint.runtime_for_callback() {
            Ok(_) => {
                panic!("reentrant target-provider callback must not block or acquire the lock")
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("runtime is busy"));
        drop(guard);
    }

    #[test]
    fn test_should_preserve_other_owner_effect_handles_after_delegated_abort() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            test_instance_id(),
            ComponentId::new("chat-runtime"),
        );
        let other_owner = rintawa_sdk::contracts::ComponentRef::new(
            test_instance_id(),
            ComponentId::new("other-runtime"),
        );
        state.effect_handles.insert(
            String::from("own"),
            ActiveWasmRuntimeEffect {
                owner: owner.clone(),
                effect_id: RuntimeEffectId::new("own-effect"),
                effect: RuntimeEffect::event_subscription("own.topic"),
            },
        );
        state.effect_handles.insert(
            String::from("other"),
            ActiveWasmRuntimeEffect {
                owner: other_owner,
                effect_id: RuntimeEffectId::new("other-effect"),
                effect: RuntimeEffect::event_subscription("other.topic"),
            },
        );
        let mut context = TestRuntimeContext::new();
        state.begin_delegated_guest_execution(owner);

        let error = state.abort_guest_execution(
            &mut context,
            ExtensionError::Message(String::from("simulated delegated failure")),
        );

        assert_eq!(error.to_string(), "simulated delegated failure");
        assert!(!state.effect_handles.contains_key("own"));
        assert!(state.effect_handles.contains_key("other"));
    }
}
