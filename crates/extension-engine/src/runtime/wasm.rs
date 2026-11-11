//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Rintawa WASM extensions.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contracts::{
        ContractConsumer, ContractDefinition, ContractGrantRequirement, ContractKey,
        ContractProvider, ContractResolutionPolicy, ContractVersion,
    },
    contributions::{ContributionDescriptor, ContributionKind, WorldSchemaContribution},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    runtime_permissions::RuntimePermission,
    secrets::{SecretAccessError, SecretPath, SecretPathPattern},
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeEffectId, RuntimeScopeId},
    world::{ItemId, SchemaKey, SchemaKind},
};
use rintawa_world::{WorldCommand, WorldTransaction};
use rintawa_world_runtime::{SystemError, SystemResult, WorldReadError, WorldSnapshot};
use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Engine, Store, StoreLimits, Trap,
    component::{Component as WasmtimeComponent, Instance, Linker, Resource, ResourceTable},
};
use zenoh::Wait;

use crate::{
    errors::{EngineError, EngineResult},
    execution_targets::ExecutionTargetRegistry,
    host_access::{HostAccessError, HostAccessServices},
    runtime_permissions::RuntimePermissionManager,
    secrets::SecretManager,
    transport::{MAX_PAYLOAD_BYTES, Transport, TransportError, validate_transport_point},
};

mod budget;
mod control_plane;
mod http_request;
mod network;
mod target_provider;

pub use crate::runtime::wasm::budget::WasmExecutionBudget;

use crate::runtime::wasm::http_request::OwnedHttpRequest;
use crate::runtime::wasm::network::OwnedNetworkHandle;
use crate::runtime::wasm::target_provider::{WasmExecutionTargetHost, WasmTargetProviderEndpoint};

#[cfg(test)]
use crate::runtime::wasm::network::{
    build_bounded_https_client, is_allowed_domain_destination_with_source, is_allowed_public_ip,
};

const TARGET_PROVIDER_EXPORT_NAME: &str = "rintawa:engine/target-provider@0.0.1";
const TASK_HANDLER_EXPORT_NAME: &str = "rintawa:engine/task-handler@0.0.1";
const TRANSPORT_HANDLER_EXPORT_NAME: &str = "rintawa:engine/transport-handler@0.0.1";
const WORLD_SYSTEM_EXPORT_NAME: &str = "rintawa:engine/world-system@0.0.1";

// Generated WIT bindings expose protocol names verbatim and are not hand-documented.
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

// Generated target-provider bindings are documented by the source WIT contract.
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

// Transport handler export is optional for ordinary WASM plugins.
#[allow(missing_docs)]
mod transport_bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "transport-handler-plugin",
        async: false,
    });
}

// Generated task bindings are documented by the source WIT contract.
#[allow(missing_docs)]
mod task_bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "task-plugin",
        async: false,
    });
}

// World System export is optional for ordinary WASM plugins.
#[allow(missing_docs)]
mod world_system_bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "world-system-plugin",
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
    http_request::{
        Error as HttpRequestError, Event as WitHttpEvent, Header as WitHttpHeader,
        Host as HttpRequestHost, Request as WitHttpRequest, ResponseHead as WitHttpResponseHead,
    },
    network::{
        Error as NetworkError, Host as NetworkHost, Listener as WitNetworkListener,
        ReadResult as WitNetworkReadResult,
    },
    preferences::{Error as PreferenceError, Host as PreferencesHost},
    registration::{
        Error as RegistrationError, Host as RegistrationHost,
        ResolutionPolicy as WitResolutionPolicy,
    },
    runtime_context::{
        Context as WitRuntimeContext, Error as RuntimeContextError, Host as RuntimeContextHost,
    },
    runtime_effects::{Error as RuntimeEffectError, Host as RuntimeEffectsHost},
    runtime_policy::{
        ArtifactPolicy as WitRuntimeArtifactPolicy, ComponentPolicy as WitRuntimePolicyComponent,
        ComponentRequest as WitRuntimePolicyRequest, Error as RuntimePolicyError,
        Host as RuntimePolicyHost, HttpOriginGrant as WitRuntimeHttpOriginGrant,
    },
    runtime_tasks::{Error as RuntimeTaskError, Host as RuntimeTasksHost},
    scoped_composition::Host as ScopedCompositionHost,
    scoped_runtime_policy::Host as ScopedRuntimePolicyHost,
    secrets::{Error as SecretError, Host as SecretsHost},
    transport::{Error as TransportApiError, Host as TransportHost},
    user_resources::{
        Error as UserResourceError, Host as UserResourcesHost, ResourceRef as WitUserResourceRef,
    },
    world_data::{Error as WorldDataError, Host as WorldDataHost, TicketStatus as WitTicketStatus},
    world_registration::{
        Error as WorldRegistrationError, Host as WorldRegistrationHost, SchemaKind as WitSchemaKind,
    },
    world_sessions::{
        Error as WorldSessionError, Host as WorldSessionsHost, Summary as WitWorldSummary,
    },
    world_system_snapshot::{Error as WorldSystemSnapshotError, Host as WorldSystemSnapshotHost},
};
use target_provider_bindings::TargetProviderPlugin;
use task_bindings::TaskPlugin;
use transport_bindings::TransportHandlerPlugin;
use world_system_bindings::WorldSystemPlugin;
use world_system_bindings::exports::rintawa::engine::world_system::Error as WorldSystemGuestError;

