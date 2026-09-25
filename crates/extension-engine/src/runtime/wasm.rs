//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Rintawa WASM extensions.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
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
    contributions::{ContributionDescriptor, ContributionKind, WorldSchemaContribution},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    runtime_permissions::RuntimePermission,
    secrets::{SecretAccessError, SecretPath, SecretPathPattern},
    services::ServiceCallError,
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeEffectId, RuntimeScopeId},
    ui::{UiActionEvent, UiLayerDescriptor, UiSurfaceContribution, UiSurfaceId},
    world::{SchemaKey, SchemaKind},
};
use rintawa_ui_runtime::UiRuntime;
use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Engine, Store, StoreLimits, Trap,
    component::{Component as WasmtimeComponent, Instance, Linker, Resource, ResourceTable},
};

use crate::{
    errors::{EngineError, EngineResult},
    execution_targets::ExecutionTargetRegistry,
    host_access::HostAccessServices,
    runtime_permissions::RuntimePermissionManager,
    secrets::SecretManager,
    services::ServiceRuntime,
};

mod budget;
mod control_plane;
mod network;
mod target_provider;
mod ui;

pub use crate::runtime::wasm::budget::WasmExecutionBudget;

use crate::runtime::wasm::network::OwnedNetworkHandle;
use crate::runtime::wasm::target_provider::{WasmExecutionTargetHost, WasmTargetProviderEndpoint};

#[cfg(test)]
use crate::runtime::wasm::network::{
    build_bounded_https_client, is_allowed_domain_destination_with_source, is_allowed_public_ip,
};

const TARGET_PROVIDER_EXPORT_NAME: &str = "rintawa:engine/target-provider@0.0.1";
const TASK_HANDLER_EXPORT_NAME: &str = "rintawa:engine/task-handler@0.0.1";

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
    artifact_store::{
        Error as ArtifactStoreError, Host as ArtifactStoreHost,
        ImportedArtifact as WitImportedArtifact,
    },
    asset_store::{AssetRef as WitAssetRef, Error as AssetStoreError, Host as AssetStoreHost},
    composition::{
        Activation as WitCompositionActivation, Error as CompositionError, Host as CompositionHost,
    },
    execution_targets::{
        ArtifactError as TargetArtifactError, Host as ExecutionTargetsHost, HostArtifactSource,
        RegistrationError as TargetRegistrationError,
    },
    host::{Host as HostOperations, LogLevel, PublishError},
    http_fetch::{Error as HttpFetchError, Host as HttpFetchHost, Response as WitHttpResponse},
    network::{
        Error as NetworkError, Host as NetworkHost, Listener as WitNetworkListener,
        ReadResult as WitNetworkReadResult,
    },
    portable_ui::{
        Activity as WitActivity, Error as PortableUiError, Host as PortableUiHost,
        PlacementHint as WitPlacementHint,
    },
    preferences::{Error as PreferenceError, Host as PreferencesHost},
    registration::{
        ContractProtocol as WitContractProtocol, Error as RegistrationError,
        Host as RegistrationHost, ResolutionPolicy as WitResolutionPolicy,
    },
    runtime_effects::{Error as RuntimeEffectError, Host as RuntimeEffectsHost},
    runtime_policy::{
        ArtifactPolicy as WitRuntimeArtifactPolicy, ComponentPolicy as WitRuntimePolicyComponent,
        ComponentRequest as WitRuntimePolicyRequest, Error as RuntimePolicyError,
        Host as RuntimePolicyHost,
    },
    runtime_tasks::{Error as RuntimeTaskError, Host as RuntimeTasksHost},
    scoped_composition::Host as ScopedCompositionHost,
    scoped_runtime_policy::Host as ScopedRuntimePolicyHost,
    secrets::{Error as SecretError, Host as SecretsHost},
    services::{Error as ServiceTransportError, Host as ServicesHost},
    ui_layer::{Error as UiLayerError, Host as UiLayerHost},
    world_registration::{
        Error as WorldRegistrationError, Host as WorldRegistrationHost, SchemaKind as WitSchemaKind,
    },
    world_sessions::{
        Error as WorldSessionError, Host as WorldSessionsHost, Summary as WitWorldSummary,
    },
};
use target_provider_bindings::TargetProviderPlugin;
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
    host_access_active: bool,
    next_network_handle: u64,
    network_handles: HashMap<u64, OwnedNetworkHandle>,
    max_background_tasks: usize,
    min_background_task_interval_ms: u32,
    max_network_handles: usize,
    max_network_io_bytes: usize,
    loopback_connect_timeout_ms: u64,
    max_http_fetch_bytes: usize,
    http_fetch_timeout_ms: u64,
    max_http_redirects: usize,
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    host_access: HostAccessServices,
    secret_access_active: bool,
    service_access_active: bool,
    ui_access_active: bool,
    max_host_message_bytes: usize,
    max_artifact_read_bytes: usize,
    max_artifact_import_bytes: usize,
    max_asset_import_bytes: usize,
    max_world_session_mutations_per_execution: usize,
    world_session_mutations_this_execution: usize,
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
    world_schemas: Vec<WorldSchemaContribution>,
    world_schema_keys: HashSet<SchemaKey>,
    definitions: Vec<ContractDefinition>,
    definition_keys: HashSet<ContractKey>,
    providers: Vec<ContractProvider>,
    provider_keys: HashSet<ContractKey>,
    consumers: Vec<ContractConsumer>,
    consumer_keys: HashSet<ContractKey>,
    ui_surfaces: Vec<UiSurfaceContribution>,
    ui_surface_ids: HashSet<UiSurfaceId>,
    ui_layer: Option<UiLayerDescriptor>,
}