#[derive(Debug)]
struct PendingAssetUpload {
    expected_size: usize,
    media_type: String,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct PendingUserResourceUpload {
    expected_size: usize,
    media_type: String,
    name: Option<String>,
    bytes: Vec<u8>,
}

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
    /// True only while the dispatcher is executing a validated UI action.
    http_request_access_active: bool,
    next_network_handle: u64,
    network_handles: HashMap<u64, OwnedNetworkHandle>,
    http_requests: HashMap<u64, OwnedHttpRequest>,
    next_http_request_handle: u64,
    max_background_tasks: usize,
    min_background_task_interval_ms: u32,
    max_network_handles: usize,
    max_network_io_bytes: usize,
    loopback_connect_timeout_ms: u64,
    max_http_fetch_bytes: usize,
    http_fetch_timeout_ms: u64,
    max_http_redirects: usize,
    max_http_request_handles: usize,
    max_http_request_body_bytes: usize,
    max_http_request_headers: usize,
    max_http_request_header_bytes: usize,
    max_http_request_chunk_bytes: usize,
    secrets: SecretManager,
    transport: Transport,
    host_access: HostAccessServices,
    secret_access_active: bool,
    transport_access_active: bool,
    pending_transport_points: Vec<String>,
    transport_queries_this_execution: usize,
    transport_publications_this_execution: usize,
    max_host_message_bytes: usize,
    max_artifact_read_bytes: usize,
    max_artifact_import_bytes: usize,
    max_asset_import_bytes: usize,
    next_asset_upload_handle: u64,
    asset_uploads: HashMap<u64, PendingAssetUpload>,
    next_user_resource_upload_handle: u64,
    user_resource_uploads: HashMap<u64, PendingUserResourceUpload>,
    max_world_session_mutations_per_execution: usize,
    world_session_mutations_this_execution: usize,
    execution_target_registration_active: bool,
    pending_execution_targets: Vec<String>,
    world_system_snapshot: Option<WorldSnapshot>,
    world_data_access_active: bool,
    next_world_data_ticket: u64,
    world_data_tickets: HashMap<u64, rintawa_world_runtime::WorldCommandTicket>,
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
}

struct WasmRegistrations {
    contributions: Vec<ContributionDescriptor>,
    world_schemas: Vec<WorldSchemaContribution>,
    definitions: Vec<ContractDefinition>,
    providers: Vec<ContractProvider>,
    consumers: Vec<ContractConsumer>,
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
    transport: Transport,
    runtime_permissions: RuntimePermissionManager,
    host_access: HostAccessServices,
}

impl WasmHostServices {
    fn standalone(secrets: SecretManager) -> Self {
        Self {
            transport: Transport::default(),
            secrets,
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
            transport,
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
            http_request_access_active: false,
            next_network_handle: 0,
            network_handles: HashMap::new(),
            http_requests: HashMap::new(),
            next_http_request_handle: 0,
            max_background_tasks: budget.max_background_tasks,
            min_background_task_interval_ms: budget.min_background_task_interval_ms,
            max_network_handles: budget.max_network_handles,
            max_network_io_bytes: budget.max_network_io_bytes,
            loopback_connect_timeout_ms: budget.loopback_connect_timeout_ms,
            max_http_fetch_bytes: budget.max_http_fetch_bytes,
            http_fetch_timeout_ms: budget.http_fetch_timeout_ms,
            max_http_redirects: budget.max_http_redirects,
            max_http_request_handles: budget.max_http_request_handles,
            max_http_request_body_bytes: budget.max_http_request_body_bytes,
            max_http_request_headers: budget.max_http_request_headers,
            max_http_request_header_bytes: budget.max_http_request_header_bytes,
            max_http_request_chunk_bytes: budget.max_http_request_chunk_bytes,
            secrets,
            transport,
            host_access,
            secret_access_active: false,
            transport_access_active: false,
            pending_transport_points: Vec::new(),
            transport_queries_this_execution: 0,
            transport_publications_this_execution: 0,
            max_host_message_bytes: budget.max_host_message_bytes,
            max_artifact_read_bytes: budget.max_artifact_read_bytes,
            max_artifact_import_bytes: budget.max_artifact_import_bytes,
            max_asset_import_bytes: budget.max_asset_import_bytes,
            next_asset_upload_handle: 0,
            asset_uploads: HashMap::new(),
            next_user_resource_upload_handle: 0,
            user_resource_uploads: HashMap::new(),
            max_world_session_mutations_per_execution: budget
                .max_world_session_mutations_per_execution,
            world_session_mutations_this_execution: 0,
            execution_target_registration_active: false,
            pending_execution_targets: Vec::new(),
            world_system_snapshot: None,
            world_data_access_active: false,
            next_world_data_ticket: 0,
            world_data_tickets: HashMap::new(),
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
        self.http_request_access_active = false;
        self.secret_access_active = false;
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
        self.http_request_access_active = false;
        self.world_session_mutations_this_execution = 0;
        self.pending_effects.clear();
        self.pending_revocations.clear();
        self.secret_access_active = false;
        self.transport_access_active = false;
        self.execution_target_registration_active = false;
        self.pending_execution_targets.clear();
        self.pending_transport_points.clear();
        self.world_system_snapshot = None;
        self.world_data_access_active = false;
        self.world_data_tickets.clear();
    }

    fn begin_world_system_execution(&mut self, snapshot: WorldSnapshot) -> ExtensionResult<()> {
        if self.world_system_snapshot.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM World System execution is already active",
            )));
        }
        self.world_system_snapshot = Some(snapshot);
        Ok(())
    }

    fn finish_world_system_execution(&mut self) {
        self.world_system_snapshot = None;
    }

    fn world_system_snapshot(&self) -> Result<&WorldSnapshot, WorldSystemSnapshotError> {
        self.world_system_snapshot
            .as_ref()
            .ok_or(WorldSystemSnapshotError::AccessNotActive)
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

    fn subscribe_runtime_effect(
        &mut self,
        effect: RuntimeEffect,
    ) -> Result<String, RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }
        let RuntimeEffect::EventSubscription { topic } = &effect;
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
        self.world_data_access_active = true;
        self.http_request_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.transport_access_active = true;
        self.transport_queries_this_execution = 0;
        self.transport_publications_this_execution = 0;
        self.execution_target_registration_active = false;
    }

    fn begin_delegated_guest_execution(&mut self, owner: rintawa_sdk::contracts::ComponentRef) {
        self.execution_owner = Some(owner);
        self.runtime_effects_active = true;
        self.task_access_active = true;
        self.network_access_active = true;
        self.host_access_active = true;
        // This Store belongs to the target provider. A delegated component has
        // no independently attested extension identity at this boundary, so it
        // must not inherit the provider's World access.
        self.world_data_access_active = false;
        self.http_request_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.transport_access_active = true;
        self.transport_queries_this_execution = 0;
        self.transport_publications_this_execution = 0;
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
        // See begin_delegated_guest_execution: World access is never inherited
        // from the target provider by a delegated component.
        self.world_data_access_active = false;
        self.http_request_access_active = true;
        self.world_session_mutations_this_execution = 0;
        self.secret_access_active = true;
        self.transport_access_active = true;
        self.transport_queries_this_execution = 0;
        self.transport_publications_this_execution = 0;
        self.execution_target_registration_active = false;
    }

    fn finish_delegated_task_execution(&mut self) {
        self.discard_guest_execution();
    }

    fn begin_start_execution(&mut self) {
        self.begin_guest_execution();
        self.execution_target_registration_active = true;
        self.pending_execution_targets.clear();
        self.pending_transport_points.clear();
    }

    fn take_pending_execution_targets(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_execution_targets)
    }

    fn take_pending_transport_points(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_transport_points)
    }

    /// Query callbacks are not lifecycle callbacks and cannot commit
    /// component effects or borrow privileged host entrypoints.
    fn begin_transport_query_execution(&mut self) {
        self.discard_guest_execution();
        self.execution_owner = self.registered_owner();
        self.transport_access_active = true;
        self.world_data_access_active = true;
        self.transport_queries_this_execution = 0;
        self.transport_publications_this_execution = 0;
    }

    fn finish_transport_query_execution(&mut self) {
        self.discard_guest_execution();
    }

    /// Closes guest access before committing its queued runtime effects.
    fn finish_guest_execution(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.runtime_effects_active = false;
        self.task_access_active = false;
        self.network_access_active = false;
        self.host_access_active = false;
        self.world_data_access_active = false;
        self.http_request_access_active = false;
        self.secret_access_active = false;
        self.transport_access_active = false;
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
        self.http_requests
            .retain(|_, request| &request.owner != owner);
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

impl RuntimeContextHost for WasmHostState {
    fn current(&mut self) -> Result<WitRuntimeContext, RuntimeContextError> {
        self.current_execution_owner()
            .ok_or(RuntimeContextError::AccessNotActive)?;
        let scope_id = self
            .scope_id
            .as_ref()
            .cloned()
            .ok_or(RuntimeContextError::Rejected)?;
        let world_id = self
            .host_access
            .runtime_context
            .world_id_for_scope(&scope_id)
            .map_err(|error| match error {
                crate::host_access::HostAccessError::Unavailable => {
                    RuntimeContextError::Unavailable
                }
                _ => RuntimeContextError::Rejected,
            })?;
        Ok(WitRuntimeContext {
            scope_id: scope_id.to_string(),
            world_id: world_id.map(|world_id| world_id.to_string()),
        })
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
    ) -> Result<(), RegistrationError> {
        let resolution = match resolution {
            WitResolutionPolicy::Single => ContractResolutionPolicy::Single,
            WitResolutionPolicy::Multiple => ContractResolutionPolicy::Multiple,
        };
        self.queue_contract_definition(
            ContractKey::new(name, ContractVersion::new(version)),
            resolution,
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
            WitSchemaKind::Property => SchemaKind::Property,
            WitSchemaKind::Command => SchemaKind::Command,
        };
        self.queue_world_schema(WorldSchemaContribution::new(key, kind, definition_json))
    }
}

impl WorldSystemSnapshotHost for WasmHostState {
    fn load_item(&mut self, id: String) -> Result<Option<String>, WorldSystemSnapshotError> {
        let id = id
            .parse::<ItemId>()
            .map_err(|_| WorldSystemSnapshotError::InvalidRequest)?;
        let item = self
            .world_system_snapshot()?
            .load_item(id)
            .map_err(map_world_system_read_error)?;
        item.map(|item| self.encode_world_system_read(item))
            .transpose()
    }

    fn load_property(
        &mut self,
        item: String,
        key: String,
    ) -> Result<Option<String>, WorldSystemSnapshotError> {
        let item = item
            .parse::<ItemId>()
            .map_err(|_| WorldSystemSnapshotError::InvalidRequest)?;
        let key = key
            .parse::<SchemaKey>()
            .map_err(|_| WorldSystemSnapshotError::InvalidRequest)?;
        let property = self
            .world_system_snapshot()?
            .load_property(item, &key)
            .map_err(map_world_system_read_error)?;
        property
            .map(|property| self.encode_world_system_read(property))
            .transpose()
    }

    fn properties_of(&mut self, item: String) -> Result<Vec<String>, WorldSystemSnapshotError> {
        let item = item
            .parse::<ItemId>()
            .map_err(|_| WorldSystemSnapshotError::InvalidRequest)?;
        let mut total_bytes = 0_usize;
        self.world_system_snapshot()?
            .properties_of(item)
            .map_err(map_world_system_read_error)?
            .into_iter()
            .map(|property| {
                let encoded = self.encode_world_system_read(property)?;
                total_bytes = total_bytes.saturating_add(encoded.len());
                if total_bytes > self.max_host_message_bytes {
                    return Err(WorldSystemSnapshotError::MessageTooLarge);
                }
                Ok(encoded)
            })
            .collect()
    }
}

impl WasmHostState {
    fn encode_world_system_read<T: serde::Serialize>(
        &self,
        value: T,
    ) -> Result<String, WorldSystemSnapshotError> {
        let encoded =
            serde_json::to_string(&value).map_err(|_| WorldSystemSnapshotError::Unavailable)?;
        if encoded.len() > self.max_host_message_bytes {
            return Err(WorldSystemSnapshotError::MessageTooLarge);
        }
        Ok(encoded)
    }
}

fn map_world_system_read_error(error: WorldReadError) -> WorldSystemSnapshotError {
    match error {
        WorldReadError::PropertyAccessDenied(_) => WorldSystemSnapshotError::AccessDenied,
        WorldReadError::SnapshotUnavailable | WorldReadError::Storage(_) => {
            WorldSystemSnapshotError::Unavailable
        }
    }
}

impl WorldDataHost for WasmHostState {
    fn load_item(&mut self, id: String) -> Result<Option<String>, WorldDataError> {
        let (scope, owner) = self.world_data_identity()?;
        self.check_world_data_bytes(id.len())?;
        let item = id
            .parse::<ItemId>()
            .map_err(|_| WorldDataError::InvalidRequest)?;
        let value = self
            .host_access
            .world_data
            .load_item(scope, owner, item)
            .map_err(map_world_data_error)?;
        self.check_world_data_result(value.as_deref())?;
        Ok(value)
    }

    fn load_property(
        &mut self,
        item: String,
        key: String,
    ) -> Result<Option<String>, WorldDataError> {
        let (scope, owner) = self.world_data_identity()?;
        self.check_world_data_bytes(item.len().saturating_add(key.len()))?;
        let item = item
            .parse::<ItemId>()
            .map_err(|_| WorldDataError::InvalidRequest)?;
        let key = key
            .parse::<SchemaKey>()
            .map_err(|_| WorldDataError::InvalidRequest)?;
        let value = self
            .host_access
            .world_data
            .load_property(scope, owner, item, &key)
            .map_err(map_world_data_error)?;
        self.check_world_data_result(value.as_deref())?;
        Ok(value)
    }

    fn find_properties(
        &mut self,
        key: String,
        after: Option<String>,
        limit: u32,
    ) -> Result<Vec<String>, WorldDataError> {
        let (scope, owner) = self.world_data_identity()?;
        self.check_world_data_bytes(
            key.len()
                .saturating_add(after.as_ref().map_or(0, String::len)),
        )?;
        let key = key
            .parse::<SchemaKey>()
            .map_err(|_| WorldDataError::InvalidRequest)?;
        let after = after
            .map(|value| {
                value
                    .parse::<ItemId>()
                    .map_err(|_| WorldDataError::InvalidRequest)
            })
            .transpose()?;
        let values = self
            .host_access
            .world_data
            .find_properties(scope, owner, &key, after, limit as usize)
            .map_err(map_world_data_error)?;
        self.check_world_data_results(&values)?;
        Ok(values)
    }

    fn submit(&mut self, schema: String, payload_json: String) -> Result<u64, WorldDataError> {
        let (scope, _) = self.world_data_identity()?;
        let owner = self
            .current_execution_owner()
            .ok_or(WorldDataError::AccessNotActive)?;
        self.check_world_data_bytes(payload_json.len().saturating_add(schema.len()))?;
        if self.world_data_tickets.len() >= 16 {
            return Err(WorldDataError::QueueFull);
        }
        let schema = schema
            .parse::<SchemaKey>()
            .map_err(|_| WorldDataError::InvalidRequest)?;
        let payload =
            serde_json::from_str(&payload_json).map_err(|_| WorldDataError::InvalidRequest)?;
        let handle = self.next_world_data_ticket;
        let next_handle = handle.checked_add(1).ok_or(WorldDataError::Unavailable)?;
        let ticket = self
            .host_access
            .world_data
            .submit(scope, owner, schema, payload)
            .map_err(map_world_data_error)?;
        self.next_world_data_ticket = next_handle;
        self.world_data_tickets.insert(handle, ticket);
        Ok(handle)
    }

    fn poll(&mut self, ticket: u64) -> Result<WitTicketStatus, WorldDataError> {
        self.world_data_identity()?;
        let result = self
            .world_data_tickets
            .get(&ticket)
            .ok_or(WorldDataError::InvalidRequest)?
            .try_wait();
        match result {
            Ok(None) => Ok(WitTicketStatus::Pending),
            Ok(Some(_)) => {
                self.world_data_tickets.remove(&ticket);
                Ok(WitTicketStatus::Committed)
            }
            Err(error) => {
                self.world_data_tickets.remove(&ticket);
                Ok(WitTicketStatus::Failed(error.to_string()))
            }
        }
    }
}

impl WasmHostState {
    fn world_data_identity(&self) -> Result<(&RuntimeScopeId, &ExtensionId), WorldDataError> {
        if !self.world_data_access_active
            || self.world_system_snapshot.is_some()
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(WorldDataError::AccessNotActive);
        }
        let scope = self
            .scope_id
            .as_ref()
            .ok_or(WorldDataError::AccessNotActive)?;
        let owner = self
            .extension_id
            .as_ref()
            .ok_or(WorldDataError::AccessNotActive)?;
        Ok((scope, owner))
    }

    fn check_world_data_bytes(&self, bytes: usize) -> Result<(), WorldDataError> {
        if bytes > self.max_host_message_bytes {
            return Err(WorldDataError::MessageTooLarge);
        }
        Ok(())
    }

    fn check_world_data_result(&self, value: Option<&str>) -> Result<(), WorldDataError> {
        self.check_world_data_bytes(value.map_or(0, str::len))
    }

    fn check_world_data_results(&self, values: &[String]) -> Result<(), WorldDataError> {
        values.iter().try_fold(0_usize, |total, value| {
            let total = total.saturating_add(value.len());
            self.check_world_data_bytes(total)?;
            Ok(total)
        })?;
        Ok(())
    }
}