struct WasmRegistrations {
    contributions: Vec<ContributionDescriptor>,
    world_schemas: Vec<WorldSchemaContribution>,
    definitions: Vec<ContractDefinition>,
    providers: Vec<ContractProvider>,
    consumers: Vec<ContractConsumer>,
    ui_surfaces: Vec<UiSurfaceContribution>,
    ui_layer: Option<UiLayerDescriptor>,
}

/// A guest request that is committed through the Engine-owned effect registry.
enum WasmRuntimeEffectOperation {
    Subscribe {
        owner: rintawa_sdk::contracts::ComponentRef,
        handle: String,
        effect: RuntimeEffect,
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

#[derive(Debug, Clone, Copy)]
enum RuntimePermissionCheck {
    Denied,
    Unavailable,
}

#[derive(Clone)]
struct WasmHostServices {
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    runtime_permissions: RuntimePermissionManager,
    host_access: HostAccessServices,
}

impl WasmHostServices {
    fn standalone(secrets: SecretManager) -> Self {
        Self {
            services: ServiceRuntime::new(secrets.clone()),
            secrets,
            ui: UiRuntime::new(),
            runtime_permissions: RuntimePermissionManager::default(),
            host_access: HostAccessServices::unavailable(),
        }
    }
}

impl WasmHostState {
    #[cfg(test)]
    fn new(component_id: ComponentId) -> Self {
        Self::with_host_services_and_budget(
            component_id,
            WasmHostServices::standalone(SecretManager::system()),
            false,
            &WasmExecutionBudget::default(),
        )
    }

    #[cfg(test)]
    fn with_secret_manager(component_id: ComponentId, secrets: SecretManager) -> Self {
        Self::with_host_services_and_budget(
            component_id,
            WasmHostServices::standalone(secrets),
            false,
            &WasmExecutionBudget::default(),
        )
    }

    fn with_host_services_and_budget(
        component_id: ComponentId,
        host_services: WasmHostServices,
        task_handler_available: bool,
        budget: &WasmExecutionBudget,
    ) -> Self {
        let WasmHostServices {
            secrets,
            services,
            ui,
            runtime_permissions,
            host_access,
        } = host_services;
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
            host_access_active: false,
            next_network_handle: 0,
            network_handles: HashMap::new(),
            max_background_tasks: budget.max_background_tasks,
            min_background_task_interval_ms: budget.min_background_task_interval_ms,
            max_network_handles: budget.max_network_handles,
            max_network_io_bytes: budget.max_network_io_bytes,
            loopback_connect_timeout_ms: budget.loopback_connect_timeout_ms,
            max_http_fetch_bytes: budget.max_http_fetch_bytes,
            http_fetch_timeout_ms: budget.http_fetch_timeout_ms,
            max_http_redirects: budget.max_http_redirects,
            secrets,
            services,
            ui,
            host_access,
            secret_access_active: false,
            service_access_active: false,
            ui_access_active: false,
            max_host_message_bytes: budget.max_host_message_bytes,
            max_artifact_read_bytes: budget.max_artifact_read_bytes,
            max_artifact_import_bytes: budget.max_artifact_import_bytes,
            max_asset_import_bytes: budget.max_asset_import_bytes,
            max_world_session_mutations_per_execution: budget
                .max_world_session_mutations_per_execution,
            world_session_mutations_this_execution: 0,
            execution_target_registration_active: false,
            pending_execution_targets: Vec::new(),
            resource_limits: budget.store_limits(),
            resource_table: ResourceTable::new(),
        }
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
            world_schemas: Vec::new(),
            world_schema_keys: HashSet::new(),
            definitions: Vec::new(),
            definition_keys: HashSet::new(),
            providers: Vec::new(),
            provider_keys: HashSet::new(),
            consumers: Vec::new(),
            consumer_keys: HashSet::new(),
            ui_surfaces: Vec::new(),
            ui_surface_ids: HashSet::new(),
            ui_layer: None,
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
        self.host_access_active = false;
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
            world_schemas: Vec::new(),
            world_schema_keys: HashSet::new(),
            definitions: Vec::new(),
            definition_keys: HashSet::new(),
            providers: Vec::new(),
            provider_keys: HashSet::new(),
            consumers: Vec::new(),
            consumer_keys: HashSet::new(),
            ui_surfaces: Vec::new(),
            ui_surface_ids: HashSet::new(),
            ui_layer: None,
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
            world_schemas: scope.world_schemas,
            definitions: scope.definitions,
            providers: scope.providers,
            consumers: scope.consumers,
            ui_surfaces: scope.ui_surfaces,
            ui_layer: scope.ui_layer,
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
        self.host_access_active = false;
        self.world_session_mutations_this_execution = 0;
        self.pending_effects.clear();
        self.pending_revocations.clear();
        self.secret_access_active = false;
        self.service_access_active = false;
        self.ui_access_active = false;
        self.execution_target_registration_active = false;
        self.pending_execution_targets.clear();
    }

    fn queue_world_schema(
        &mut self,
        schema: WorldSchemaContribution,
    ) -> Result<(), WorldRegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(WorldRegistrationError::RegistrationNotActive);
        };
        if !scope.world_schema_keys.insert(schema.key().clone()) {
            return Err(WorldRegistrationError::DuplicateWorldSchema);
        }
        scope.world_schemas.push(schema);
        Ok(())
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
        self.subscribe_runtime_effect(RuntimeEffect::event_subscription(topic))
    }

    fn subscribe_signal(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        self.subscribe_runtime_effect(RuntimeEffect::signal_subscription(topic))
    }

    fn subscribe_runtime_effect(
        &mut self,
        effect: RuntimeEffect,
    ) -> Result<String, RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }
        let topic = match &effect {
            RuntimeEffect::EventSubscription { topic }
            | RuntimeEffect::SignalSubscription { topic } => topic,
        };
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
                effect,
            });
        Ok(handle)
    }

    fn unsubscribe_event(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        self.unsubscribe_runtime_effect(handle)
    }

    fn unsubscribe_signal(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        self.unsubscribe_runtime_effect(handle)
    }

    fn unsubscribe_runtime_effect(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
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
        self.host_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
        self.execution_target_registration_active = false;
    }

    fn begin_delegated_guest_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        self.runtime_effects_active = true;
        self.task_access_active = true;
        self.network_access_active = true;
        self.host_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
        self.execution_target_registration_active = false;
    }

    fn begin_delegated_task_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        // Runtime effects require an Engine ComponentContext for the exact owner.
        // The cooperative provider pump currently has only the provider context,
        // so delegated task callbacks cannot create or revoke runtime effects.
        self.runtime_effects_active = false;
        self.task_access_active = true;
        self.network_access_active = true;
        self.host_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
        self.execution_target_registration_active = false;
    }

    fn finish_delegated_task_execution(&mut self) {
        self.discard_guest_execution();
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
        self.host_access_active = false;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
    }

    fn begin_delegated_service_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.host_access_active = false;
        self.secret_access_active = true;
        self.service_access_active = true;
        self.ui_access_active = true;
    }

    fn finish_service_execution(&mut self) {
        self.execution_owner = None;
        self.task_access_active = false;
        self.network_access_active = false;
        self.host_access_active = false;
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
        self.host_access_active = false;
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
                    effect,
                } => ctx
                    .register_runtime_effect(effect.clone())
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
                    }),
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

    fn runtime_permission_owner(
        &self,
        permission: RuntimePermission,
    ) -> Result<rintawa_sdk::contracts::ComponentRef, RuntimePermissionCheck> {
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(RuntimePermissionCheck::Denied)?;
        match self.runtime_permissions.has_grant(&owner, permission) {
            Ok(true) => Ok(owner),
            Ok(false) => Err(RuntimePermissionCheck::Denied),
            Err(()) => Err(RuntimePermissionCheck::Unavailable),
        }
    }

    fn revoke_runtime_resources_for_owner(&mut self, owner: &rintawa_sdk::contracts::ComponentRef) {
        self.active_tasks.retain(|_, task| &task.owner != owner);
        self.network_handles
            .retain(|_, resource| &resource.owner != owner);
    }

    fn next_task_due_in(&self, now: Instant) -> Option<Duration> {
        self.active_tasks
            .values()
            .map(|task| task.next_due.saturating_duration_since(now))
            .min()
    }

    fn due_tasks(&self, now: Instant) -> Vec<(rintawa_sdk::contracts::ComponentRef, u64)> {
        let mut tasks: Vec<_> = self
            .active_tasks
            .iter()
            .filter(|(_, task)| task.next_due <= now)
            .map(|(handle, task)| (task.owner.clone(), *handle))
            .collect();
        tasks.sort_by(|left, right| {
            left.0
                .instance_id
                .as_str()
                .cmp(right.0.instance_id.as_str())
                .then_with(|| {
                    left.0
                        .component_id
                        .as_str()
                        .cmp(right.0.component_id.as_str())
                })
                .then_with(|| left.1.cmp(&right.1))
        });
        tasks
    }

    fn reschedule_task(&mut self, handle: u64, now: Instant) {
        if let Some(task) = self.active_tasks.get_mut(&handle) {
            task.next_due = now + task.interval;
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
    ) -> Result<Vec<String>, TargetArtifactError> {
        let source = self
            .resource_table
            .get(&source)
            .map_err(|_| TargetArtifactError::Unavailable)?;
        source
            .path_strings_with_limit(self.max_host_message_bytes)
            .ok_or(TargetArtifactError::MessageTooLarge)
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

    fn subscribe_signal(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        debug!(
            plugin = %self.component_id,
            topic = %topic,
            "WASM plugin subscribed to runtime signal"
        );
        self.subscribe_signal(topic)
    }

    fn unsubscribe_signal(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        self.unsubscribe_signal(handle)
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

impl WorldRegistrationHost for WasmHostState {
    fn register_world_schema(
        &mut self,
        id: String,
        version: u32,
        kind: WitSchemaKind,
        definition_json: String,
    ) -> Result<(), WorldRegistrationError> {
        if id.len().saturating_add(definition_json.len()) > self.max_host_message_bytes {
            return Err(WorldRegistrationError::MessageTooLarge);
        }
        let key = format!("{id}@{version}")
            .parse::<SchemaKey>()
            .map_err(|_| WorldRegistrationError::InvalidWorldSchema)?;
        let kind = match kind {
            WitSchemaKind::Entity => SchemaKind::Entity,
            WitSchemaKind::Relation => SchemaKind::Relation,
            WitSchemaKind::Facet => SchemaKind::Facet,
            WitSchemaKind::Command => SchemaKind::Command,
            WitSchemaKind::Event => SchemaKind::Event,
            WitSchemaKind::Effect => SchemaKind::Effect,
            WitSchemaKind::Projection => SchemaKind::Projection,
        };
        self.queue_world_schema(WorldSchemaContribution::new(key, kind, definition_json))
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
    host_services: WasmHostServices,
    execution_targets: ExecutionTargetRegistry,
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
        Self::with_host_services_and_budget(
            WasmHostServices::standalone(secrets),
            ExecutionTargetRegistry::default(),
            budget,
        )
    }

    pub(crate) fn with_host_services(
        secrets: SecretManager,
        services: ServiceRuntime,
        ui: UiRuntime,
        execution_targets: ExecutionTargetRegistry,
        runtime_permissions: RuntimePermissionManager,
        host_access: HostAccessServices,
    ) -> EngineResult<Self> {
        Self::with_host_services_and_budget(
            WasmHostServices {
                secrets,
                services,
                ui,
                runtime_permissions,
                host_access,
            },
            execution_targets,
            WasmExecutionBudget::default(),
        )
    }

    fn with_host_services_and_budget(
        host_services: WasmHostServices,
        execution_targets: ExecutionTargetRegistry,
        budget: WasmExecutionBudget,
    ) -> EngineResult<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(false);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;

        Ok(Self {
            engine,
            host_services,
            execution_targets,
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
            host_services: self.host_services.clone(),
            execution_targets: self.execution_targets.clone(),
            task_handler_available,
            budget: self.budget.clone(),
            active: false,
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
    host_services: WasmHostServices,
    execution_targets: ExecutionTargetRegistry,
    task_handler_available: bool,
    budget: WasmExecutionBudget,
    active: bool,
    runtime: Arc<Mutex<WasmSharedRuntime>>,
}

impl WasmComponent {
    pub(crate) fn supports_execution_target_provider(&self) -> bool {
        self.component
            .component_type()
            .get_export(&self.engine, TARGET_PROVIDER_EXPORT_NAME)
            .is_some()
    }

    fn ensure_active(&self, operation: &'static str) -> ExtensionResult<()> {
        if self.active {
            Ok(())
        } else {
            Err(ExtensionError::Message(format!(
                "WASM component is not active during {operation}"
            )))
        }
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

fn apply_wasm_registrations(
    registrations: WasmRegistrations,
    ctx: &mut dyn RegistrationContext,
) -> ExtensionResult<()> {
    for contribution in registrations.contributions {
        ctx.register(contribution)?;
    }
    for schema in registrations.world_schemas {
        ctx.register_world_schema(schema)?;
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
    if let Some(layer) = registrations.ui_layer {
        ctx.register_ui_layer(layer)?;
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
                self.host_services.clone(),
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

    fn set_fuel(
        store: &mut Store<WasmHostState>,
        fuel: u64,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        store.set_fuel(fuel).map_err(|error| {
            ExtensionError::Message(format!("could not set WASM fuel for {operation}: {error}"))
        })
    }

    fn set_callback_fuel(
        store: &mut Store<WasmHostState>,
        budget: &WasmExecutionBudget,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        Self::set_fuel(store, budget.fuel_per_callback, operation)
    }

    fn set_ui_action_fuel(
        store: &mut Store<WasmHostState>,
        budget: &WasmExecutionBudget,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        Self::set_fuel(store, budget.fuel_per_ui_action, operation)
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
        self.ensure_active("event dispatch")?;
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
        self.active = false;
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
            self.active = true;
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
        self.active = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.active = false;
        let budget = self.budget.clone();
        let (mut runtime, lock_failure) = match self.runtime.try_lock() {
            Ok(runtime) => (runtime, None),
            Err(TryLockError::WouldBlock) => {
                return Err(ExtensionError::Message(String::from(
                    "WASM runtime state is busy",
                )));
            }
            Err(TryLockError::Poisoned(poisoned)) => (
                poisoned.into_inner(),
                Some(ExtensionError::Message(String::from(
                    "WASM runtime state lock was poisoned",
                ))),
            ),
        };
        let stop_result = if let Some(instance) = runtime.instance.as_mut() {
            let result = if let Some(error) = lock_failure {
                Err(error)
            } else {
                Self::set_callback_fuel(&mut instance.store, &budget, "stop").and_then(|()| {
                    instance
                        .plugin
                        .rintawa_engine_guest()
                        .call_stop(&mut instance.store)
                        .map_err(|err| Self::execution_error("stop", err))
                })
            };
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
        } else if let Some(error) = lock_failure {
            Err(error)
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

    fn handle_event(
        &mut self,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        self.dispatch_event(ctx, topic, payload)
    }

    fn poll_runtime(
        &mut self,
        ctx: &mut dyn ComponentContext,
    ) -> ExtensionResult<Option<Duration>> {
        let root_owner = rintawa_sdk::contracts::ComponentRef::new(
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
            let due = instance.store.data().due_tasks(now);
            if due.is_empty() {
                Ok(instance.store.data().next_task_due_in(now))
            } else {
                Self::ensure_task_handler(instance)?;
                let mut failure = None;
                for (task_owner, handle) in due {
                    if !instance
                        .store
                        .data()
                        .active_tasks
                        .get(&handle)
                        .is_some_and(|task| task.owner == task_owner)
                    {
                        continue;
                    }
                    Self::set_callback_fuel(&mut instance.store, &budget, "runtime task")?;
                    let is_root_owner = task_owner == root_owner;
                    if is_root_owner {
                        instance.store.data_mut().begin_guest_execution();
                    } else {
                        instance
                            .store
                            .data_mut()
                            .begin_delegated_task_execution(task_owner.clone());
                    }
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
                        failure = Some(if is_root_owner {
                            guest_failed = true;
                            instance.store.data_mut().abort_guest_execution(ctx, error)
                        } else {
                            instance.store.data_mut().discard_guest_execution();
                            instance
                                .store
                                .data_mut()
                                .revoke_runtime_resources_for_owner(&task_owner);
                            error
                        });
                        break;
                    }
                    if is_root_owner {
                        if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
                            guest_failed = true;
                            failure =
                                Some(instance.store.data_mut().abort_guest_execution(ctx, error));
                            break;
                        }
                    } else {
                        instance.store.data_mut().finish_delegated_task_execution();
                    }
                    instance
                        .store
                        .data_mut()
                        .reschedule_task(handle, Instant::now());
                }
                if let Some(error) = failure {
                    Err(error)
                } else {
                    let now = Instant::now();
                    Ok(instance.store.data().next_task_due_in(now))
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
            Self::set_ui_action_fuel(&mut instance.store, &budget, "UI action")?;
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
            return result.map_err(|error| ExtensionError::ComponentRuntimeInvalidated {
                operation: "UI action",
                reason: error.to_string(),
            });
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
mod tests;