fn map_world_data_error(error: HostAccessError) -> WorldDataError {
    match error {
        HostAccessError::QueueFull => WorldDataError::QueueFull,
        HostAccessError::Rejected => WorldDataError::AccessDenied,
        HostAccessError::Unavailable => WorldDataError::Unavailable,
        _ => WorldDataError::InvalidRequest,
    }
}

impl TransportHost for WasmHostState {
    fn declare(&mut self, point: String) -> Result<(), TransportApiError> {
        // A provider target executing for a dependent principal must not
        // borrow this provider's World scope to publish a queryable.
        if !self.execution_target_registration_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        validate_transport_point(&point).map_err(map_transport_error)?;
        let scope = self
            .scope_id
            .as_ref()
            .ok_or(TransportApiError::AccessNotActive)?;
        let owner = self
            .registered_owner()
            .ok_or(TransportApiError::AccessNotActive)?;
        self.transport
            .check_declaration(&owner, scope, &point)
            .map_err(map_transport_error)?;
        if self
            .pending_transport_points
            .iter()
            .any(|name| name == &point)
        {
            return Err(TransportApiError::Duplicate);
        }
        if self.pending_transport_points.len() >= 16 {
            return Err(TransportApiError::LimitExceeded);
        }
        self.pending_transport_points.push(point);
        Ok(())
    }

    fn publish(&mut self, point: String, payload: Vec<u8>) -> Result<(), TransportApiError> {
        if !self.transport_access_active
            || self.execution_target_registration_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        if self.transport_publications_this_execution >= 32 {
            return Err(TransportApiError::LimitExceeded);
        }
        self.transport_publications_this_execution += 1;
        let scope = self
            .scope_id
            .as_ref()
            .ok_or(TransportApiError::AccessNotActive)?;
        let owner = self
            .registered_owner()
            .ok_or(TransportApiError::AccessNotActive)?;
        self.transport
            .publish(&owner, scope, &point, &payload)
            .map_err(map_transport_error)
    }

    fn subscribe(&mut self, point: String) -> Result<u64, TransportApiError> {
        if !self.transport_access_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        let scope = self
            .scope_id
            .as_ref()
            .ok_or(TransportApiError::AccessNotActive)?;
        let owner = self
            .registered_owner()
            .ok_or(TransportApiError::AccessNotActive)?;
        self.transport
            .subscribe(&owner, scope, &point)
            .map_err(map_transport_error)
    }

    fn poll(&mut self, handle: u64) -> Result<Vec<Vec<u8>>, TransportApiError> {
        if !self.transport_access_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        let owner = self
            .registered_owner()
            .ok_or(TransportApiError::AccessNotActive)?;
        self.transport
            .poll(&owner, handle)
            .map_err(map_transport_error)
    }

    fn unsubscribe(&mut self, handle: u64) -> Result<(), TransportApiError> {
        if !self.transport_access_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        let owner = self
            .registered_owner()
            .ok_or(TransportApiError::AccessNotActive)?;
        self.transport
            .unsubscribe(&owner, handle)
            .map_err(map_transport_error)
    }

    fn query(
        &mut self,
        point: String,
        payload: Vec<u8>,
    ) -> Result<Vec<Vec<u8>>, TransportApiError> {
        if !self.transport_access_active
            || self.current_execution_owner() != self.registered_owner().as_ref()
        {
            return Err(TransportApiError::AccessNotActive);
        }
        let scope_id = self
            .scope_id
            .clone()
            .ok_or(TransportApiError::AccessNotActive)?;
        // A callback may make nested calls, but cannot make unbounded
        // blocking host imports without consuming WASM fuel.
        if self.transport_queries_this_execution >= 8 {
            return Err(TransportApiError::LimitExceeded);
        }
        self.transport_queries_this_execution += 1;
        self.transport
            .query(&scope_id, &point, &payload)
            .map_err(map_transport_error)
    }
}

fn map_transport_error(error: TransportError) -> TransportApiError {
    match error {
        TransportError::Inactive => TransportApiError::AccessNotActive,
        TransportError::InvalidName => TransportApiError::InvalidName,
        TransportError::Duplicate => TransportApiError::Duplicate,
        TransportError::LimitExceeded => TransportApiError::LimitExceeded,
        TransportError::Unavailable => TransportApiError::Unavailable,
        TransportError::UnknownSubscription => TransportApiError::UnknownSubscription,
        TransportError::NotSelected => TransportApiError::NotSelected,
        TransportError::ActiveConflict => TransportApiError::Unavailable,
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
        transport: Transport,
        execution_targets: ExecutionTargetRegistry,
        runtime_permissions: RuntimePermissionManager,
        host_access: HostAccessServices,
    ) -> EngineResult<Self> {
        Self::with_host_services_and_budget(
            WasmHostServices {
                secrets,
                transport,
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
        let transport_handler_available = component
            .component_type()
            .get_export(&self.engine, TRANSPORT_HANDLER_EXPORT_NAME)
            .is_some();
        let world_system_handler_available = component
            .component_type()
            .get_export(&self.engine, WORLD_SYSTEM_EXPORT_NAME)
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
            transport_handler_available,
            world_system_handler_available,
            world_system_active: Arc::new(AtomicBool::new(false)),
            budget: self.budget.clone(),
            active: false,
            runtime: Arc::new(Mutex::new(WasmSharedRuntime {
                instance: None,
                failed_lifecycle_callback: None,
                active: false,
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
    transport_handler_available: bool,
    world_system_handler_available: bool,
    world_system_active: Arc<AtomicBool>,
    budget: WasmExecutionBudget,
    active: bool,
    runtime: Arc<Mutex<WasmSharedRuntime>>,
}

/// A live, host-mediated entry point for one active WASM World System.
///
/// The endpoint carries no World identity, principal, or mutation capability.
/// The Host supplies its owner-scoped pinned snapshot for each evaluation.
#[derive(Clone)]
pub struct WasmWorldSystemEndpoint {
    engine: Engine,
    component: WasmtimeComponent,
    linker: Arc<Linker<WasmHostState>>,
    host_services: WasmHostServices,
    budget: WasmExecutionBudget,
    active: Arc<AtomicBool>,
    runtime: Arc<Mutex<WasmSystemRuntime>>,
}

impl WasmComponent {
    pub(crate) fn supports_execution_target_provider(&self) -> bool {
        self.component
            .component_type()
            .get_export(&self.engine, TARGET_PROVIDER_EXPORT_NAME)
            .is_some()
    }

    /// Returns this component's authoritative World System entry point when it
    /// exports the dedicated callback interface.
    pub fn world_system_endpoint(&self) -> Option<WasmWorldSystemEndpoint> {
        self.world_system_handler_available
            .then(|| WasmWorldSystemEndpoint {
                engine: self.engine.clone(),
                component: self.component.clone(),
                linker: self.linker.clone(),
                host_services: self.host_services.clone(),
                budget: self.budget.clone(),
                active: self.world_system_active.clone(),
                runtime: Arc::new(Mutex::new(WasmSystemRuntime { instance: None })),
            })
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

    fn respond_to_transport_query(
        runtime: &Arc<Mutex<WasmSharedRuntime>>,
        budget: &WasmExecutionBudget,
        point: &str,
        query: zenoh::query::Query,
    ) {
        let result = (|| -> ExtensionResult<Vec<u8>> {
            let payload = query
                .payload()
                .map(|payload| payload.to_bytes().into_owned())
                .unwrap_or_default();
            if payload.len() > MAX_PAYLOAD_BYTES {
                return Err(ExtensionError::HostMessageTooLarge {
                    operation: "transport request",
                    actual_bytes: payload.len(),
                    maximum_bytes: MAX_PAYLOAD_BYTES,
                });
            }
            // A Store is never re-entered: busy callbacks fail instead of
            // deadlocking Zenoh or the extension lifecycle.
            let mut state = runtime.try_lock().map_err(|_| {
                ExtensionError::Message(String::from("WASM transport provider is busy"))
            })?;
            let instance = state.instance.as_mut().ok_or_else(|| {
                ExtensionError::Message(String::from("WASM transport provider is stopped"))
            })?;
            // Bound WASM query execution to the short Zenoh deadline.
            // The full service fuel allowance can leave a Store busy after timeout.
            Self::set_callback_fuel(&mut instance.store, budget, "transport query")?;
            instance.store.data_mut().begin_transport_query_execution();
            let response = instance
                .transport_handler
                .as_ref()
                .ok_or_else(|| {
                    ExtensionError::Message(String::from("WASM transport handler is unavailable"))
                })
                .and_then(|handler| {
                    handler
                        .rintawa_engine_transport_handler()
                        .call_on_query(&mut instance.store, point, &payload)
                        .map_err(|error| Self::execution_error("transport query", error))
                });
            instance.store.data_mut().finish_transport_query_execution();
            if response.is_err() {
                // A trapped Component Model instance cannot be re-entered.
                // Keep the Host running, but require an explicit component reload.
                state.instance = None;
                state.failed_lifecycle_callback = Some("transport query");
            }
            let response = response?;
            if response.len() > MAX_PAYLOAD_BYTES {
                return Err(ExtensionError::HostMessageTooLarge {
                    operation: "transport reply",
                    actual_bytes: response.len(),
                    maximum_bytes: MAX_PAYLOAD_BYTES,
                });
            }
            Ok(response)
        })();
        let reply = match result {
            Ok(bytes) => query.reply(query.key_expr().as_str(), bytes).wait(),
            Err(error) => {
                warn!(%error, %point, "WASM queryable failed to handle a request");
                query.reply_err(b"provider failed".to_vec()).wait()
            }
        };
        if let Err(error) = reply {
            debug!(%error, %point, "Zenoh request was no longer awaiting a reply");
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

impl WasmWorldSystemEndpoint {
    /// Evaluates one Host-validated command against the supplied pinned snapshot.
    ///
    /// The guest can read only through the snapshot imports enabled for this
    /// callback and can return only a deserializable WorldTransaction.
    pub fn evaluate(
        &self,
        snapshot: WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let command_json = serde_json::to_string(&serde_json::json!({
            "id": command.id().to_string(),
            "schema": command.schema().to_string(),
            "payload": command.payload(),
        }))
        .map_err(|error| SystemError::Failed(format!("could not encode World command: {error}")))?;
        if command_json.len() > self.budget.max_host_message_bytes {
            return Err(SystemError::Failed(String::from(
                "World command exceeds WASM message limit",
            )));
        }

        if !self.active.load(Ordering::Acquire) {
            return Err(SystemError::Failed(String::from(
                "WASM World System is not active",
            )));
        }
        let mut runtime = self.runtime.try_lock().map_err(|_| {
            SystemError::Failed(String::from("WASM World System is busy or unavailable"))
        })?;
        if runtime.instance.is_none() {
            let host_state = WasmHostState::with_host_services_and_budget(
                ComponentId::new("world-system"),
                self.host_services.clone(),
                false,
                &self.budget,
            );
            let mut store = Store::new(&self.engine, host_state);
            store.limiter(|state| &mut state.resource_limits);
            WasmComponent::set_callback_fuel(&mut store, &self.budget, "World System instantiate")
                .map_err(|error| SystemError::Failed(error.to_string()))?;
            let instance = self
                .linker
                .instantiate(&mut store, &self.component)
                .map_err(|error| {
                    SystemError::Failed(format!("WASM World System instantiate failed: {error}"))
                })?;
            let handler = WorldSystemPlugin::new(&mut store, &instance).map_err(|error| {
                SystemError::Failed(format!("WASM World System export is unavailable: {error}"))
            })?;
            runtime.instance = Some(WasmSystemInstance { store, handler });
        }
        let instance = runtime.instance.as_mut().ok_or_else(|| {
            SystemError::Failed(String::from("WASM World System instance is unavailable"))
        })?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "World System")
            .map_err(|error| SystemError::Failed(error.to_string()))?;
        instance
            .store
            .data_mut()
            .begin_world_system_execution(snapshot)
            .map_err(|error| SystemError::Failed(error.to_string()))?;

        let response = instance
            .handler
            .rintawa_engine_world_system()
            .call_evaluate(&mut instance.store, &command_json)
            .map_err(|error| SystemError::Failed(format!("WASM World System failed: {error}")));
        instance.store.data_mut().finish_world_system_execution();

        let response = match response {
            Ok(Ok(transaction_json)) => transaction_json,
            Ok(Err(WorldSystemGuestError::Rejected(reason))) => {
                return Err(SystemError::Rejected(reason));
            }
            Ok(Err(WorldSystemGuestError::Failed(reason))) => {
                return Err(SystemError::Failed(reason));
            }
            Err(error) => {
                runtime.instance = None;
                return Err(error);
            }
        };
        if response.len() > self.budget.max_host_message_bytes {
            return Err(SystemError::Failed(String::from(
                "WASM World System transaction exceeds message limit",
            )));
        }
        serde_json::from_str(&response).map_err(|error| {
            SystemError::Failed(format!("invalid WASM World transaction: {error}"))
        })
    }
}

struct WasmSharedRuntime {
    instance: Option<WasmInstance>,
    failed_lifecycle_callback: Option<&'static str>,
    active: bool,
}

struct WasmSystemRuntime {
    instance: Option<WasmSystemInstance>,
}

struct WasmSystemInstance {
    store: Store<WasmHostState>,
    handler: WorldSystemPlugin,
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
    transport_handler: Option<TransportHandlerPlugin>,
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
                transport_handler: None,
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

    fn set_runtime_task_fuel(
        store: &mut Store<WasmHostState>,
        budget: &WasmExecutionBudget,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        Self::set_fuel(store, budget.fuel_per_runtime_task, operation)
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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> &ComponentId {
        &self.id
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
        let mut pending_transport_points = Vec::new();
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
                pending_transport_points =
                    instance.store.data_mut().take_pending_transport_points();
                if !pending_transport_points.is_empty() && !self.transport_handler_available {
                    Err(ExtensionError::Message(String::from(
                        "WASM component declared queryables without transport-handler exports",
                    )))
                } else if !pending_transport_points.is_empty() {
                    match TransportHandlerPlugin::new(&mut instance.store, &instance.instance) {
                        Ok(handler) => {
                            instance.transport_handler = Some(handler);
                            if pending_targets.is_empty() {
                                Ok(())
                            } else {
                                WasmTargetProviderEndpoint::ensure_target_provider(instance)
                            }
                        }
                        Err(error) => Err(ExtensionError::Message(format!(
                            "could not bind WASM transport handler: {error}"
                        ))),
                    }
                } else if pending_targets.is_empty() {
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

        if !pending_transport_points.is_empty() {
            let owner = rintawa_sdk::contracts::ComponentRef::new(
                ctx.extension_instance_id().clone(),
                ctx.component_id().clone(),
            );
            for point in pending_transport_points {
                let callback_runtime = self.runtime.clone();
                let callback_budget = self.budget.clone();
                let callback_point = point.clone();
                self.host_services
                    .transport
                    .declare(&owner, ctx.runtime_scope_id(), &point, move |query| {
                        Self::respond_to_transport_query(
                            &callback_runtime,
                            &callback_budget,
                            &callback_point,
                            query,
                        );
                    })
                    .map_err(|error| {
                        ExtensionError::Message(format!(
                            "could not declare WASM transport point: {error}"
                        ))
                    })?;
            }
        }

        if pending_targets.is_empty() {
            self.active = true;
            self.world_system_active.store(true, Ordering::Release);
            self.runtime()?.active = true;
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
        self.world_system_active.store(true, Ordering::Release);
        self.runtime()?.active = true;
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.active = false;
        self.world_system_active.store(false, Ordering::Release);
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
        runtime.active = false;
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
                    Self::set_runtime_task_fuel(&mut instance.store, &budget, "runtime task")?;
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
}

#[cfg(test)]
mod tests;
