//! Runtime orchestration for exact artifact activations in a local Rintawa host.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rintawa_artifacts::{ArtifactDigest, ContentType, RtwArchive, RtwLimits};
use rintawa_extension_engine::{
    AcceptedWorldCommand, ActivationPlanError, ArtifactStoreAccess, AssetStoreAccess,
    CompositionAccess, CompositionActivation, EngineError, ExtensionEngine, HostAccessError,
    HostAccessResult, ImportedArtifact, ImportedAsset, PreferenceAccess, RtwExtensionLoadOutcome,
    RtwExtensionLoader, RuntimeArtifactPolicy, RuntimePolicyAccess, RuntimePolicyComponent,
    RuntimePolicyRequest, UnresolvedContractReason, UserContentAccess, UserContentDocument,
    UserContentSummary, WorldCommandAccess, WorldCommandAccessError, WorldCommandAccessResult,
    WorldCommandActor, WorldCommandRequest, WorldSessionAccess, WorldSessionSummary,
};
use rintawa_sdk::{
    content::{
        ContentHandlerRequest, ContentHandlerResponse, MAX_CONTENT_HANDLER_DIAGNOSTIC_BYTES,
        MAX_CONTENT_HANDLER_ENTRY_BYTES, content_handler_service_contract_key,
    },
    contracts::{
        ComponentRef, ContractKey, ContractResolutionPolicy, host_shell_contract_key,
        ui_layer_contract_key,
    },
    runtime_permissions::RuntimePermission,
    runtime_signals::RuntimeSignal,
    types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId},
    world::{EntityId, PrincipalId, SchemaKey, UnixTimeMillis, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    ActorRef, CommitDisposition, SchemaDefinition, SchemaKind, StoredWorldEvent, WorldCommand,
};
use rintawa_world_runtime::{
    MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES, ServiceWorldEffectHandler, ServiceWorldProjection,
    ServiceWorldSystem, WorldCommandOutcome, WorldEffectServiceResponse, WorldProjectionView,
    WorldRuntime, WorldRuntimeBuilder, world_effect_service_contract_key,
    world_projection_service_contract_key, world_system_service_contract_key,
};

use tracing::warn;

use crate::{
    BootstrapBlockedActivation, BootstrapDeferredActivation, BootstrapRollback, BootstrapStall,
    CompositionProfile, HOST_SCOPE, HostCleanupFailure, HostCleanupOperation, HostError, HostHome,
    HostResult, HostShutdownFailures, UserContentEntry, UserContentId, WorldRuntimeCleanupFailure,
    WorldSummary, runtime_signal::RuntimeSignalQueue, world_runtime_scope_id,
};

/// Maximum durable effect jobs processed for one world in one cooperative pump.
pub const MAX_EFFECT_JOBS_PER_PUMP: usize = 8;
/// Lease duration assigned to one synchronous effect-handler attempt.
pub const EFFECT_JOB_LEASE_MILLIS: i64 = 30_000;
/// Initial host-owned retry delay after a failed effect attempt.
pub const EFFECT_RETRY_BASE_MILLIS: i64 = 1_000;
/// Maximum host-owned retry delay regardless of attempt count.
pub const EFFECT_RETRY_MAX_MILLIS: i64 = 60_000;
/// Maximum distinct world lifecycle requests accepted before a host pump drains them.
const MAX_PENDING_WORLD_SESSION_REQUESTS: usize = 64;
/// Maximum deferred authoritative commands accepted before a host pump drains them.
const MAX_PENDING_WORLD_COMMAND_REQUESTS: usize = 128;
/// Maximum diagnostic bytes retained for one failed deferred world lifecycle request.
const MAX_WORLD_SESSION_DIAGNOSTIC_BYTES: usize = 2 * 1024;

#[derive(Default)]
struct WorldSessionRuntimeState {
    active: HashSet<WorldId>,
    pending: BTreeMap<WorldId, bool>,
    last_errors: BTreeMap<WorldId, String>,
}

#[derive(Clone, Default)]
struct WorldSessionRuntimeControl {
    state: Arc<Mutex<WorldSessionRuntimeState>>,
}

impl WorldSessionRuntimeControl {
    fn summary(&self, world: WorldSummary) -> HostAccessResult<WorldSessionSummary> {
        let state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        Ok(WorldSessionSummary {
            world_id: world.id.to_string(),
            commit_position: world.commit_position,
            active: state.active.contains(&world.id),
            pending_active: state.pending.get(&world.id).copied(),
            last_error: state.last_errors.get(&world.id).cloned(),
        })
    }

    fn request(&self, world_id: WorldId, active: bool) -> HostAccessResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        if !state.pending.contains_key(&world_id)
            && state.pending.len() >= MAX_PENDING_WORLD_SESSION_REQUESTS
        {
            return Err(HostAccessError::QueueFull);
        }
        state.pending.insert(world_id, active);
        state.last_errors.remove(&world_id);
        Ok(())
    }

    fn accepts_commands(&self, world_id: WorldId) -> HostAccessResult<bool> {
        let state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        Ok(match state.pending.get(&world_id) {
            Some(true) => true,
            Some(false) => false,
            None => state.active.contains(&world_id),
        })
    }

    fn drain_pending(&self) -> HostResult<Vec<(WorldId, bool)>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostError::WorldSessionControlUnavailable)?;
        Ok(std::mem::take(&mut state.pending).into_iter().collect())
    }

    fn record_outcome(
        &self,
        world_id: WorldId,
        actual_active: bool,
        diagnostic: Option<&str>,
    ) -> HostResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostError::WorldSessionControlUnavailable)?;
        // `drain_pending` removed the request being completed. Never remove a
        // newer request for this world that may have arrived while the lifecycle
        // transition was running.
        if actual_active {
            state.active.insert(world_id);
        } else {
            state.active.remove(&world_id);
        }
        if let Some(diagnostic) = diagnostic {
            state
                .last_errors
                .insert(world_id, bounded_world_session_diagnostic(diagnostic));
        } else {
            state.last_errors.remove(&world_id);
        }
        Ok(())
    }
}

struct DeferredWorldCommand {
    world_id: WorldId,
    command: WorldCommand,
}

#[derive(Clone, Default)]
struct WorldCommandRuntimeControl {
    queue: Arc<Mutex<VecDeque<DeferredWorldCommand>>>,
}

impl WorldCommandRuntimeControl {
    fn request(
        &self,
        world_id: WorldId,
        command: WorldCommand,
    ) -> WorldCommandAccessResult<AcceptedWorldCommand> {
        let command_id = command.id().to_string();
        let correlation_id = command.correlation_id().to_string();
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| WorldCommandAccessError::Unavailable)?;
        if queue.len() >= MAX_PENDING_WORLD_COMMAND_REQUESTS {
            return Err(WorldCommandAccessError::QueueFull);
        }
        queue.push_back(DeferredWorldCommand { world_id, command });
        Ok(AcceptedWorldCommand {
            command_id,
            correlation_id,
        })
    }

    fn drain_pending(&self) -> HostResult<Vec<DeferredWorldCommand>> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| HostError::WorldCommandControlUnavailable)?;
        Ok(queue.drain(..).collect())
    }
}

struct LocalHostAccess {
    home_root: PathBuf,
    local_principal: PrincipalId,
    world_sessions: WorldSessionRuntimeControl,
    world_commands: WorldCommandRuntimeControl,
}

impl LocalHostAccess {
    fn new(
        home_root: &Path,
        local_principal: PrincipalId,
        world_sessions: WorldSessionRuntimeControl,
        world_commands: WorldCommandRuntimeControl,
    ) -> Self {
        Self {
            home_root: home_root.to_path_buf(),
            local_principal,
            world_sessions,
            world_commands,
        }
    }

    fn home(&self) -> HostAccessResult<HostHome> {
        HostHome::open(&self.home_root).map_err(map_host_access_error)
    }
}

impl ArtifactStoreAccess for LocalHostAccess {
    fn import_rtw(&self, bytes: &[u8]) -> HostAccessResult<ImportedArtifact> {
        let home = self.home()?;
        let imported = home
            .import_rtw_bytes(bytes)
            .map_err(map_artifact_import_error)?;
        let archive = home
            .artifact_store()
            .open_artifact(imported.digest())
            .map_err(|_| HostAccessError::InvalidArtifact)?;
        Ok(ImportedArtifact {
            digest: imported.digest().to_string(),
            content: archive.manifest().content.to_string(),
        })
    }
}

impl AssetStoreAccess for LocalHostAccess {
    fn import_asset(&self, bytes: &[u8], media_type: &str) -> HostAccessResult<ImportedAsset> {
        let home = self.home()?;
        let imported = home
            .asset_store()
            .import_bytes(bytes, media_type.to_string())
            .map_err(map_asset_import_error)?;
        let reference = imported.asset_ref();
        Ok(ImportedAsset {
            digest: reference.digest.to_string(),
            size: reference.size,
            media_type: reference.media_type.to_string(),
        })
    }
}

impl UserContentAccess for LocalHostAccess {
    fn list_user_content(
        &self,
        content: Option<&str>,
    ) -> HostAccessResult<Vec<UserContentSummary>> {
        let content = content
            .map(ContentType::parse)
            .transpose()
            .map_err(|_| HostAccessError::InvalidContentType)?;
        Ok(self
            .home()?
            .list_user_content()
            .map_err(map_host_access_error)?
            .into_iter()
            .filter(|entry| {
                content
                    .as_ref()
                    .is_none_or(|content| &entry.content == content)
            })
            .map(to_user_content_summary)
            .collect())
    }

    fn read_user_content(&self, id: &str) -> HostAccessResult<UserContentDocument> {
        let id = id
            .parse::<UserContentId>()
            .map_err(|_| HostAccessError::InvalidUserContentId)?;
        let home = self.home()?;
        let entry = home
            .indexed_user_content(id)
            .map_err(map_host_access_error)?;
        let mut archive = home
            .artifact_store()
            .open_artifact(&entry.revision)
            .map_err(|_| HostAccessError::Unavailable)?;
        if archive.manifest().content != entry.content {
            return Err(HostAccessError::Rejected);
        }
        let entry_path = archive.manifest().entry.clone();
        let maximum_entry_bytes = u64::try_from(MAX_CONTENT_HANDLER_ENTRY_BYTES)
            .map_err(|_| HostAccessError::Rejected)?;
        let descriptor = archive
            .read_with_limit(&entry_path, maximum_entry_bytes)
            .map_err(|_| HostAccessError::Rejected)?;
        Ok(UserContentDocument {
            metadata: to_user_content_summary(entry),
            descriptor,
        })
    }
}

fn to_user_content_summary(entry: UserContentEntry) -> UserContentSummary {
    UserContentSummary {
        id: entry.id.to_string(),
        content: entry.content.to_string(),
        revision: entry.revision.to_string(),
    }
}

impl WorldSessionAccess for LocalHostAccess {
    fn list_worlds(&self) -> HostAccessResult<Vec<WorldSessionSummary>> {
        self.home()?
            .list_worlds()
            .map_err(map_host_access_error)?
            .into_iter()
            .map(|world| self.world_sessions.summary(world))
            .collect()
    }

    fn create_world(&self) -> HostAccessResult<WorldSessionSummary> {
        let world = self.home()?.create_world().map_err(map_host_access_error)?;
        self.world_sessions.summary(world)
    }

    fn set_active(&self, world_id: &str, active: bool) -> HostAccessResult<()> {
        let world_id = world_id
            .parse::<WorldId>()
            .map_err(|_| HostAccessError::InvalidWorldId)?;
        self.home()?
            .load_world_state(world_id)
            .map_err(map_host_access_error)?;
        self.world_sessions.request(world_id, active)
    }
}

impl WorldCommandAccess for LocalHostAccess {
    fn submit_world_command(
        &self,
        request: WorldCommandRequest,
    ) -> WorldCommandAccessResult<AcceptedWorldCommand> {
        let world_id = request
            .world_id
            .parse::<WorldId>()
            .map_err(|_| WorldCommandAccessError::InvalidWorldId)?;
        let schema = request
            .schema
            .parse::<SchemaKey>()
            .map_err(|_| WorldCommandAccessError::InvalidSchema)?;
        let actor = match request.actor {
            WorldCommandActor::Principal => ActorRef::Principal(self.local_principal),
            WorldCommandActor::Entity(entity_id) => ActorRef::Entity(
                entity_id
                    .parse::<EntityId>()
                    .map_err(|_| WorldCommandAccessError::InvalidActor)?,
            ),
        };
        let payload = serde_json::from_slice(&request.payload_json)
            .map_err(|_| WorldCommandAccessError::InvalidPayload)?;
        self.home()
            .map_err(|_| WorldCommandAccessError::Unavailable)?
            .load_world_state(world_id)
            .map_err(map_world_command_host_error)?;
        if !self
            .world_sessions
            .accepts_commands(world_id)
            .map_err(|_| WorldCommandAccessError::Unavailable)?
        {
            return Err(WorldCommandAccessError::WorldNotActive);
        }

        let mut command = WorldCommand::new(schema, self.local_principal, actor, payload);
        if let Some(expected_position) = request.expected_position {
            command = command.expecting_position(expected_position);
        }
        self.world_commands.request(world_id, command)
    }
}

fn map_world_command_host_error(error: HostError) -> WorldCommandAccessError {
    match error {
        HostError::WorldNotFound(_) => WorldCommandAccessError::NotFound,
        HostError::Io(_) | HostError::Storage(_) => WorldCommandAccessError::Unavailable,
        _ => WorldCommandAccessError::Rejected,
    }
}

impl PreferenceAccess for LocalHostAccess {
    fn get(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<Option<String>> {
        self.home()?
            .get_preference(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                key,
            )
            .map_err(map_host_access_error)
    }

    fn set(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
        value: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .set_preference(
                RuntimeScopeId::new(scope_id),
                ComponentRef::new(instance_id, component_id),
                key.to_string(),
                value.to_string(),
            )
            .map_err(map_host_access_error)
    }

    fn delete(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .delete_preference(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                key,
            )
            .map_err(map_host_access_error)
    }
}

fn inspect_runtime_artifact(
    access: &LocalHostAccess,
    digest: &str,
) -> HostAccessResult<RuntimeArtifactPolicy> {
    let digest: ArtifactDigest = digest.parse().map_err(|_| HostAccessError::InvalidDigest)?;
    let home = access.home()?;
    let engine = ExtensionEngine::new();
    let manifest = RtwExtensionLoader::new()
        .read_stored_manifest(&engine, home.artifact_store(), &digest)
        .map_err(|_| HostAccessError::InvalidArtifact)?;
    let components = manifest
        .components
        .into_iter()
        .map(|component| RuntimePolicyRequest {
            component_id: component.id.to_string(),
            requested: component
                .permissions
                .runtime
                .into_iter()
                .map(|permission| permission.to_string())
                .collect(),
        })
        .collect();
    Ok(RuntimeArtifactPolicy {
        subject: manifest.id.to_string(),
        name: manifest.name,
        version: manifest.version,
        components,
    })
}

impl RuntimePolicyAccess for LocalHostAccess {
    fn inspect_artifact(&self, digest: &str) -> HostAccessResult<RuntimeArtifactPolicy> {
        inspect_runtime_artifact(self, digest)
    }

    fn list_components(&self) -> HostAccessResult<Vec<RuntimePolicyComponent>> {
        self.home()?
            .list_runtime_permission_policy()
            .map(to_runtime_policy_components)
            .map_err(map_host_access_error)
    }

    fn list_components_in_scope(
        &self,
        scope_id: &str,
    ) -> HostAccessResult<Vec<RuntimePolicyComponent>> {
        self.home()?
            .list_runtime_permission_policy_in_scope(&RuntimeScopeId::new(scope_id))
            .map(to_runtime_policy_components)
            .map_err(map_host_access_error)
    }

    fn grant(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        permission: &str,
    ) -> HostAccessResult<()> {
        let permission: RuntimePermission = permission
            .parse()
            .map_err(|_| HostAccessError::InvalidPermission)?;
        self.home()?
            .grant_runtime_permission(
                RuntimeScopeId::new(scope_id),
                ComponentRef::new(instance_id, component_id),
                permission,
            )
            .map_err(map_host_access_error)
    }

    fn revoke(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        permission: &str,
    ) -> HostAccessResult<()> {
        let permission: RuntimePermission = permission
            .parse()
            .map_err(|_| HostAccessError::InvalidPermission)?;
        self.home()?
            .revoke_runtime_permission(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                permission,
            )
            .map_err(map_host_access_error)
    }
}

fn to_runtime_policy_components(
    entries: Vec<crate::RuntimePermissionPolicyEntry>,
) -> Vec<RuntimePolicyComponent> {
    entries
        .into_iter()
        .map(|entry| RuntimePolicyComponent {
            scope_id: entry.scope_id.to_string(),
            instance_id: entry.instance_id.to_string(),
            component_id: entry.component_id.to_string(),
            requested: entry
                .requested
                .into_iter()
                .map(|permission| permission.to_string())
                .collect(),
            granted: entry
                .granted
                .into_iter()
                .map(|permission| permission.to_string())
                .collect(),
        })
        .collect()
}

impl CompositionAccess for LocalHostAccess {
    fn list_activations(&self) -> HostAccessResult<Vec<CompositionActivation>> {
        self.home()?
            .list_activations()
            .map(|activations| {
                activations
                    .into_iter()
                    .map(to_composition_activation)
                    .collect()
            })
            .map_err(map_host_access_error)
    }

    fn select_artifact(
        &self,
        digest: &str,
        enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation> {
        let digest: ArtifactDigest = digest.parse().map_err(|_| HostAccessError::InvalidDigest)?;
        self.home()?
            .select_stored_rtw(&digest, enabled)
            .map(to_composition_activation)
            .map_err(map_host_access_error)
    }

    fn set_enabled(&self, subject: &str, enabled: bool) -> HostAccessResult<()> {
        self.home()?
            .set_enabled(subject, enabled)
            .map_err(map_host_access_error)
    }

    fn set_world_default(&self, subject: &str, world_default: bool) -> HostAccessResult<()> {
        self.home()?
            .set_world_default(subject, world_default)
            .map_err(map_host_access_error)
    }

    fn remove_activation(&self, subject: &str) -> HostAccessResult<()> {
        self.home()?
            .remove_activation(subject)
            .map_err(map_host_access_error)
    }

    fn list_activations_in_scope(
        &self,
        scope_id: &str,
    ) -> HostAccessResult<Vec<CompositionActivation>> {
        self.home()?
            .list_activations_in_scope(&RuntimeScopeId::new(scope_id))
            .map(|activations| {
                activations
                    .into_iter()
                    .map(to_composition_activation)
                    .collect()
            })
            .map_err(map_host_access_error)
    }

    fn select_artifact_in_scope(
        &self,
        scope_id: &str,
        digest: &str,
        enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation> {
        let digest: ArtifactDigest = digest.parse().map_err(|_| HostAccessError::InvalidDigest)?;
        self.home()?
            .select_stored_rtw_in_scope(RuntimeScopeId::new(scope_id), &digest, enabled)
            .map(to_composition_activation)
            .map_err(map_host_access_error)
    }

    fn set_enabled_in_scope(
        &self,
        scope_id: &str,
        subject: &str,
        enabled: bool,
    ) -> HostAccessResult<()> {
        self.home()?
            .set_enabled_in_scope(&RuntimeScopeId::new(scope_id), subject, enabled)
            .map_err(map_host_access_error)
    }

    fn remove_activation_in_scope(&self, scope_id: &str, subject: &str) -> HostAccessResult<()> {
        self.home()?
            .remove_activation_in_scope(&RuntimeScopeId::new(scope_id), subject)
            .map_err(map_host_access_error)
    }
}

fn to_composition_activation(activation: crate::InstalledActivation) -> CompositionActivation {
    CompositionActivation {
        subject: activation.subject,
        content: activation.content.to_string(),
        name: activation.name,
        version: activation.version,
        digest: activation.digest.to_string(),
        instance_id: activation.instance_id.to_string(),
        scope_id: activation.scope_id.to_string(),
        enabled: activation.enabled,
        world_default: activation.world_default,
    }
}

fn map_artifact_import_error(error: HostError) -> HostAccessError {
    match error {
        HostError::Artifact(_) | HostError::UnsupportedContent(_) => {
            HostAccessError::InvalidArtifact
        }
        HostError::Io(_)
        | HostError::ProfileDecode(_)
        | HostError::ProfileEncode(_)
        | HostError::UnsupportedProfileSchema(_) => HostAccessError::Unavailable,
        _ => HostAccessError::Rejected,
    }
}

fn map_asset_import_error(error: rintawa_artifacts::AssetError) -> HostAccessError {
    match error {
        rintawa_artifacts::AssetError::InvalidDigest(_)
        | rintawa_artifacts::AssetError::InvalidMediaType { .. }
        | rintawa_artifacts::AssetError::UnsupportedSource(_)
        | rintawa_artifacts::AssetError::AssetTooLarge { .. } => HostAccessError::InvalidAsset,
        rintawa_artifacts::AssetError::Io(_)
        | rintawa_artifacts::AssetError::StoredAssetNotFound(_)
        | rintawa_artifacts::AssetError::StoreCorruption(_)
        | rintawa_artifacts::AssetError::SizeMismatch { .. }
        | rintawa_artifacts::AssetError::InvalidStoreEntry { .. } => HostAccessError::Unavailable,
    }
}

fn map_host_access_error(error: HostError) -> HostAccessError {
    match error {
        HostError::ActivationNotFound(_)
        | HostError::WorldNotFound(_)
        | HostError::UserContentNotFound(_) => HostAccessError::NotFound,
        HostError::UnsupportedContent(_) => HostAccessError::UnsupportedContent,
        HostError::Io(_)
        | HostError::ProfileDecode(_)
        | HostError::ProfileEncode(_)
        | HostError::UnsupportedProfileSchema(_) => HostAccessError::Unavailable,
        HostError::InvalidPreference(_) => HostAccessError::InvalidPreference,
        HostError::PreferenceQuotaExceeded => HostAccessError::PreferenceQuotaExceeded,
        HostError::Artifact(_) => HostAccessError::Rejected,
        _ => HostAccessError::Rejected,
    }
}

/// Authoritative command result plus best-effort live event delivery status.
#[derive(Debug)]
pub struct WorldCommandDispatchOutcome {
    authoritative: WorldCommandOutcome,
    live_delivery: HostResult<usize>,
}

impl WorldCommandDispatchOutcome {
    /// Returns the durable authoritative command outcome.
    pub const fn authoritative(&self) -> &WorldCommandOutcome {
        &self.authoritative
    }

    /// Returns successful live subscriber callbacks, or the post-commit delivery failure.
    ///
    /// A delivery error never means that the authoritative command was rolled back.
    pub fn live_delivery(&self) -> Result<usize, &HostError> {
        self.live_delivery.as_ref().copied()
    }

    /// Consumes the result into authoritative and ephemeral delivery parts.
    pub fn into_parts(self) -> (WorldCommandOutcome, HostResult<usize>) {
        (self.authoritative, self.live_delivery)
    }
}

struct ActiveWorldRuntimeParts {
    runtime: WorldRuntime,
    outbox: SqliteWorldStorage,
    effect_handlers: BTreeMap<SchemaKey, ServiceWorldEffectHandler>,
    projections: BTreeMap<SchemaKey, ServiceWorldProjection>,
}

struct ActiveWorld {
    runtime: WorldRuntime,
    outbox: SqliteWorldStorage,
    effect_handlers: BTreeMap<SchemaKey, ServiceWorldEffectHandler>,
    projections: BTreeMap<SchemaKey, ServiceWorldProjection>,
    signals: RuntimeSignalQueue,
    registered_instances: Vec<ExtensionInstanceId>,
    started_instances: Vec<ExtensionInstanceId>,
}

/// Running host composition containing baseline extensions and active worlds.
pub struct HostRuntime {
    // Active worlds are declared before the Engine so implicit field drop also
    // tears down command execution before provider component state disappears.
    active_worlds: BTreeMap<WorldId, ActiveWorld>,
    engine: ExtensionEngine,
    started_instances: Vec<ExtensionInstanceId>,
    host_shell_provider: Option<ComponentRef>,
    ui_layer_provider: Option<ComponentRef>,
    host_access: Option<Arc<LocalHostAccess>>,
}

impl HostRuntime {
    /// Loads and starts every enabled activation in the baseline profile.
    ///
    /// Bootstrap is staged to a fixed point. Artifacts whose required execution
    /// targets do not exist yet are deferred without side effects. Before the full
    /// topology is available, only target-provider-capable root WASM activations
    /// and their resolvable required contract dependency closure may start. Once
    /// every artifact is registered, the remaining instances use the ordinary
    /// deterministic full-topology activation plan.
    pub fn start(home: &HostHome) -> HostResult<Self> {
        let profile = home.load_profile()?;
        let world_sessions = WorldSessionRuntimeControl::default();
        let world_commands = WorldCommandRuntimeControl::default();
        let access = Arc::new(LocalHostAccess::new(
            home.root(),
            home.local_principal(),
            world_sessions,
            world_commands,
        ));
        let artifact_store_access: Arc<dyn ArtifactStoreAccess> = access.clone();
        let asset_store_access: Arc<dyn AssetStoreAccess> = access.clone();
        let user_content_access: Arc<dyn UserContentAccess> = access.clone();
        let world_session_access: Arc<dyn WorldSessionAccess> = access.clone();
        let world_command_access: Arc<dyn WorldCommandAccess> = access.clone();
        let composition_access: Arc<dyn CompositionAccess> = access.clone();
        let preference_access: Arc<dyn PreferenceAccess> = access.clone();
        let runtime_policy_access: Arc<dyn RuntimePolicyAccess> = access.clone();
        let mut engine = ExtensionEngine::with_host_access(
            artifact_store_access,
            asset_store_access,
            world_session_access,
            composition_access,
            preference_access,
            runtime_policy_access,
        );
        engine.attach_user_content_access(user_content_access);
        engine.attach_world_command_access(world_command_access);
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        let host_shell_contract = host_shell_contract_key();
        let ui_layer_contract = ui_layer_contract_key();
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            host_shell_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            ui_layer_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;

        let activated = activate_composition(&mut engine, home, &profile, &host_scope)?;
        let registered = activated.registered_instances;
        let started = activated.started_instances;
        let mut host_shell_provider = None;
        let mut ui_layer_provider = None;

        let result: HostResult<()> = (|| {
            let has_explicit_shell_selection =
                profile.preferred_providers.iter().any(|selection| {
                    selection.scope_id == host_scope && selection.contract() == host_shell_contract
                });
            host_shell_provider = match engine
                .resolve_active_contract_providers_in_scope(&host_scope, &host_shell_contract)
            {
                Ok(providers) => providers.into_iter().next(),
                Err(UnresolvedContractReason::NoProvider) if !has_explicit_shell_selection => None,
                Err(reason) => {
                    return Err(HostError::ContractRoleUnavailable {
                        scope_id: host_scope.to_string(),
                        contract: host_shell_contract.to_string(),
                        reason,
                    });
                }
            };

            let has_explicit_ui_layer_selection =
                profile.preferred_providers.iter().any(|selection| {
                    selection.scope_id == host_scope && selection.contract() == ui_layer_contract
                });
            ui_layer_provider = match engine
                .resolve_active_contract_providers_in_scope(&host_scope, &ui_layer_contract)
            {
                Ok(providers) => providers.into_iter().next(),
                Err(UnresolvedContractReason::NoProvider) if !has_explicit_ui_layer_selection => {
                    None
                }
                Err(reason) => {
                    return Err(HostError::ContractRoleUnavailable {
                        scope_id: host_scope.to_string(),
                        contract: ui_layer_contract.to_string(),
                        reason,
                    });
                }
            };
            if let Some(provider) = ui_layer_provider.clone() {
                engine.attach_registered_ui_layer(provider)?;
            }
            Ok(())
        })();

        if let Err(error) = result {
            let cleanup_failures = cleanup_instances(&mut engine, &started, &registered);
            if cleanup_failures.is_empty() {
                return Err(error);
            }
            return Err(HostError::BootstrapRollback(Box::new(BootstrapRollback {
                primary: Box::new(error),
                cleanup_failures,
            })));
        }

        Ok(Self {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: started,
            host_shell_provider,
            ui_layer_provider,
            host_access: Some(access),
        })
    }

    /// Returns the selected active provider of the platform Host Shell role.
    ///
    /// `None` is a valid headless composition with no eligible Host Shell provider.
    pub fn host_shell_provider(&self) -> Option<&ComponentRef> {
        self.host_shell_provider.as_ref()
    }

    /// Returns the selected active provider of the portable UI Layer role.
    ///
    /// `None` is valid for a headless composition or a shell that does not use
    /// the portable UI presentation protocol.
    pub fn ui_layer_provider(&self) -> Option<&ComponentRef> {
        self.ui_layer_provider.as_ref()
    }

    /// Imports one extension-validated RTW artifact into the generic user-content library.
    ///
    /// The root container and bounded descriptor are validated before CAS publication.
    /// Handler resolution uses the baseline `host` composition and ordinary `Single`
    /// provider policy; Core does not interpret feature-specific content semantics.
    ///
    /// # Errors
    ///
    /// Returns an RTW validation error, content-handler transport/protocol rejection,
    /// artifact-store failure, or library persistence error.
    pub fn import_user_content_rtw(
        &mut self,
        home: &HostHome,
        source: impl AsRef<Path>,
    ) -> HostResult<UserContentEntry> {
        let source = source.as_ref();
        let content = self.validate_user_content_rtw(source)?;
        let imported = home.artifact_store().import(source)?;
        home.insert_user_content_revision(content, imported.digest().clone())
    }

    /// Replaces the immutable revision of one logical user-content item.
    ///
    /// The logical [`UserContentId`] is preserved and the versioned RTW content type
    /// cannot change. Validation completes before the new bytes enter the CAS.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::UserContentNotFound`] for an unknown ID,
    /// [`HostError::UserContentTypeMismatch`] for a type change, or the same
    /// validation/storage errors as [`Self::import_user_content_rtw`].
    pub fn replace_user_content_rtw(
        &mut self,
        home: &HostHome,
        id: UserContentId,
        source: impl AsRef<Path>,
    ) -> HostResult<UserContentEntry> {
        let existing = home.indexed_user_content(id)?;
        let source = source.as_ref();
        let content = self.validate_user_content_rtw(source)?;
        if content != existing.content {
            return Err(HostError::UserContentTypeMismatch {
                id,
                expected: existing.content.to_string(),
                actual: content.to_string(),
            });
        }
        let imported = home.artifact_store().import(source)?;
        home.replace_user_content_revision(id, content, imported.digest().clone())
    }

    fn validate_user_content_rtw(&mut self, source: &Path) -> HostResult<ContentType> {
        let mut archive = RtwArchive::open(source, RtwLimits::default())?;
        let content = archive.manifest().content.clone();
        let entry = archive.manifest().entry.clone();
        let maximum_entry_bytes =
            u64::try_from(MAX_CONTENT_HANDLER_ENTRY_BYTES).unwrap_or(u64::MAX);
        let descriptor = archive.read_with_limit(&entry, maximum_entry_bytes)?;
        let contract = content_handler_service_contract_key(content.id(), content.major());
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        self.engine.define_platform_service_contract_in_scope(
            host_scope.clone(),
            contract.clone(),
            ContractResolutionPolicy::Single,
        )?;
        let request = ContentHandlerRequest::new(content.to_string(), descriptor);
        let request = serde_json::to_vec(&request).map_err(|source| {
            HostError::ContentHandlerRequestEncode {
                content: content.to_string(),
                source,
            }
        })?;
        let response = self
            .engine
            .platform_service_caller(host_scope)
            .call(&contract, &request)
            .map_err(|source| HostError::ContentHandlerCall {
                content: content.to_string(),
                source,
            })?;
        let response: ContentHandlerResponse =
            serde_json::from_slice(&response).map_err(|source| {
                HostError::ContentHandlerResponseDecode {
                    content: content.to_string(),
                    source,
                }
            })?;
        match response {
            ContentHandlerResponse::Accepted => Ok(content),
            ContentHandlerResponse::Rejected { diagnostic } => {
                if diagnostic.len() > MAX_CONTENT_HANDLER_DIAGNOSTIC_BYTES {
                    return Err(HostError::ContentHandlerDiagnosticTooLarge {
                        content: content.to_string(),
                        maximum_bytes: MAX_CONTENT_HANDLER_DIAGNOSTIC_BYTES,
                    });
                }
                Err(HostError::UserContentRejected {
                    content: content.to_string(),
                    diagnostic,
                })
            }
        }
    }

    /// Returns whether one authoritative world currently owns a running worker.
    pub fn is_world_active(&self, world_id: WorldId) -> bool {
        self.active_worlds.contains_key(&world_id)
    }

    /// Opens one persistent world and starts its authoritative command worker.
    ///
    /// Every persisted command schema is bound to a platform service owned by the
    /// extension that owns that schema. Provider availability remains dynamic:
    /// activating the world does not require every System provider to be online.
    ///
    /// # Errors
    ///
    /// Returns WorldAlreadyActive for duplicate activation, world/storage errors,
    /// contract reservation errors, or World Runtime startup errors.
    pub fn activate_world(&mut self, home: &HostHome, world_id: WorldId) -> HostResult<()> {
        let result = self.activate_world_inner(home, world_id);
        let status_result = self.record_world_session_result(world_id, &result);
        match result {
            Err(error) => Err(error),
            Ok(()) => status_result,
        }
    }

    fn activate_world_inner(&mut self, home: &HostHome, world_id: WorldId) -> HostResult<()> {
        if self.active_worlds.contains_key(&world_id) {
            return Err(HostError::WorldAlreadyActive(world_id));
        }

        let profile = home.load_world_composition(world_id)?;
        let scope_id = world_runtime_scope_id(world_id);
        let overlay = activate_composition(&mut self.engine, home, &profile, &scope_id)?;
        let runtime_result: HostResult<ActiveWorldRuntimeParts> = (|| {
            let storage = home.open_world_storage(world_id)?;
            register_world_schema_contributions(
                &self.engine,
                &storage,
                &overlay.registered_instances,
            )?;
            let session = storage.load_session()?;
            let mut builder = WorldRuntimeBuilder::new(storage);
            for definition in session
                .schemas()
                .iter()
                .filter(|definition| definition.kind() == SchemaKind::Command)
            {
                builder.register_system(self.bind_world_system(
                    world_id,
                    definition.key().clone(),
                    definition.owner().clone(),
                )?)?;
            }
            let mut effect_handlers = BTreeMap::new();
            for definition in session
                .schemas()
                .iter()
                .filter(|definition| definition.kind() == SchemaKind::Effect)
            {
                effect_handlers.insert(
                    definition.key().clone(),
                    self.bind_world_effect_handler(
                        world_id,
                        definition.key().clone(),
                        definition.owner().clone(),
                    )?,
                );
            }
            let mut projections = BTreeMap::new();
            for definition in session
                .schemas()
                .iter()
                .filter(|definition| definition.kind() == SchemaKind::Projection)
            {
                projections.insert(
                    definition.key().clone(),
                    self.bind_world_projection(
                        world_id,
                        definition.key().clone(),
                        definition.owner().clone(),
                    )?,
                );
            }
            let outbox = home.open_world_storage(world_id)?;
            Ok(ActiveWorldRuntimeParts {
                runtime: builder.start()?,
                outbox,
                effect_handlers,
                projections,
            })
        })();

        let runtime_parts = match runtime_result {
            Ok(runtime) => runtime,
            Err(error) => {
                let cleanup_failures = cleanup_instances(
                    &mut self.engine,
                    &overlay.started_instances,
                    &overlay.registered_instances,
                );
                clear_world_scope_topology(&mut self.engine, &scope_id);
                if cleanup_failures.is_empty() {
                    return Err(error);
                }
                return Err(HostError::BootstrapRollback(Box::new(BootstrapRollback {
                    primary: Box::new(error),
                    cleanup_failures,
                })));
            }
        };

        self.active_worlds.insert(
            world_id,
            ActiveWorld {
                runtime: runtime_parts.runtime,
                outbox: runtime_parts.outbox,
                effect_handlers: runtime_parts.effect_handlers,
                projections: runtime_parts.projections,
                signals: RuntimeSignalQueue::default(),
                registered_instances: overlay.registered_instances,
                started_instances: overlay.started_instances,
            },
        );
        Ok(())
    }

    /// Stops one active authoritative world worker after draining queued commands.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive when the world is not running, or the runtime
    /// shutdown failure after the world has been removed from the active set.
    pub fn deactivate_world(&mut self, world_id: WorldId) -> HostResult<()> {
        let result = self.deactivate_world_inner(world_id);
        let status_result = self.record_world_session_result(world_id, &result);
        match result {
            Err(error) => Err(error),
            Ok(()) => status_result,
        }
    }

    fn deactivate_world_inner(&mut self, world_id: WorldId) -> HostResult<()> {
        let active = self
            .active_worlds
            .remove(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        let (world_failures, cleanup_failures) =
            cleanup_active_world(&mut self.engine, world_id, active);
        if world_failures.is_empty() && cleanup_failures.is_empty() {
            Ok(())
        } else {
            Err(HostError::ShutdownFailed(Box::new(HostShutdownFailures {
                world_failures,
                cleanup_failures,
            })))
        }
    }

    /// Executes one command in an active world and then performs live event delivery.
    ///
    /// The outer result covers authoritative command execution only. Once a commit
    /// succeeds, live runtime-event delivery is reported separately inside
    /// WorldCommandDispatchOutcome so an ephemeral delivery failure cannot make a
    /// committed command appear rolled back. Idempotent replay is never redelivered.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive or an authoritative World Runtime failure.
    pub fn submit_world_command(
        &mut self,
        world_id: WorldId,
        command: WorldCommand,
    ) -> HostResult<WorldCommandDispatchOutcome> {
        let authoritative = {
            let active = self
                .active_worlds
                .get(&world_id)
                .ok_or(HostError::WorldNotActive(world_id))?;
            active.runtime.submit(command)?.wait_outcome()?
        };

        let live_delivery = if authoritative.receipt().disposition() == CommitDisposition::Committed
        {
            self.dispatch_world_events(authoritative.committed_events())
        } else {
            Ok(0)
        };

        Ok(WorldCommandDispatchOutcome {
            authoritative,
            live_delivery,
        })
    }

    /// Builds one schema-addressed, Principal-specific view of an active World.
    ///
    /// The projection provider receives only bounded owner-filtered reads from one
    /// pinned snapshot. It never receives SQLite handles or the raw WorldSnapshot,
    /// and the final value is validated against the registered Projection schema.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive, WorldProjectionUnavailable, a snapshot/storage error,
    /// or a typed projection protocol/policy failure.
    pub fn project_world(
        &self,
        world_id: WorldId,
        principal: PrincipalId,
        projection_schema: &SchemaKey,
        input: serde_json::Value,
    ) -> HostResult<WorldProjectionView> {
        let active = self
            .active_worlds
            .get(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        let projection = active
            .projections
            .get(projection_schema)
            .ok_or_else(|| HostError::WorldProjectionUnavailable(projection_schema.clone()))?;
        let snapshot = active.runtime.snapshot()?;
        Ok(projection.project(&snapshot, principal, input)?)
    }

    /// Binds one exact command schema to an extension-provided World System service.
    ///
    /// The service contract is platform-owned and scoped to the authoritative world.
    /// Provider resolution is pinned to the extension that owns the command schema,
    /// so another package in the same world scope cannot hijack the System role.
    /// The returned System still evaluates through the ordinary World Runtime, so
    /// transaction validation, authority checks, optimistic position checks, and
    /// commit ordering remain authoritative host responsibilities.
    ///
    /// # Errors
    ///
    /// Returns an Extension Engine contract-definition error when the same key was
    /// already reserved incompatibly or defined by an extension.
    fn bind_world_system(
        &mut self,
        world_id: WorldId,
        command_schema: SchemaKey,
        schema_owner: ExtensionId,
    ) -> HostResult<ServiceWorldSystem> {
        let scope_id = world_runtime_scope_id(world_id);
        let contract = world_system_service_contract_key(&command_schema);
        self.engine.define_platform_service_contract_in_scope(
            scope_id.clone(),
            contract,
            ContractResolutionPolicy::Single,
        )?;
        let caller = self
            .engine
            .platform_service_caller_for_extension(scope_id, schema_owner);
        Ok(ServiceWorldSystem::new(
            world_id,
            command_schema,
            move |contract: &ContractKey, request: &[u8]| caller.call(contract, request),
        ))
    }

    /// Binds one exact effect schema to an extension-provided durable handler service.
    ///
    /// The contract is platform-owned and owner-pinned to the extension that owns
    /// the immutable effect schema, preventing another package in the same world
    /// scope from hijacking post-commit external work.
    fn bind_world_effect_handler(
        &mut self,
        world_id: WorldId,
        effect_schema: SchemaKey,
        schema_owner: ExtensionId,
    ) -> HostResult<ServiceWorldEffectHandler> {
        let scope_id = world_runtime_scope_id(world_id);
        let contract = world_effect_service_contract_key(&effect_schema);
        self.engine.define_platform_service_contract_in_scope(
            scope_id.clone(),
            contract,
            ContractResolutionPolicy::Single,
        )?;
        let caller = self
            .engine
            .platform_service_caller_for_extension(scope_id, schema_owner);
        Ok(ServiceWorldEffectHandler::new(
            world_id,
            effect_schema,
            move |contract: &ContractKey, request: &[u8]| caller.call(contract, request),
        ))
    }

    /// Binds one exact Projection schema to its extension-owned filtering service.
    ///
    /// Provider resolution is pinned to the immutable schema owner in the exact
    /// world scope. The service receives a Host-fixed Principal and bounded read
    /// continuations; raw world storage remains behind the authoritative Host.
    fn bind_world_projection(
        &mut self,
        world_id: WorldId,
        projection_schema: SchemaKey,
        schema_owner: ExtensionId,
    ) -> HostResult<ServiceWorldProjection> {
        let scope_id = world_runtime_scope_id(world_id);
        let contract = world_projection_service_contract_key(&projection_schema);
        self.engine.define_platform_service_contract_in_scope(
            scope_id.clone(),
            contract,
            ContractResolutionPolicy::Single,
        )?;
        let caller = self
            .engine
            .platform_service_caller_for_extension(scope_id, schema_owner.clone());
        Ok(ServiceWorldProjection::new(
            world_id,
            projection_schema,
            schema_owner,
            move |contract: &ContractKey, request: &[u8]| caller.call(contract, request),
        ))
    }

    /// Delivers committed durable world events to one active runtime scope.
    ///
    /// Routing uses each event's exact versioned schema as the runtime topic.
    /// The callback payload is the serialized full `StoredWorldEvent` envelope,
    /// not only its feature payload, so subscribers retain authoritative world,
    /// position, actor/principal, causation, and correlation metadata.
    ///
    /// This method does not reinterpret or mutate the durable event. Delivery is
    /// ephemeral and may be retried from persistent world storage by a higher
    /// world-session supervisor.
    ///
    /// # Errors
    ///
    /// Returns an encoding error or Extension Engine delivery error.
    pub fn dispatch_world_events(&mut self, events: &[StoredWorldEvent]) -> HostResult<usize> {
        let Some(first) = events.first() else {
            return Ok(0);
        };
        if events.iter().any(|event| event.world_id != first.world_id) {
            return Err(HostError::MixedWorldEventBatch);
        }

        let scope_id = world_runtime_scope_id(first.world_id);
        let mut delivered = 0_usize;
        for event in events {
            let payload = serde_json::to_vec(event)?;
            delivered += self.engine.dispatch_runtime_event_in_scope(
                &scope_id,
                &event.schema.to_string(),
                &payload,
            )?;
        }
        Ok(delivered)
    }

    /// Enqueues one bounded ephemeral signal for an active world's runtime scope.
    ///
    /// Signals are intentionally in-memory only. They are discarded on world
    /// deactivation or process exit and never become authoritative history.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive, InvalidRuntimeSignalTopic, RuntimeSignalQueueFull,
    /// RuntimeSignalMessageTooLarge, or an envelope serialization error.
    pub fn enqueue_runtime_signal(&mut self, signal: RuntimeSignal) -> HostResult<()> {
        let world_id = signal.world_id();
        let active = self
            .active_worlds
            .get_mut(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        active.signals.enqueue(signal)
    }

    /// Drains queued runtime signals for one world in FIFO order.
    ///
    /// A dequeued signal is not retried after a callback-infrastructure failure:
    /// RuntimeSignal is deliberately best-effort and non-durable. Work requiring
    /// retry or restart recovery must use the durable Effect/Job outbox instead.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive or an Extension Engine delivery failure.
    pub fn pump_runtime_signals(&mut self, world_id: WorldId) -> HostResult<usize> {
        if !self.active_worlds.contains_key(&world_id) {
            return Err(HostError::WorldNotActive(world_id));
        }
        let scope_id = world_runtime_scope_id(world_id);
        let mut delivered = 0_usize;
        loop {
            let queued = self
                .active_worlds
                .get_mut(&world_id)
                .and_then(|active| active.signals.pop_front());
            let Some(queued) = queued else {
                break;
            };
            delivered += self.engine.dispatch_runtime_signal_in_scope(
                &scope_id,
                queued.topic(),
                queued.encoded(),
            )?;
        }
        Ok(delivered)
    }

    fn pump_all_runtime_signals(&mut self) -> HostResult<usize> {
        let world_ids = self.active_worlds.keys().copied().collect::<Vec<_>>();
        let mut delivered = 0_usize;
        for world_id in world_ids {
            delivered += self.pump_runtime_signals(world_id)?;
        }
        Ok(delivered)
    }

    /// Processes a bounded batch of due durable effect jobs for one active world.
    ///
    /// Provider/transport failures and invalid responses do not lose the durable
    /// job: they are returned to pending state with host-owned exponential backoff.
    /// A successful optional follow-up command goes through the ordinary authoritative
    /// World Runtime before the fencing attempt is marked complete.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive or a storage/fencing failure. Ordinary handler and
    /// command failures are durably retried rather than returned as host failures.
    pub fn pump_effect_outbox(&mut self, world_id: WorldId) -> HostResult<usize> {
        self.pump_effect_outbox_with_clock(world_id, current_unix_time_millis)
    }

    #[cfg(test)]
    fn pump_effect_outbox_at(
        &mut self,
        world_id: WorldId,
        now: UnixTimeMillis,
    ) -> HostResult<usize> {
        self.pump_effect_outbox_with_clock(world_id, || now)
    }

    fn pump_effect_outbox_with_clock<F>(
        &mut self,
        world_id: WorldId,
        mut clock: F,
    ) -> HostResult<usize>
    where
        F: FnMut() -> UnixTimeMillis,
    {
        if !self.active_worlds.contains_key(&world_id) {
            return Err(HostError::WorldNotActive(world_id));
        }
        let mut handled = 0_usize;

        for _ in 0..MAX_EFFECT_JOBS_PER_PUMP {
            let now = clock();
            let lease_expires_at =
                UnixTimeMillis::new(now.get().saturating_add(EFFECT_JOB_LEASE_MILLIS));
            let claim = {
                let active = self
                    .active_worlds
                    .get(&world_id)
                    .ok_or(HostError::WorldNotActive(world_id))?;
                active.outbox.claim_next_effect(now, lease_expires_at)?
            };
            let Some(claim) = claim else {
                break;
            };
            let job_id = claim.job().id;
            let attempt = claim.attempt();
            let response = {
                let active = self
                    .active_worlds
                    .get(&world_id)
                    .ok_or(HostError::WorldNotActive(world_id))?;
                match active.effect_handlers.get(&claim.job().schema) {
                    Some(handler) => handler.execute(&claim),
                    None => {
                        self.retry_effect_claim(
                            world_id,
                            job_id,
                            attempt,
                            now,
                            "effect schema is not bound to a handler contract",
                        )?;
                        handled += 1;
                        continue;
                    }
                }
            };

            match response {
                Ok(WorldEffectServiceResponse::Complete { command }) => {
                    if let Some(command) = command
                        && let Err(error) = self.submit_world_command(world_id, command)
                    {
                        let diagnostic = bounded_effect_diagnostic(&format!(
                            "effect follow-up command failed: {error}"
                        ));
                        self.retry_effect_claim(world_id, job_id, attempt, now, &diagnostic)?;
                        handled += 1;
                        continue;
                    }
                    let active = self
                        .active_worlds
                        .get(&world_id)
                        .ok_or(HostError::WorldNotActive(world_id))?;
                    active.outbox.complete_effect(job_id, attempt)?;
                }
                Ok(WorldEffectServiceResponse::Retry { reason }) => {
                    self.retry_effect_claim(world_id, job_id, attempt, now, &reason)?;
                }
                Ok(WorldEffectServiceResponse::Cancel { reason }) => {
                    let active = self
                        .active_worlds
                        .get(&world_id)
                        .ok_or(HostError::WorldNotActive(world_id))?;
                    active
                        .outbox
                        .cancel_claimed_effect(job_id, attempt, &reason)?;
                }
                Err(error) => {
                    let diagnostic = bounded_effect_diagnostic(&error.to_string());
                    self.retry_effect_claim(world_id, job_id, attempt, now, &diagnostic)?;
                }
            }
            handled += 1;
        }
        Ok(handled)
    }

    fn retry_effect_claim(
        &self,
        world_id: WorldId,
        job_id: rintawa_sdk::world::EffectJobId,
        attempt: u32,
        now: UnixTimeMillis,
        diagnostic: &str,
    ) -> HostResult<()> {
        let active = self
            .active_worlds
            .get(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        let available_at = effect_retry_at(now, attempt);
        active
            .outbox
            .retry_effect(job_id, attempt, available_at, diagnostic)?;
        Ok(())
    }

    fn pump_all_effect_outboxes(&mut self) -> HostResult<usize> {
        let world_ids = self.active_worlds.keys().copied().collect::<Vec<_>>();
        let mut handled = 0_usize;
        for world_id in world_ids {
            handled += self.pump_effect_outbox(world_id)?;
        }
        Ok(handled)
    }

    fn next_effect_wakeup_delay(&self, now: UnixTimeMillis) -> HostResult<Option<Duration>> {
        let mut earliest = None;
        for active in self.active_worlds.values() {
            let Some(wakeup) = active.outbox.next_effect_wakeup()? else {
                continue;
            };
            let millis = wakeup.get().saturating_sub(now.get()).max(0);
            let delay = Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX));
            earliest = Some(earliest.map_or(delay, |current: Duration| current.min(delay)));
        }
        Ok(earliest)
    }

    fn record_world_session_result(
        &self,
        world_id: WorldId,
        result: &HostResult<()>,
    ) -> HostResult<()> {
        let Some(access) = &self.host_access else {
            return Ok(());
        };
        let diagnostic = result.as_ref().err().map(ToString::to_string);
        access.world_sessions.record_outcome(
            world_id,
            self.is_world_active(world_id),
            diagnostic.as_deref(),
        )
    }

    fn pump_world_session_requests(&mut self) -> HostResult<usize> {
        let Some(access) = self.host_access.clone() else {
            return Ok(0);
        };
        let requests = access.world_sessions.drain_pending()?;
        if requests.is_empty() {
            return Ok(0);
        }
        let home = access
            .home()
            .map_err(|_| HostError::WorldSessionControlUnavailable)?;
        let mut handled = 0_usize;
        for (world_id, desired_active) in requests {
            if desired_active == self.is_world_active(world_id) {
                access
                    .world_sessions
                    .record_outcome(world_id, desired_active, None)?;
                handled += 1;
                continue;
            }
            // Apply the transition without re-entering the public wrapper so the
            // package-domain failure can be contained while control-state failures
            // still terminate the host pump.
            let lifecycle_result = if desired_active {
                self.activate_world_inner(&home, world_id)
            } else {
                self.deactivate_world_inner(world_id)
            };
            self.record_world_session_result(world_id, &lifecycle_result)?;
            handled += 1;
        }
        Ok(handled)
    }

    fn pump_world_command_requests(&mut self) -> HostResult<usize> {
        let Some(access) = self.host_access.clone() else {
            return Ok(0);
        };
        let requests = access.world_commands.drain_pending()?;
        let mut handled = 0_usize;
        for request in requests {
            let world_id = request.world_id;
            let command_id = request.command.id();
            match self.submit_world_command(world_id, request.command) {
                Ok(outcome) => {
                    if let Err(error) = outcome.live_delivery() {
                        warn!(
                            %world_id,
                            %command_id,
                            %error,
                            "deferred world command committed but live delivery failed"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        %world_id,
                        %command_id,
                        %error,
                        "deferred world command failed"
                    );
                }
            }
            handled = handled.saturating_add(1);
        }
        Ok(handled)
    }

    /// Executes one cooperative runtime pump for active baseline and world components.
    ///
    /// The returned duration is the earliest requested next wake-up. `None` means
    /// no active component currently owns scheduled cooperative work.
    pub fn poll_runtime(&mut self) -> HostResult<Option<Duration>> {
        self.pump_world_session_requests()?;
        self.pump_world_command_requests()?;
        self.pump_all_runtime_signals()?;
        self.pump_all_effect_outboxes()?;
        let engine_delay = self.engine.poll_runtime()?;
        self.pump_world_session_requests()?;
        self.pump_world_command_requests()?;
        let effect_delay = self.next_effect_wakeup_delay(current_unix_time_millis())?;
        Ok(min_optional_duration(engine_delay, effect_delay))
    }

    /// Stops every active world, then unregisters baseline runtime instances.
    pub fn shutdown(mut self) -> HostResult<()> {
        let (world_failures, mut cleanup_failures) =
            shutdown_worlds(&mut self.active_worlds, &mut self.engine);
        cleanup_failures.extend(cleanup_instances(
            &mut self.engine,
            &self.started_instances,
            &self.started_instances,
        ));
        if world_failures.is_empty() && cleanup_failures.is_empty() {
            Ok(())
        } else {
            Err(HostError::ShutdownFailed(Box::new(HostShutdownFailures {
                world_failures,
                cleanup_failures,
            })))
        }
    }
}

fn bounded_world_session_diagnostic(value: &str) -> String {
    if value.len() <= MAX_WORLD_SESSION_DIAGNOSTIC_BYTES {
        return value.to_string();
    }
    let mut end = MAX_WORLD_SESSION_DIAGNOSTIC_BYTES;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn current_unix_time_millis() -> UnixTimeMillis {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let millis = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    UnixTimeMillis::new(millis)
}

fn effect_retry_at(now: UnixTimeMillis, attempt: u32) -> UnixTimeMillis {
    let exponent = attempt.saturating_sub(1).min(16);
    let multiplier = 1_i64.checked_shl(exponent).unwrap_or(i64::MAX);
    let delay = EFFECT_RETRY_BASE_MILLIS
        .saturating_mul(multiplier)
        .min(EFFECT_RETRY_MAX_MILLIS);
    UnixTimeMillis::new(now.get().saturating_add(delay))
}

fn bounded_effect_diagnostic(reason: &str) -> String {
    if reason.len() <= MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES {
        return reason.to_string();
    }
    let mut end = MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

fn min_optional_duration(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

struct ActivatedComposition {
    registered_instances: Vec<ExtensionInstanceId>,
    started_instances: Vec<ExtensionInstanceId>,
}

fn activate_composition(
    engine: &mut ExtensionEngine,
    home: &HostHome,
    profile: &CompositionProfile,
    scope_id: &RuntimeScopeId,
) -> HostResult<ActivatedComposition> {
    profile.validate_scope(scope_id)?;
    engine.clear_preferred_contract_provider_policies_in_scope(scope_id);
    let loader = RtwExtensionLoader::new();
    let mut registered = Vec::new();
    let mut started = Vec::new();

    let result: HostResult<()> = (|| {
        let activations: Vec<_> = profile
            .activations
            .iter()
            .filter(|item| item.enabled)
            .cloned()
            .collect();
        for activation in &activations {
            if activation.content.to_string() != "rintawa.extension@1" {
                return Err(HostError::UnsupportedContent(
                    activation.content.to_string(),
                ));
            }
        }

        for selection in &profile.preferred_providers {
            engine.set_preferred_contract_provider_policy_in_scope(
                selection.scope_id.clone(),
                selection.contract(),
                selection.provider(),
            );
        }

        let mut pending = activations.clone();
        let mut registered_set = HashSet::new();
        let mut started_set = HashSet::new();
        let mut bootstrap_instances = HashSet::new();

        while !pending.is_empty() {
            let mut made_progress = false;
            let mut next_pending = Vec::new();
            let mut deferred = Vec::new();

            for activation in pending {
                match loader.try_load_stored_extension(
                    engine,
                    home.artifact_store(),
                    &activation.artifact,
                    activation.instance_id.clone(),
                    activation.scope_id.clone(),
                )? {
                    RtwExtensionLoadOutcome::Loaded(loaded) => {
                        registered_set.insert(activation.instance_id.clone());
                        registered.push(activation.instance_id.clone());
                        if loaded.can_publish_execution_targets {
                            bootstrap_instances.insert(activation.instance_id.clone());
                        }
                        for grant in profile.runtime_permissions.iter().filter(|grant| {
                            grant.scope_id == activation.scope_id
                                && grant.instance_id == activation.instance_id
                        }) {
                            engine.grant_requested_runtime_permission_for_instance(
                                &activation.instance_id,
                                &grant.component_id,
                                grant.permission,
                            )?;
                        }
                        made_progress = true;
                    }
                    RtwExtensionLoadOutcome::Deferred(waiting) => {
                        deferred.push(BootstrapDeferredActivation {
                            subject: activation.subject.clone(),
                            instance_id: activation.instance_id.clone(),
                            extension_id: waiting.extension_id.to_string(),
                            missing_required_targets: waiting
                                .missing_required_targets()
                                .cloned()
                                .collect(),
                        });
                        next_pending.push(activation);
                    }
                }
            }
            pending = next_pending;

            if pending.is_empty() {
                break;
            }

            let mut blocked = Vec::new();
            loop {
                blocked.clear();
                let mut started_one = false;
                let mut expanded_dependencies = false;
                for activation in &activations {
                    if !bootstrap_instances.contains(&activation.instance_id)
                        || !registered_set.contains(&activation.instance_id)
                        || started_set.contains(&activation.instance_id)
                    {
                        continue;
                    }
                    match engine.start_extension_instance(&activation.instance_id) {
                        Ok(()) => {
                            started_set.insert(activation.instance_id.clone());
                            started.push(activation.instance_id.clone());
                            made_progress = true;
                            started_one = true;
                            break;
                        }
                        Err(EngineError::ActivationPlan(reason)) => {
                            expanded_dependencies |= expand_bootstrap_dependencies(
                                engine,
                                &activation.scope_id,
                                &activation.instance_id,
                                &reason,
                                &registered_set,
                                &mut bootstrap_instances,
                            );
                            blocked.push(BootstrapBlockedActivation {
                                instance_id: activation.instance_id.clone(),
                                reason,
                            });
                        }
                        Err(error) => return Err(HostError::Engine(error)),
                    }
                }
                if started_one || expanded_dependencies {
                    continue;
                }
                break;
            }

            if made_progress {
                continue;
            }

            let blocked_instances: Vec<_> = activations
                .iter()
                .filter(|activation| {
                    bootstrap_instances.contains(&activation.instance_id)
                        && registered_set.contains(&activation.instance_id)
                        && !started_set.contains(&activation.instance_id)
                })
                .map(|activation| activation.instance_id.clone())
                .collect();
            let batch_error = bootstrap_batch_error(engine, &blocked_instances)?;
            return Err(HostError::BootstrapStalled(Box::new(BootstrapStall {
                deferred,
                blocked,
                batch_error,
            })));
        }

        let remaining_instances: Vec<_> = activations
            .iter()
            .filter(|activation| {
                registered_set.contains(&activation.instance_id)
                    && !started_set.contains(&activation.instance_id)
            })
            .map(|activation| activation.instance_id.clone())
            .collect();
        let plan = engine.plan_extension_activation(&remaining_instances)?;
        for instance_id in plan.into_ordered_instances() {
            engine.start_extension_instance(&instance_id)?;
            started_set.insert(instance_id.clone());
            started.push(instance_id);
        }
        Ok(())
    })();

    if let Err(error) = result {
        let cleanup_failures = cleanup_instances(engine, &started, &registered);
        engine.clear_preferred_contract_provider_policies_in_scope(scope_id);
        if cleanup_failures.is_empty() {
            return Err(error);
        }
        return Err(HostError::BootstrapRollback(Box::new(BootstrapRollback {
            primary: Box::new(error),
            cleanup_failures,
        })));
    }

    Ok(ActivatedComposition {
        registered_instances: registered,
        started_instances: started,
    })
}

fn register_world_schema_contributions(
    engine: &ExtensionEngine,
    storage: &SqliteWorldStorage,
    instance_ids: &[ExtensionInstanceId],
) -> HostResult<()> {
    let mut definitions = Vec::new();
    for instance_id in instance_ids {
        for registered in engine.registered_world_schemas(instance_id)? {
            let contribution = registered.contribution();
            let definition =
                serde_json::from_str(contribution.definition_json()).map_err(|source| {
                    HostError::InvalidWorldSchemaJson {
                        schema: contribution.key().clone(),
                        source,
                    }
                })?;
            definitions.push(SchemaDefinition::new(
                contribution.key().clone(),
                contribution.kind(),
                registered.extension_id().clone(),
                definition,
            ));
        }
    }

    definitions.sort_by(|left, right| {
        left.key()
            .cmp(right.key())
            .then_with(|| left.owner().as_str().cmp(right.owner().as_str()))
    });
    storage.register_schemas(&definitions)?;
    Ok(())
}

fn cleanup_active_world(
    engine: &mut ExtensionEngine,
    world_id: WorldId,
    active: ActiveWorld,
) -> (Vec<WorldRuntimeCleanupFailure>, Vec<HostCleanupFailure>) {
    let mut world_failures = Vec::new();
    if let Err(error) = active.runtime.shutdown() {
        world_failures.push(WorldRuntimeCleanupFailure { world_id, error });
    }
    let cleanup_failures = cleanup_instances(
        engine,
        &active.started_instances,
        &active.registered_instances,
    );
    clear_world_scope_topology(engine, &world_runtime_scope_id(world_id));
    (world_failures, cleanup_failures)
}

fn clear_world_scope_topology(engine: &mut ExtensionEngine, scope_id: &RuntimeScopeId) {
    engine.clear_preferred_contract_provider_policies_in_scope(scope_id);
    engine.clear_platform_contract_definitions_in_scope(scope_id);
}

fn shutdown_worlds(
    active_worlds: &mut BTreeMap<WorldId, ActiveWorld>,
    engine: &mut ExtensionEngine,
) -> (Vec<WorldRuntimeCleanupFailure>, Vec<HostCleanupFailure>) {
    let worlds = std::mem::take(active_worlds);
    let mut world_failures = Vec::new();
    let mut cleanup_failures = Vec::new();
    for (world_id, active) in worlds.into_iter().rev() {
        let (mut world_errors, mut extension_errors) =
            cleanup_active_world(engine, world_id, active);
        world_failures.append(&mut world_errors);
        cleanup_failures.append(&mut extension_errors);
    }
    (world_failures, cleanup_failures)
}

fn cleanup_instances(
    engine: &mut ExtensionEngine,
    started_instances: &[ExtensionInstanceId],
    registered_instances: &[ExtensionInstanceId],
) -> Vec<HostCleanupFailure> {
    let started: HashSet<_> = started_instances.iter().cloned().collect();
    let mut failures = Vec::new();

    // Registered-but-never-started dependents can still hold execution-target
    // dependencies that prevent their provider from stopping. Remove those first.
    for instance_id in registered_instances
        .iter()
        .rev()
        .filter(|instance_id| !started.contains(*instance_id))
    {
        record_unregister_failure(engine, instance_id, &mut failures);
    }

    // Started instances are recorded in provider-before-consumer order. Reverse
    // that order and unregister each dependent immediately after stop so provider
    // lifetime guards never observe a dangling registered dependent.
    for instance_id in started_instances.iter().rev() {
        if let Err(error) = engine.stop_extension_instance(instance_id) {
            failures.push(HostCleanupFailure {
                instance_id: instance_id.clone(),
                operation: HostCleanupOperation::Stop,
                error,
            });
        }
        record_unregister_failure(engine, instance_id, &mut failures);
    }
    failures
}

fn record_unregister_failure(
    engine: &mut ExtensionEngine,
    instance_id: &ExtensionInstanceId,
    failures: &mut Vec<HostCleanupFailure>,
) {
    if let Err(error) = engine.unregister_extension_instance(instance_id) {
        failures.push(HostCleanupFailure {
            instance_id: instance_id.clone(),
            operation: HostCleanupOperation::Unregister,
            error,
        });
    }
}

fn expand_bootstrap_dependencies(
    engine: &ExtensionEngine,
    scope_id: &RuntimeScopeId,
    activation_instance_id: &ExtensionInstanceId,
    reason: &ActivationPlanError,
    registered_instances: &HashSet<ExtensionInstanceId>,
    bootstrap_instances: &mut HashSet<ExtensionInstanceId>,
) -> bool {
    let ActivationPlanError::RequiredContractUnresolved {
        instance_id,
        component_id,
        contract,
        ..
    } = reason
    else {
        return false;
    };
    if instance_id != activation_instance_id {
        return false;
    }

    let consumer = ComponentRef::new(instance_id.clone(), component_id.clone());
    let topology = engine.composition_topology_snapshot_for_scope(scope_id);
    let Some(binding) = topology.bindings.iter().find(|binding| {
        binding.required && binding.consumer == consumer && binding.contract == *contract
    }) else {
        return false;
    };

    let mut expanded = false;
    for provider in &binding.providers {
        if provider.instance_id != *instance_id
            && registered_instances.contains(&provider.instance_id)
        {
            expanded |= bootstrap_instances.insert(provider.instance_id.clone());
        }
    }
    if let Some(definition_owner) = engine.contract_definition_owner_in_scope(scope_id, contract)
        && definition_owner.instance_id != *instance_id
        && registered_instances.contains(&definition_owner.instance_id)
    {
        expanded |= bootstrap_instances.insert(definition_owner.instance_id);
    }
    expanded
}

fn bootstrap_batch_error(
    engine: &ExtensionEngine,
    blocked_instances: &[ExtensionInstanceId],
) -> HostResult<Option<ActivationPlanError>> {
    if blocked_instances.is_empty() {
        return Ok(None);
    }
    match engine.plan_extension_activation(blocked_instances) {
        Ok(_) => Ok(None),
        Err(EngineError::ActivationPlan(reason)) => Ok(Some(reason)),
        Err(error) => Err(HostError::Engine(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::{
        context::{ComponentContext, RegistrationContext},
        contracts::{
            ContractConsumer, ContractDefinition, ContractKey, ContractProvider, ContractVersion,
        },
        contributions::WorldSchemaContribution,
        errors::{ExtensionError, ExtensionResult},
        manifest::ExtensionManifest,
        prelude::{Component, ComponentId, ContractResolutionPolicy, ExtensionId},
        runtime_effects::RuntimeEffect,
        world::{
            CommandId, CorrelationId, EffectJobId, PrincipalId, SchemaKey, UnixTimeMillis,
            WorldEventId, WorldId,
        },
        world_system::{WorldSystemEventProposal, WorldSystemTransaction},
    };
    use rintawa_storage::SqliteWorldStorage;
    use rintawa_world::{
        ActorRef, CausationRef, CommandProvenance, EffectJobDraft, SchemaDefinition, SchemaKind,
        WorldCommand, WorldEventDraft, WorldTransaction,
    };
    use rintawa_world_runtime::{
        WorldEffectServiceRequest, WorldEffectServiceResponse, WorldProjectionServiceRequest,
        WorldProjectionServiceResponse, WorldRuntimeBuilder, WorldSystem,
        WorldSystemServiceRequest, WorldSystemServiceResponse, world_effect_service_contract_key,
        world_projection_service_contract_key, world_system_service_contract_key,
    };
    use std::sync::{Arc, Mutex};

    struct ContractComponent {
        id: ComponentId,
        definition: Option<ContractDefinition>,
        provider: Option<ContractProvider>,
        consumer: Option<ContractConsumer>,
    }

    impl ContractComponent {
        fn new(id: &str) -> Self {
            Self {
                id: ComponentId::new(id),
                definition: None,
                provider: None,
                consumer: None,
            }
        }
    }

    impl Component for ContractComponent {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            if let Some(definition) = &self.definition {
                ctx.define_contract(definition.clone())?;
            }
            if let Some(provider) = &self.provider {
                ctx.provide_contract(provider.clone())?;
            }
            if let Some(consumer) = &self.consumer {
                ctx.consume_contract(consumer.clone())?;
            }
            Ok(())
        }
    }

    fn test_manifest(id: &str) -> ExtensionManifest {
        ExtensionManifest {
            id: ExtensionId::new(id),
            name: id.to_string(),
            version: String::from("0.0.1"),
            sdk: String::from("^0.0"),
            components: Vec::new(),
        }
    }

    struct RecordingWorldSystem {
        command_schema: SchemaKey,
        event_schema: SchemaKey,
        observed: Arc<Mutex<Vec<WorldCommand>>>,
    }

    impl WorldSystem for RecordingWorldSystem {
        fn command_schema(&self) -> &SchemaKey {
            &self.command_schema
        }

        fn evaluate(
            &self,
            _snapshot: &rintawa_world_runtime::WorldSnapshot,
            command: &WorldCommand,
        ) -> rintawa_world_runtime::SystemResult<WorldTransaction> {
            self.observed
                .lock()
                .map_err(|_| {
                    rintawa_world_runtime::SystemError::Failed(String::from(
                        "test command observer lock poisoned",
                    ))
                })?
                .push(command.clone());
            let mut transaction = WorldTransaction::new();
            transaction.push_event(WorldEventDraft::new(
                self.event_schema.clone(),
                command.payload().clone(),
            ));
            Ok(transaction)
        }
    }

    struct ContentHandlerProvider {
        id: ComponentId,
        contract: ContractKey,
        observed: Arc<Mutex<Vec<ContentHandlerRequest>>>,
    }

    impl Component for ContentHandlerProvider {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.provide_contract(ContractProvider::new(self.contract.clone()))
        }

        fn handle_service(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            contract: &ContractKey,
            request: &[u8],
        ) -> ExtensionResult<Vec<u8>> {
            if contract != &self.contract {
                return Err(ExtensionError::ServiceHandlerUnavailable(
                    contract.to_string(),
                ));
            }
            let request: ContentHandlerRequest = serde_json::from_slice(request)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            self.observed
                .lock()
                .map_err(|_| {
                    ExtensionError::Message(String::from("content observer lock poisoned"))
                })?
                .push(request.clone());
            let response = if request.descriptor == b"reject" {
                ContentHandlerResponse::Rejected {
                    diagnostic: String::from("descriptor rejected by test handler"),
                }
            } else {
                ContentHandlerResponse::Accepted
            };
            serde_json::to_vec(&response)
                .map_err(|error| ExtensionError::Message(error.to_string()))
        }
    }

    fn write_test_content_rtw(
        root: &Path,
        name: &str,
        content: &str,
        descriptor: &[u8],
    ) -> anyhow::Result<PathBuf> {
        let source = root.join(format!("{name}-source"));
        std::fs::create_dir(&source)?;
        std::fs::write(
            source.join("rtw.toml"),
            format!("format = 1\ncontent = \"{content}\"\nentry = \"content.bin\"\n"),
        )?;
        std::fs::write(source.join("content.bin"), descriptor)?;
        let output = root.join(format!("{name}.rtw"));
        rintawa_artifacts::pack_directory(&source, &output, RtwLimits::default())?;
        Ok(output)
    }

    struct WorldSchemaComponent {
        id: ComponentId,
        schemas: Vec<WorldSchemaContribution>,
    }

    impl Component for WorldSchemaComponent {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            for schema in &self.schemas {
                ctx.register_world_schema(schema.clone())?;
            }
            Ok(())
        }
    }

    struct WorldSystemProvider {
        id: ComponentId,
        contract: ContractKey,
        event_schema: SchemaKey,
    }

    impl Component for WorldSystemProvider {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.provide_contract(ContractProvider::new(self.contract.clone()))
        }

        fn handle_service(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            contract: &ContractKey,
            request: &[u8],
        ) -> ExtensionResult<Vec<u8>> {
            if contract != &self.contract {
                return Err(ExtensionError::ServiceHandlerUnavailable(
                    contract.to_string(),
                ));
            }
            let request: WorldSystemServiceRequest = serde_json::from_slice(request)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            let mut transaction = WorldSystemTransaction::new();
            transaction.push_event(WorldSystemEventProposal {
                schema: self.event_schema.clone(),
                payload: request.command().payload.clone(),
            });
            serde_json::to_vec(&WorldSystemServiceResponse::Transaction { transaction })
                .map_err(|error| ExtensionError::Message(error.to_string()))
        }
    }

    struct WorldEffectProvider {
        id: ComponentId,
        contract: ContractKey,
        result_command_schema: SchemaKey,
        remaining_retries: usize,
        should_cancel: bool,
        observed: Arc<Mutex<Vec<WorldEffectServiceRequest>>>,
    }

    impl Component for WorldEffectProvider {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.provide_contract(ContractProvider::new(self.contract.clone()))
        }

        fn handle_service(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            contract: &ContractKey,
            request: &[u8],
        ) -> ExtensionResult<Vec<u8>> {
            if contract != &self.contract {
                return Err(ExtensionError::ServiceHandlerUnavailable(
                    contract.to_string(),
                ));
            }
            let request: WorldEffectServiceRequest = serde_json::from_slice(request)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            self.observed
                .lock()
                .map_err(|_| {
                    ExtensionError::Message(String::from("effect observer lock poisoned"))
                })?
                .push(request.clone());

            let response = if self.should_cancel {
                WorldEffectServiceResponse::Cancel {
                    reason: String::from("test cancellation"),
                }
            } else if self.remaining_retries > 0 {
                self.remaining_retries -= 1;
                WorldEffectServiceResponse::Retry {
                    reason: String::from("temporary provider failure"),
                }
            } else {
                let job = request.job();
                let command = WorldCommand::with_ids(
                    request.completion_command_id(),
                    job.provenance.correlation_id,
                    self.result_command_schema.clone(),
                    job.provenance.principal,
                    job.provenance.actor,
                    serde_json::json!({ "effect_job_id": job.id.to_string() }),
                )
                .caused_by(CausationRef::Effect(job.id));
                WorldEffectServiceResponse::Complete {
                    command: Some(command),
                }
            };
            serde_json::to_vec(&response)
                .map_err(|error| ExtensionError::Message(error.to_string()))
        }
    }

    struct WorldProjectionProvider {
        id: ComponentId,
        contract: ContractKey,
        reject: bool,
        observed: Arc<Mutex<Vec<WorldProjectionServiceRequest>>>,
    }

    impl Component for WorldProjectionProvider {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.provide_contract(ContractProvider::new(self.contract.clone()))
        }

        fn handle_service(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            contract: &ContractKey,
            request: &[u8],
        ) -> ExtensionResult<Vec<u8>> {
            if contract != &self.contract {
                return Err(ExtensionError::ServiceHandlerUnavailable(
                    contract.to_string(),
                ));
            }
            let request: WorldProjectionServiceRequest = serde_json::from_slice(request)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            self.observed
                .lock()
                .map_err(|_| {
                    ExtensionError::Message(String::from("projection observer lock poisoned"))
                })?
                .push(request.clone());
            if self.reject {
                return Err(ExtensionError::Message(String::from(
                    "hijacker projection provider invoked",
                )));
            }
            serde_json::to_vec(&WorldProjectionServiceResponse::Complete {
                value: serde_json::json!({
                    "provider": "owner",
                    "principal": request.principal().to_string(),
                }),
            })
            .map_err(|error| ExtensionError::Message(error.to_string()))
        }
    }

    fn enqueue_test_effect(
        storage: &SqliteWorldStorage,
        command_schema: &SchemaKey,
        effect_schema: &SchemaKey,
        principal: PrincipalId,
    ) -> anyhow::Result<(EffectJobId, CorrelationId)> {
        let command = WorldCommand::new(
            command_schema.clone(),
            principal,
            ActorRef::Principal(principal),
            serde_json::json!({ "kind": "enqueue-effect" }),
        );
        let correlation_id = command.correlation_id();
        let effect = EffectJobDraft::new(
            effect_schema.clone(),
            serde_json::json!({ "input": "generate" }),
        );
        let effect_id = effect.id();
        let mut transaction = WorldTransaction::new();
        transaction.push_effect(effect);
        let snapshot = storage.snapshot_for_command(&command)?;
        let position = snapshot.position();
        drop(snapshot);
        storage.commit(&command, &transaction, position)?;
        Ok((effect_id, correlation_id))
    }

    struct TestEffectRoutes<'a> {
        result_owner: &'a ExtensionId,
        result_schema: &'a SchemaKey,
        event_schema: &'a SchemaKey,
        effect_owner: &'a ExtensionId,
        effect_schema: &'a SchemaKey,
        remaining_retries: usize,
        observed: Arc<Mutex<Vec<WorldEffectServiceRequest>>>,
    }

    fn register_test_effect_routes(
        host: &mut HostRuntime,
        world_id: WorldId,
        routes: TestEffectRoutes<'_>,
    ) -> anyhow::Result<()> {
        let scope = world_runtime_scope_id(world_id);
        let system_instance = ExtensionInstanceId::new("effect-result-system");
        host.engine.register_extension_instance(
            system_instance.clone(),
            scope.clone(),
            test_manifest(routes.result_owner.as_str()),
            vec![Box::new(WorldSystemProvider {
                id: ComponentId::new("runtime"),
                contract: world_system_service_contract_key(routes.result_schema),
                event_schema: routes.event_schema.clone(),
            })],
        )?;
        host.engine.start_extension_instance(&system_instance)?;

        let effect_contract = world_effect_service_contract_key(routes.effect_schema);
        let hijacker_instance = ExtensionInstanceId::new("a-effect-hijacker");
        host.engine.register_extension_instance(
            hijacker_instance.clone(),
            scope.clone(),
            test_manifest("rintawa.effect-hijacker"),
            vec![Box::new(WorldEffectProvider {
                id: ComponentId::new("runtime"),
                contract: effect_contract.clone(),
                result_command_schema: routes.result_schema.clone(),
                remaining_retries: 0,
                should_cancel: true,
                observed: Arc::new(Mutex::new(Vec::new())),
            })],
        )?;
        host.engine.start_extension_instance(&hijacker_instance)?;

        let provider_instance = ExtensionInstanceId::new("z-effect-provider");
        host.engine.register_extension_instance(
            provider_instance.clone(),
            scope,
            test_manifest(routes.effect_owner.as_str()),
            vec![Box::new(WorldEffectProvider {
                id: ComponentId::new("runtime"),
                contract: effect_contract,
                result_command_schema: routes.result_schema.clone(),
                remaining_retries: routes.remaining_retries,
                should_cancel: false,
                observed: routes.observed,
            })],
        )?;
        host.engine.start_extension_instance(&provider_instance)?;

        let active = host
            .active_worlds
            .get_mut(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        active.registered_instances.extend([
            system_instance.clone(),
            hijacker_instance.clone(),
            provider_instance.clone(),
        ]);
        active
            .started_instances
            .extend([system_instance, hijacker_instance, provider_instance]);
        Ok(())
    }

    struct RejectingWorldSystemProvider {
        id: ComponentId,
        contract: ContractKey,
    }

    impl Component for RejectingWorldSystemProvider {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.provide_contract(ContractProvider::new(self.contract.clone()))
        }

        fn handle_service(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            contract: &ContractKey,
            _request: &[u8],
        ) -> ExtensionResult<Vec<u8>> {
            if contract != &self.contract {
                return Err(ExtensionError::ServiceHandlerUnavailable(
                    contract.to_string(),
                ));
            }
            serde_json::to_vec(&WorldSystemServiceResponse::Rejected {
                reason: String::from("hijacker selected"),
            })
            .map_err(|error| ExtensionError::Message(error.to_string()))
        }
    }

    struct RuntimeSignalSubscriber {
        id: ComponentId,
        topic: String,
        observed: Arc<Mutex<Vec<RuntimeSignal>>>,
    }

    impl Component for RuntimeSignalSubscriber {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            ctx.register_runtime_effect(RuntimeEffect::signal_subscription(self.topic.clone()))?;
            Ok(())
        }

        fn handle_event(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            _topic: &str,
            payload: &[u8],
        ) -> ExtensionResult<()> {
            let signal: RuntimeSignal = serde_json::from_slice(payload)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            self.observed
                .lock()
                .map_err(|_| {
                    ExtensionError::Message(String::from("signal observer lock poisoned"))
                })?
                .push(signal);
            Ok(())
        }
    }

    struct WorldEventSubscriber {
        id: ComponentId,
        topic: String,
        observed: Arc<Mutex<Vec<(String, StoredWorldEvent)>>>,
    }

    impl Component for WorldEventSubscriber {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            ctx.register_runtime_effect(RuntimeEffect::event_subscription(self.topic.clone()))?;
            Ok(())
        }

        fn handle_event(
            &mut self,
            _ctx: &mut dyn ComponentContext,
            topic: &str,
            payload: &[u8],
        ) -> ExtensionResult<()> {
            let event: StoredWorldEvent = serde_json::from_slice(payload)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            self.observed
                .lock()
                .map_err(|_| ExtensionError::Message(String::from("event observer lock poisoned")))?
                .push((topic.to_string(), event));
            Ok(())
        }
    }

    #[test]
    fn test_should_publish_registered_extension_world_schemas_atomically() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let world_id = WorldId::new();
        let storage = SqliteWorldStorage::create(root.path().join("world.sqlite"), world_id)?;
        let scope = world_runtime_scope_id(world_id);
        let owner = ExtensionId::new("rintawa.schema-owner");
        let instance = ExtensionInstanceId::new("schema-owner-instance");
        let first: SchemaKey = "rintawa.test.extension-event@1".parse()?;
        let second: SchemaKey = "rintawa.test.extension-facet@1".parse()?;
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            instance.clone(),
            scope,
            test_manifest(owner.as_str()),
            vec![Box::new(WorldSchemaComponent {
                id: ComponentId::new("schemas"),
                schemas: vec![
                    WorldSchemaContribution::new(
                        first.clone(),
                        SchemaKind::Event,
                        r#"{"type":"object"}"#,
                    ),
                    WorldSchemaContribution::new(
                        second.clone(),
                        SchemaKind::Facet,
                        r#"{"type":"string"}"#,
                    ),
                ],
            })],
        )?;

        register_world_schema_contributions(&engine, &storage, std::slice::from_ref(&instance))?;
        let session = storage.load_session()?;
        let first_definition = session
            .schemas()
            .get(&first)
            .expect("event schema must persist");
        assert_eq!(first_definition.owner(), &owner);
        assert_eq!(first_definition.kind(), SchemaKind::Event);
        assert_eq!(
            session.schemas().get(&second).map(SchemaDefinition::kind),
            Some(SchemaKind::Facet)
        );

        register_world_schema_contributions(&engine, &storage, &[instance])?;
        assert_eq!(storage.load_session()?.schemas().len(), 2);
        Ok(())
    }

    #[test]
    fn test_should_reject_malformed_extension_schema_without_partial_publication()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let world_id = WorldId::new();
        let storage = SqliteWorldStorage::create(root.path().join("world.sqlite"), world_id)?;
        let scope = world_runtime_scope_id(world_id);
        let owner = ExtensionId::new("rintawa.bad-schema-owner");
        let instance = ExtensionInstanceId::new("bad-schema-owner-instance");
        let valid: SchemaKey = "rintawa.test.valid-before-bad@1".parse()?;
        let invalid: SchemaKey = "rintawa.test.invalid-json@1".parse()?;
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            instance.clone(),
            scope,
            test_manifest(owner.as_str()),
            vec![Box::new(WorldSchemaComponent {
                id: ComponentId::new("schemas"),
                schemas: vec![
                    WorldSchemaContribution::new(
                        valid.clone(),
                        SchemaKind::Event,
                        r#"{"type":"object"}"#,
                    ),
                    WorldSchemaContribution::new(invalid.clone(), SchemaKind::Facet, "{"),
                ],
            })],
        )?;

        assert!(matches!(
            register_world_schema_contributions(&engine, &storage, &[instance]),
            Err(HostError::InvalidWorldSchemaJson { schema, .. }) if schema == invalid
        ));
        assert!(storage.load_session()?.schemas().is_empty());
        Ok(())
    }

    #[test]
    fn test_should_import_raw_asset_through_generic_host_access() -> anyhow::Result<()> {
        let root = tempfile::TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let access = LocalHostAccess::new(
            home.root(),
            home.local_principal(),
            WorldSessionRuntimeControl::default(),
            WorldCommandRuntimeControl::default(),
        );
        let imported = AssetStoreAccess::import_asset(&access, b"portrait", "Image/PNG")
            .map_err(|error| anyhow::anyhow!("asset host access failed: {error:?}"))?;

        assert_eq!(imported.size, 8);
        assert_eq!(imported.media_type, "image/png");
        let reference = rintawa_artifacts::AssetRef::new(
            rintawa_artifacts::AssetDigest::parse(&imported.digest)?,
            imported.size,
            imported.media_type,
        )?;
        home.asset_store().verify(&reference)?;
        assert_eq!(
            AssetStoreAccess::import_asset(&access, b"portrait", "invalid media type"),
            Err(HostAccessError::InvalidAsset)
        );
        Ok(())
    }

    #[test]
    fn test_should_not_publish_user_content_without_active_handler() -> anyhow::Result<()> {
        let root = tempfile::TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let mut host = HostRuntime::start(&home)?;
        let content = ContentType::parse("rintawa.unhandled-content@1")?;
        let source = write_test_content_rtw(
            root.path(),
            "unhandled-content",
            &content.to_string(),
            b"valid-container",
        )?;
        let digest = ArtifactDigest::sha256(&std::fs::read(&source)?);

        assert!(matches!(
            host.import_user_content_rtw(&home, &source),
            Err(HostError::ContentHandlerCall { .. })
        ));
        assert!(home.list_user_content()?.is_empty());
        assert!(home.artifact_store().open_artifact(&digest).is_err());
        Ok(())
    }

    #[test]
    fn test_should_bound_and_coalesce_world_session_request_queue() {
        let control = WorldSessionRuntimeControl::default();
        let mut ids = Vec::new();
        for _ in 0..MAX_PENDING_WORLD_SESSION_REQUESTS {
            let world_id = WorldId::new();
            control
                .request(world_id, true)
                .expect("request within queue capacity must succeed");
            ids.push(world_id);
        }
        assert_eq!(
            control.request(WorldId::new(), true),
            Err(HostAccessError::QueueFull)
        );
        control
            .request(ids[0], false)
            .expect("coalescing an existing world must not consume extra capacity");
        let drained = control
            .drain_pending()
            .expect("bounded queue must remain healthy");
        assert_eq!(drained.len(), MAX_PENDING_WORLD_SESSION_REQUESTS);
        assert!(drained.contains(&(ids[0], false)));
    }

    #[test]
    fn test_should_preserve_newer_world_session_request_after_older_completion()
    -> anyhow::Result<()> {
        let control = WorldSessionRuntimeControl::default();
        let world_id = WorldId::new();
        let summary = WorldSummary {
            id: world_id,
            commit_position: 0,
        };

        control
            .request(world_id, true)
            .expect("initial activation request must fit queue");
        assert_eq!(control.drain_pending()?, vec![(world_id, true)]);

        control
            .request(world_id, false)
            .expect("newer deactivation request must fit queue");
        control.record_outcome(world_id, true, None)?;

        let observed = control
            .summary(summary)
            .expect("world-session state must remain readable");
        assert!(observed.active);
        assert_eq!(observed.pending_active, Some(false));
        Ok(())
    }

    #[test]
    fn test_should_report_actual_world_state_separately_from_lifecycle_error() -> anyhow::Result<()>
    {
        let control = WorldSessionRuntimeControl::default();
        let world_id = WorldId::new();
        control.record_outcome(world_id, true, None)?;
        control.record_outcome(world_id, false, Some("cleanup failed after stop"))?;

        let observed = control
            .summary(WorldSummary {
                id: world_id,
                commit_position: 3,
            })
            .expect("world-session status must remain readable");
        assert!(!observed.active);
        assert_eq!(observed.pending_active, None);
        assert_eq!(
            observed.last_error.as_deref(),
            Some("cleanup failed after stop")
        );
        Ok(())
    }

    #[test]
    fn test_should_apply_deferred_world_session_lifecycle_requests_on_host_pump()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let mut host = HostRuntime::start(&home)?;
        let access = host
            .host_access
            .as_ref()
            .expect("production HostRuntime must retain host access")
            .clone();

        assert_eq!(
            WorldSessionAccess::set_active(access.as_ref(), "not-a-world-id", true),
            Err(HostAccessError::InvalidWorldId)
        );
        assert_eq!(
            WorldSessionAccess::set_active(access.as_ref(), &WorldId::new().to_string(), true),
            Err(HostAccessError::NotFound)
        );

        let created = WorldSessionAccess::create_world(access.as_ref())
            .expect("world-session access must create an empty world");
        let world_id: WorldId = created.world_id.parse()?;
        assert_eq!(created.commit_position, 0);
        assert!(!created.active);
        assert_eq!(created.pending_active, None);
        assert!(!host.is_world_active(world_id));

        WorldSessionAccess::set_active(access.as_ref(), &created.world_id, true)
            .expect("world-session access must queue activation");
        let pending = WorldSessionAccess::list_worlds(access.as_ref())
            .expect("world-session access must list pending activation");
        let pending = pending
            .into_iter()
            .find(|world| world.world_id == created.world_id)
            .expect("created world must remain listed");
        assert!(!pending.active);
        assert_eq!(pending.pending_active, Some(true));

        host.poll_runtime()?;
        assert!(host.is_world_active(world_id));
        let active = WorldSessionAccess::list_worlds(access.as_ref())
            .expect("world-session access must list active world")
            .into_iter()
            .find(|world| world.world_id == created.world_id)
            .expect("active world must remain listed");
        assert!(active.active);
        assert_eq!(active.pending_active, None);
        assert_eq!(active.last_error, None);

        WorldSessionAccess::set_active(access.as_ref(), &created.world_id, false)
            .expect("world-session access must queue deactivation");
        let pending = WorldSessionAccess::list_worlds(access.as_ref())
            .expect("world-session access must list pending deactivation")
            .into_iter()
            .find(|world| world.world_id == created.world_id)
            .expect("pending world must remain listed");
        assert!(pending.active);
        assert_eq!(pending.pending_active, Some(false));

        host.poll_runtime()?;
        assert!(!host.is_world_active(world_id));
        let inactive = WorldSessionAccess::list_worlds(access.as_ref())
            .expect("world-session access must list inactive world")
            .into_iter()
            .find(|world| world.world_id == created.world_id)
            .expect("inactive world must remain listed");
        assert!(!inactive.active);
        assert_eq!(inactive.pending_active, None);
        assert_eq!(inactive.last_error, None);
        Ok(())
    }

    #[test]
    fn test_should_expose_persisted_user_content_through_generic_read_access() -> anyhow::Result<()>
    {
        let root = tempfile::TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let mut host = HostRuntime::start(&home)?;
        let content = ContentType::parse("rintawa.test-content@1")?;
        let contract = content_handler_service_contract_key(content.id(), content.major());
        let provider_instance = ExtensionInstanceId::new("content-handler-provider");

        host.engine.register_extension_instance(
            provider_instance.clone(),
            RuntimeScopeId::new(HOST_SCOPE),
            test_manifest("rintawa.test-content-handler"),
            vec![Box::new(ContentHandlerProvider {
                id: ComponentId::new("runtime"),
                contract,
                observed: Arc::new(Mutex::new(Vec::new())),
            })],
        )?;
        host.engine.start_extension_instance(&provider_instance)?;

        let descriptor = br#"{"name":"Example"}"#;
        let source = write_test_content_rtw(
            root.path(),
            "generic-read-content",
            &content.to_string(),
            descriptor,
        )?;
        let imported = host.import_user_content_rtw(&home, source)?;
        let access = host
            .host_access
            .as_ref()
            .expect("production HostRuntime must retain host access")
            .clone();

        assert_eq!(
            UserContentAccess::list_user_content(access.as_ref(), Some("not-versioned")),
            Err(HostAccessError::InvalidContentType)
        );
        assert!(
            UserContentAccess::list_user_content(access.as_ref(), Some("rintawa.other-content@1"))
                .expect("valid content filter should remain readable")
                .is_empty()
        );

        let listed =
            UserContentAccess::list_user_content(access.as_ref(), Some("rintawa.test-content@1"))
                .expect("persisted user content should remain readable");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, imported.id.to_string());
        assert_eq!(listed[0].content, content.to_string());
        assert_eq!(listed[0].revision, imported.revision.to_string());

        assert_eq!(
            UserContentAccess::read_user_content(access.as_ref(), "not-an-id"),
            Err(HostAccessError::InvalidUserContentId)
        );
        let document =
            UserContentAccess::read_user_content(access.as_ref(), &imported.id.to_string())
                .expect("persisted user-content descriptor should remain readable");
        assert_eq!(document.metadata, listed[0]);
        assert_eq!(document.descriptor, descriptor);
        Ok(())
    }

    #[test]
    fn test_should_validate_and_revision_user_content_through_extension_handler()
    -> anyhow::Result<()> {
        let root = tempfile::TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let mut host = HostRuntime::start(&home)?;
        let content = ContentType::parse("rintawa.test-content@1")?;
        let contract = content_handler_service_contract_key(content.id(), content.major());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let provider_instance = ExtensionInstanceId::new("content-handler-provider");

        host.engine.register_extension_instance(
            provider_instance.clone(),
            RuntimeScopeId::new(HOST_SCOPE),
            test_manifest("rintawa.test-content-handler"),
            vec![Box::new(ContentHandlerProvider {
                id: ComponentId::new("runtime"),
                contract,
                observed: Arc::clone(&observed),
            })],
        )?;
        host.engine.start_extension_instance(&provider_instance)?;

        let oversized_descriptor = vec![b'x'; MAX_CONTENT_HANDLER_ENTRY_BYTES + 1];
        let oversized = write_test_content_rtw(
            root.path(),
            "oversized-content",
            &content.to_string(),
            &oversized_descriptor,
        )?;
        let oversized_digest = ArtifactDigest::sha256(&std::fs::read(&oversized)?);
        assert!(matches!(
            host.import_user_content_rtw(&home, &oversized),
            Err(HostError::Artifact(_))
        ));
        assert!(
            home.artifact_store()
                .open_artifact(&oversized_digest)
                .is_err()
        );
        assert!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("content observer lock poisoned"))?
                .is_empty()
        );

        let rejected = write_test_content_rtw(
            root.path(),
            "rejected-content",
            &content.to_string(),
            b"reject",
        )?;
        let rejected_digest = ArtifactDigest::sha256(&std::fs::read(&rejected)?);
        assert!(matches!(
            host.import_user_content_rtw(&home, &rejected),
            Err(HostError::UserContentRejected { .. })
        ));
        assert!(home.list_user_content()?.is_empty());
        assert!(
            home.artifact_store()
                .open_artifact(&rejected_digest)
                .is_err()
        );

        let first_path = write_test_content_rtw(
            root.path(),
            "accepted-content-1",
            &content.to_string(),
            b"revision-one",
        )?;
        let first = host.import_user_content_rtw(&home, &first_path)?;
        home.artifact_store().verify(&first.revision)?;
        let stored_first = home
            .artifact_store()
            .root()
            .join("sha256")
            .join(format!("{}.rtw", first.revision.hex()));
        std::fs::write(stored_first, b"corrupted")?;
        assert!(matches!(
            home.list_user_content(),
            Err(HostError::Artifact(_))
        ));

        let second_path = write_test_content_rtw(
            root.path(),
            "accepted-content-2",
            &content.to_string(),
            b"revision-two",
        )?;
        let second = host.replace_user_content_rtw(&home, first.id, &second_path)?;
        home.artifact_store().verify(&second.revision)?;

        assert_eq!(second.id, first.id);
        assert_eq!(second.content, content);
        assert_ne!(second.revision, first.revision);
        assert_eq!(home.list_user_content()?, vec![second]);

        let observed = observed
            .lock()
            .map_err(|_| anyhow::anyhow!("content observer lock poisoned"))?;
        assert_eq!(observed.len(), 3);
        assert!(
            observed
                .iter()
                .all(|request| request.content == content.to_string())
        );
        assert_eq!(observed[0].descriptor, b"reject");
        assert_eq!(observed[1].descriptor, b"revision-one");
        assert_eq!(observed[2].descriptor, b"revision-two");
        Ok(())
    }

    #[test]
    fn test_should_bind_deferred_world_command_to_stable_local_principal() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let principal = home.local_principal();
        assert_eq!(HostHome::open(home.root())?.local_principal(), principal);

        let world = home.create_world()?;
        let world_sessions = WorldSessionRuntimeControl::default();
        let world_commands = WorldCommandRuntimeControl::default();
        let access = LocalHostAccess::new(
            home.root(),
            principal,
            world_sessions.clone(),
            world_commands.clone(),
        );
        assert_eq!(
            WorldCommandAccess::submit_world_command(
                &access,
                WorldCommandRequest {
                    world_id: world.id.to_string(),
                    schema: String::from("example.command@1"),
                    actor: WorldCommandActor::Principal,
                    expected_position: Some(0),
                    payload_json: br#"{"value":1}"#.to_vec(),
                },
            ),
            Err(WorldCommandAccessError::WorldNotActive)
        );

        world_sessions
            .request(world.id, true)
            .map_err(|error| anyhow::anyhow!("world-session request failed: {error:?}"))?;
        let accepted = WorldCommandAccess::submit_world_command(
            &access,
            WorldCommandRequest {
                world_id: world.id.to_string(),
                schema: String::from("example.command@1"),
                actor: WorldCommandActor::Principal,
                expected_position: Some(0),
                payload_json: br#"{"value":1}"#.to_vec(),
            },
        )
        .map_err(|error| anyhow::anyhow!("world-command submission failed: {error:?}"))?;
        let queued = world_commands.drain_pending()?;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].command.principal(), principal);
        assert_eq!(queued[0].command.actor(), ActorRef::Principal(principal));
        assert_eq!(queued[0].command.expected_position(), Some(0));
        assert_eq!(accepted.command_id, queued[0].command.id().to_string());
        assert_eq!(
            accepted.correlation_id,
            queued[0].command.correlation_id().to_string()
        );
        Ok(())
    }

    #[test]
    fn test_should_pump_deferred_world_command_through_authoritative_runtime() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world = home.create_world()?;
        let command_schema: SchemaKey = "example.deferred-command@1".parse()?;
        let event_schema: SchemaKey = "example.deferred-event@1".parse()?;
        let owner = ExtensionId::new("example.deferred-system");
        let storage = home.open_world_storage(world.id)?;
        storage.register_schema(&SchemaDefinition::new(
            command_schema.clone(),
            SchemaKind::Command,
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        ))?;
        storage.register_schema(&SchemaDefinition::new(
            event_schema.clone(),
            SchemaKind::Event,
            owner,
            serde_json::json!({ "type": "object" }),
        ))?;
        let outbox = home.open_world_storage(world.id)?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut builder = WorldRuntimeBuilder::new(storage);
        builder.register_system(RecordingWorldSystem {
            command_schema: command_schema.clone(),
            event_schema: event_schema.clone(),
            observed: Arc::clone(&observed),
        })?;
        let runtime = builder.start()?;

        let mut host = HostRuntime::start(&home)?;
        host.active_worlds.insert(
            world.id,
            ActiveWorld {
                runtime,
                outbox,
                effect_handlers: BTreeMap::new(),
                projections: BTreeMap::new(),
                signals: RuntimeSignalQueue::default(),
                registered_instances: Vec::new(),
                started_instances: Vec::new(),
            },
        );
        host.record_world_session_result(world.id, &Ok(()))?;
        let access = host
            .host_access
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("production host access must be attached"))?;
        let accepted = WorldCommandAccess::submit_world_command(
            access.as_ref(),
            WorldCommandRequest {
                world_id: world.id.to_string(),
                schema: command_schema.to_string(),
                actor: WorldCommandActor::Principal,
                expected_position: Some(0),
                payload_json: br#"{"kind":"deferred"}"#.to_vec(),
            },
        )
        .map_err(|error| anyhow::anyhow!("world-command submission failed: {error:?}"))?;
        assert_eq!(host.pump_world_command_requests()?, 1);

        let commands = observed
            .lock()
            .map_err(|_| anyhow::anyhow!("test command observer lock poisoned"))?;
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].principal(), home.local_principal());
        assert_eq!(
            commands[0].actor(),
            ActorRef::Principal(home.local_principal())
        );
        assert_eq!(commands[0].id().to_string(), accepted.command_id);
        drop(commands);
        let inspection = home.open_world_storage(world.id)?;
        assert_eq!(inspection.load_session()?.commit_position(), 1);
        let events = inspection.events_after(0, 8)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].schema, event_schema);
        assert_eq!(events[0].payload, serde_json::json!({ "kind": "deferred" }));
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_execute_extension_world_system_through_platform_service() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let world_id = WorldId::new();
        let storage = SqliteWorldStorage::create(root.path().join("world.sqlite"), world_id)?;
        let command_schema: SchemaKey = "rintawa.test.service-command@1".parse()?;
        let event_schema: SchemaKey = "rintawa.test.service-event@1".parse()?;
        let owner = ExtensionId::new("rintawa.test-system");
        storage.register_schema(&SchemaDefinition::new(
            command_schema.clone(),
            SchemaKind::Command,
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        ))?;
        storage.register_schema(&SchemaDefinition::new(
            event_schema.clone(),
            SchemaKind::Event,
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        ))?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine: ExtensionEngine::new(),
            started_instances: Vec::new(),
            host_shell_provider: None,
            ui_layer_provider: None,
            host_access: None,
        };
        let system = host.bind_world_system(world_id, command_schema.clone(), owner)?;
        let contract = world_system_service_contract_key(&command_schema);
        let scope = world_runtime_scope_id(world_id);

        let hijacker_instance = ExtensionInstanceId::new("a-hijacker");
        host.engine.register_extension_instance(
            hijacker_instance.clone(),
            scope.clone(),
            test_manifest("rintawa.hijacker"),
            vec![Box::new(RejectingWorldSystemProvider {
                id: ComponentId::new("runtime"),
                contract: contract.clone(),
            })],
        )?;
        host.engine.start_extension_instance(&hijacker_instance)?;
        host.started_instances.push(hijacker_instance);

        let provider_instance = ExtensionInstanceId::new("z-world-system-provider");
        host.engine.register_extension_instance(
            provider_instance.clone(),
            scope,
            test_manifest("rintawa.test-system"),
            vec![Box::new(WorldSystemProvider {
                id: ComponentId::new("runtime"),
                contract,
                event_schema: event_schema.clone(),
            })],
        )?;
        host.engine.start_extension_instance(&provider_instance)?;
        host.started_instances.push(provider_instance);

        let mut builder = WorldRuntimeBuilder::new(storage);
        builder.register_system(system)?;
        let world_runtime = builder.start()?;
        let principal = PrincipalId::new();
        let outcome = world_runtime
            .submit(WorldCommand::new(
                command_schema,
                principal,
                ActorRef::Principal(principal),
                serde_json::json!({ "kind": "extension-system" }),
            ))?
            .wait_outcome()?;

        assert_eq!(outcome.receipt().position(), 1);
        assert_eq!(outcome.committed_events().len(), 1);
        assert_eq!(outcome.committed_events()[0].schema, event_schema);
        assert_eq!(
            outcome.committed_events()[0].payload,
            serde_json::json!({ "kind": "extension-system" })
        );

        world_runtime.shutdown()?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_route_world_projection_only_to_schema_owner() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("world.sqlite");
        let world_id = WorldId::new();
        let storage = SqliteWorldStorage::create(&path, world_id)?;
        let projection_schema: SchemaKey = "rintawa.test.owner-view@1".parse()?;
        let owner = ExtensionId::new("rintawa.test-projection");
        storage.register_schema(&SchemaDefinition::new(
            projection_schema.clone(),
            SchemaKind::Projection,
            owner.clone(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "provider": { "const": "owner" },
                    "principal": { "type": "string" }
                },
                "required": ["provider", "principal"],
                "additionalProperties": false
            }),
        ))?;
        let outbox = SqliteWorldStorage::open(&path)?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine: ExtensionEngine::new(),
            started_instances: Vec::new(),
            host_shell_provider: None,
            ui_layer_provider: None,
            host_access: None,
        };
        let projection =
            host.bind_world_projection(world_id, projection_schema.clone(), owner.clone())?;
        let contract = world_projection_service_contract_key(&projection_schema);
        let scope = world_runtime_scope_id(world_id);
        let hijacker_observed = Arc::new(Mutex::new(Vec::new()));
        let owner_observed = Arc::new(Mutex::new(Vec::new()));

        let hijacker_instance = ExtensionInstanceId::new("a-projection-hijacker");
        host.engine.register_extension_instance(
            hijacker_instance.clone(),
            scope.clone(),
            test_manifest("rintawa.projection-hijacker"),
            vec![Box::new(WorldProjectionProvider {
                id: ComponentId::new("runtime"),
                contract: contract.clone(),
                reject: true,
                observed: Arc::clone(&hijacker_observed),
            })],
        )?;
        host.engine.start_extension_instance(&hijacker_instance)?;

        let owner_instance = ExtensionInstanceId::new("z-projection-owner");
        host.engine.register_extension_instance(
            owner_instance.clone(),
            scope,
            test_manifest(owner.as_str()),
            vec![Box::new(WorldProjectionProvider {
                id: ComponentId::new("runtime"),
                contract,
                reject: false,
                observed: Arc::clone(&owner_observed),
            })],
        )?;
        host.engine.start_extension_instance(&owner_instance)?;

        let runtime = WorldRuntimeBuilder::new(storage).start()?;
        host.active_worlds.insert(
            world_id,
            ActiveWorld {
                runtime,
                outbox,
                effect_handlers: BTreeMap::new(),
                projections: BTreeMap::from([(projection_schema.clone(), projection)]),
                signals: RuntimeSignalQueue::default(),
                registered_instances: vec![hijacker_instance.clone(), owner_instance.clone()],
                started_instances: vec![hijacker_instance, owner_instance],
            },
        );

        let principal = PrincipalId::new();
        let view = host.project_world(
            world_id,
            principal,
            &projection_schema,
            serde_json::json!({ "kind": "test" }),
        )?;
        assert_eq!(view.principal(), principal);
        assert_eq!(view.value()["provider"], "owner");
        assert_eq!(view.value()["principal"], principal.to_string());
        assert!(
            hijacker_observed
                .lock()
                .map_err(|_| anyhow::anyhow!("hijacker observer lock poisoned"))?
                .is_empty()
        );
        assert_eq!(
            owner_observed
                .lock()
                .map_err(|_| anyhow::anyhow!("owner observer lock poisoned"))?
                .len(),
            1
        );

        host.deactivate_world(world_id)?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_supervise_world_commit_delivery_replay_and_deactivation() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let created = home.create_world()?;
        let world_id = created.id;
        let command_schema: SchemaKey = "rintawa.test.supervised-command@1".parse()?;
        let event_schema: SchemaKey = "rintawa.test.supervised-event@1".parse()?;
        let owner = ExtensionId::new("rintawa.supervised-system");
        let storage = home.open_world_storage(world_id)?;
        storage.register_schema(&SchemaDefinition::new(
            command_schema.clone(),
            SchemaKind::Command,
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        ))?;
        storage.register_schema(&SchemaDefinition::new(
            event_schema.clone(),
            SchemaKind::Event,
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        ))?;
        drop(storage);

        let mut host = HostRuntime::start(&home)?;
        host.activate_world(&home, world_id)?;
        assert!(host.is_world_active(world_id));
        assert!(matches!(
            host.activate_world(&home, world_id),
            Err(HostError::WorldAlreadyActive(id)) if id == world_id
        ));

        let scope = world_runtime_scope_id(world_id);
        let contract = world_system_service_contract_key(&command_schema);
        let provider_instance = ExtensionInstanceId::new("supervised-system-provider");
        host.engine.register_extension_instance(
            provider_instance.clone(),
            scope.clone(),
            test_manifest(owner.as_str()),
            vec![Box::new(WorldSystemProvider {
                id: ComponentId::new("runtime"),
                contract,
                event_schema: event_schema.clone(),
            })],
        )?;
        host.engine.start_extension_instance(&provider_instance)?;
        host.started_instances.push(provider_instance);

        let observed = Arc::new(Mutex::new(Vec::new()));
        let subscriber_instance = ExtensionInstanceId::new("supervised-event-subscriber");
        host.engine.register_extension_instance(
            subscriber_instance.clone(),
            scope,
            test_manifest("rintawa.supervised-subscriber"),
            vec![Box::new(WorldEventSubscriber {
                id: ComponentId::new("runtime"),
                topic: event_schema.to_string(),
                observed: Arc::clone(&observed),
            })],
        )?;
        host.engine.start_extension_instance(&subscriber_instance)?;
        host.started_instances.push(subscriber_instance);

        let principal = PrincipalId::new();
        let command = WorldCommand::new(
            command_schema.clone(),
            principal,
            ActorRef::Principal(principal),
            serde_json::json!({ "kind": "supervised" }),
        );
        let first = host.submit_world_command(world_id, command.clone())?;
        assert_eq!(
            first.authoritative().receipt().disposition(),
            CommitDisposition::Committed
        );
        assert_eq!(first.authoritative().receipt().position(), 1);
        assert!(matches!(first.live_delivery(), Ok(1)));
        assert_eq!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("event observer lock poisoned"))?
                .len(),
            1
        );

        let replay = host.submit_world_command(world_id, command)?;
        assert_eq!(
            replay.authoritative().receipt().disposition(),
            CommitDisposition::AlreadyCommitted
        );
        assert!(matches!(replay.live_delivery(), Ok(0)));
        assert_eq!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("event observer lock poisoned"))?
                .len(),
            1
        );

        host.deactivate_world(world_id)?;
        assert!(!host.is_world_active(world_id));
        let next = WorldCommand::new(
            command_schema,
            principal,
            ActorRef::Principal(principal),
            serde_json::json!({ "kind": "after-stop" }),
        );
        assert!(matches!(
            host.submit_world_command(world_id, next),
            Err(HostError::WorldNotActive(id)) if id == world_id
        ));
        assert!(matches!(
            host.deactivate_world(world_id),
            Err(HostError::WorldNotActive(id)) if id == world_id
        ));

        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_retry_then_complete_owner_pinned_effect_job() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let seed_schema: SchemaKey = "rintawa.test.effect-seed@1".parse()?;
        let result_schema: SchemaKey = "rintawa.test.effect-result@1".parse()?;
        let event_schema: SchemaKey = "rintawa.test.effect-result-event@1".parse()?;
        let effect_schema: SchemaKey = "rintawa.test.external-effect@1".parse()?;
        let result_owner = ExtensionId::new("rintawa.effect-result-system");
        let effect_owner = ExtensionId::new("rintawa.effect-provider");
        let storage = home.open_world_storage(world_id)?;
        storage.register_schemas(&[
            SchemaDefinition::new(
                seed_schema.clone(),
                SchemaKind::Command,
                ExtensionId::new("rintawa.effect-seed"),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                result_schema.clone(),
                SchemaKind::Command,
                result_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                event_schema.clone(),
                SchemaKind::Event,
                result_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                effect_schema.clone(),
                SchemaKind::Effect,
                effect_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
        ])?;
        let principal = PrincipalId::new();
        let (effect_id, correlation_id) =
            enqueue_test_effect(&storage, &seed_schema, &effect_schema, principal)?;
        drop(storage);

        let mut host = HostRuntime::start(&home)?;
        host.activate_world(&home, world_id)?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        register_test_effect_routes(
            &mut host,
            world_id,
            TestEffectRoutes {
                result_owner: &result_owner,
                result_schema: &result_schema,
                event_schema: &event_schema,
                effect_owner: &effect_owner,
                effect_schema: &effect_schema,
                remaining_retries: 1,
                observed: Arc::clone(&observed),
            },
        )?;

        assert_eq!(
            host.pump_effect_outbox_at(world_id, UnixTimeMillis::new(100))?,
            1
        );
        let inspection = home.open_world_storage(world_id)?;
        assert_eq!(
            inspection.next_effect_wakeup()?,
            Some(UnixTimeMillis::new(1_100))
        );
        assert_eq!(inspection.load_session()?.commit_position(), 1);
        assert_eq!(
            host.pump_effect_outbox_at(world_id, UnixTimeMillis::new(1_099))?,
            0
        );
        assert_eq!(
            host.pump_effect_outbox_at(world_id, UnixTimeMillis::new(1_100))?,
            1
        );
        assert_eq!(inspection.load_session()?.commit_position(), 2);
        assert!(
            inspection
                .claim_next_effect(UnixTimeMillis::new(2_000), UnixTimeMillis::new(3_000))?
                .is_none()
        );
        let events = inspection.events_after(0, 16)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].schema, event_schema);
        assert_eq!(
            events[0].provenance.causation,
            Some(CausationRef::Effect(effect_id))
        );
        assert_eq!(events[0].provenance.correlation_id, correlation_id);
        assert_eq!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("effect observer lock poisoned"))?
                .len(),
            2
        );

        host.deactivate_world(world_id)?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_recover_effect_after_follow_up_commit_without_duplicate_world_commit()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let seed_schema: SchemaKey = "rintawa.test.crash-seed@1".parse()?;
        let result_schema: SchemaKey = "rintawa.test.crash-result@1".parse()?;
        let event_schema: SchemaKey = "rintawa.test.crash-result-event@1".parse()?;
        let effect_schema: SchemaKey = "rintawa.test.crash-effect@1".parse()?;
        let result_owner = ExtensionId::new("rintawa.crash-result-system");
        let effect_owner = ExtensionId::new("rintawa.crash-effect-provider");
        let storage = home.open_world_storage(world_id)?;
        storage.register_schemas(&[
            SchemaDefinition::new(
                seed_schema.clone(),
                SchemaKind::Command,
                ExtensionId::new("rintawa.crash-seed"),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                result_schema.clone(),
                SchemaKind::Command,
                result_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                event_schema.clone(),
                SchemaKind::Event,
                result_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
            SchemaDefinition::new(
                effect_schema.clone(),
                SchemaKind::Effect,
                effect_owner.clone(),
                serde_json::json!({ "type": "object" }),
            ),
        ])?;
        let principal = PrincipalId::new();
        let (effect_id, _) =
            enqueue_test_effect(&storage, &seed_schema, &effect_schema, principal)?;
        drop(storage);

        let mut host = HostRuntime::start(&home)?;
        host.activate_world(&home, world_id)?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        register_test_effect_routes(
            &mut host,
            world_id,
            TestEffectRoutes {
                result_owner: &result_owner,
                result_schema: &result_schema,
                event_schema: &event_schema,
                effect_owner: &effect_owner,
                effect_schema: &effect_schema,
                remaining_retries: 0,
                observed: Arc::clone(&observed),
            },
        )?;

        let claim = host.active_worlds[&world_id]
            .outbox
            .claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(200))?
            .expect("effect must be claimable");
        assert_eq!(claim.attempt(), 1);
        let response =
            host.active_worlds[&world_id].effect_handlers[&effect_schema].execute(&claim)?;
        let WorldEffectServiceResponse::Complete {
            command: Some(command),
        } = response
        else {
            anyhow::bail!("effect provider must complete with a command");
        };
        let first = host.submit_world_command(world_id, command)?;
        assert_eq!(
            first.authoritative().receipt().disposition(),
            CommitDisposition::Committed
        );
        assert_eq!(first.authoritative().receipt().position(), 2);

        // Simulate a process crash after the command commit but before complete_effect.
        assert_eq!(
            host.pump_effect_outbox_at(world_id, UnixTimeMillis::new(200))?,
            1
        );
        let inspection = home.open_world_storage(world_id)?;
        assert_eq!(inspection.load_session()?.commit_position(), 2);
        assert_eq!(inspection.events_after(0, 16)?.len(), 1);
        assert!(
            inspection
                .claim_next_effect(UnixTimeMillis::new(300), UnixTimeMillis::new(400))?
                .is_none()
        );
        let requests = observed
            .lock()
            .map_err(|_| anyhow::anyhow!("effect observer lock poisoned"))?;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].job().id, effect_id);
        assert_eq!(requests[0].job().attempt_count, 1);
        assert_eq!(requests[1].job().attempt_count, 2);
        drop(requests);

        host.deactivate_world(world_id)?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_queue_and_dispatch_ephemeral_runtime_signals_with_backpressure()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let world_id = WorldId::new();
        let storage = SqliteWorldStorage::create(root.path().join("world.sqlite"), world_id)?;
        let outbox = SqliteWorldStorage::open(root.path().join("world.sqlite"))?;
        let world_runtime = WorldRuntimeBuilder::new(storage).start()?;
        let scope = world_runtime_scope_id(world_id);
        let instance = ExtensionInstanceId::new("runtime-signal-subscriber");
        let topic = String::from("rintawa.operation.chunk@1");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            instance.clone(),
            scope.clone(),
            test_manifest("runtime-signal-subscriber"),
            vec![Box::new(RuntimeSignalSubscriber {
                id: ComponentId::new("runtime"),
                topic: topic.clone(),
                observed: Arc::clone(&observed),
            })],
        )?;
        engine.start_extension_instance(&instance)?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::from([(
                world_id,
                ActiveWorld {
                    runtime: world_runtime,
                    outbox,
                    effect_handlers: BTreeMap::new(),
                    projections: BTreeMap::new(),
                    signals: RuntimeSignalQueue::default(),
                    registered_instances: vec![instance.clone()],
                    started_instances: vec![instance],
                },
            )]),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            ui_layer_provider: None,
            host_access: None,
        };

        let correlation = CorrelationId::new();
        host.enqueue_runtime_signal(
            RuntimeSignal::new(world_id, topic.clone(), b"first".to_vec())
                .with_correlation_id(correlation),
        )?;
        host.enqueue_runtime_signal(RuntimeSignal::new(
            world_id,
            topic.clone(),
            b"second".to_vec(),
        ))?;
        assert_eq!(host.active_worlds[&world_id].signals.len(), 2);
        assert_eq!(host.pump_runtime_signals(world_id)?, 2);
        let values = observed
            .lock()
            .map_err(|_| anyhow::anyhow!("signal observer lock poisoned"))?
            .clone();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].payload(), b"first");
        assert_eq!(values[0].correlation_id(), Some(correlation));
        assert_eq!(values[1].payload(), b"second");
        assert_eq!(values[1].correlation_id(), None);

        host.enqueue_runtime_signal(RuntimeSignal::new(
            world_id,
            topic.clone(),
            b"from-poll".to_vec(),
        ))?;
        host.poll_runtime()?;
        assert_eq!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("signal observer lock poisoned"))?
                .len(),
            3
        );

        assert!(matches!(
            host.enqueue_runtime_signal(RuntimeSignal::new(world_id, "   ", Vec::new())),
            Err(HostError::InvalidRuntimeSignalTopic)
        ));
        assert!(matches!(
            host.enqueue_runtime_signal(RuntimeSignal::new(
                world_id,
                topic.clone(),
                vec![0_u8; crate::MAX_RUNTIME_SIGNAL_MESSAGE_BYTES],
            )),
            Err(HostError::RuntimeSignalMessageTooLarge { .. })
        ));

        for index in 0..crate::DEFAULT_RUNTIME_SIGNAL_QUEUE_CAPACITY {
            host.enqueue_runtime_signal(RuntimeSignal::new(
                world_id,
                topic.clone(),
                index.to_le_bytes().to_vec(),
            ))?;
        }
        assert!(matches!(
            host.enqueue_runtime_signal(RuntimeSignal::new(world_id, topic, b"overflow".to_vec())),
            Err(HostError::RuntimeSignalQueueFull(id)) if id == world_id
        ));

        host.deactivate_world(world_id)?;
        assert!(matches!(
            host.enqueue_runtime_signal(RuntimeSignal::new(
                world_id,
                "rintawa.operation.chunk@1",
                b"after-stop".to_vec(),
            )),
            Err(HostError::WorldNotActive(id)) if id == world_id
        ));
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_deliver_full_durable_world_event_envelope() -> anyhow::Result<()> {
        let world_id = WorldId::new();
        let scope = world_runtime_scope_id(world_id);
        let instance = ExtensionInstanceId::new("world-event-subscriber");
        let schema: rintawa_sdk::world::SchemaKey = "rintawa.test.event@1".parse()?;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            instance.clone(),
            scope.clone(),
            test_manifest("world-event-subscriber"),
            vec![Box::new(WorldEventSubscriber {
                id: ComponentId::new("runtime"),
                topic: schema.to_string(),
                observed: Arc::clone(&observed),
            })],
        )?;
        engine.start_extension_instance(&instance)?;

        let principal = PrincipalId::new();
        let event = StoredWorldEvent {
            world_id,
            id: WorldEventId::new(),
            commit_position: 7,
            event_index: 0,
            provenance: CommandProvenance {
                command_id: CommandId::new(),
                command_schema: "rintawa.test.command@1".parse()?,
                principal,
                actor: ActorRef::Principal(principal),
                causation: None,
                correlation_id: CorrelationId::new(),
                effective_at: Some(UnixTimeMillis::new(1234)),
                recorded_at: UnixTimeMillis::new(5678),
            },
            schema: schema.clone(),
            payload: serde_json::json!({ "message": "hello" }),
        };

        let mut runtime = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: vec![instance],
            host_shell_provider: None,
            ui_layer_provider: None,
            host_access: None,
        };

        let mut foreign_event = event.clone();
        foreign_event.world_id = WorldId::new();
        assert!(matches!(
            runtime.dispatch_world_events(&[event.clone(), foreign_event]),
            Err(HostError::MixedWorldEventBatch)
        ));
        assert!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("event observer lock poisoned"))?
                .is_empty()
        );

        assert_eq!(
            runtime.dispatch_world_events(std::slice::from_ref(&event))?,
            1
        );
        let deliveries = observed
            .lock()
            .map_err(|_| anyhow::anyhow!("event observer lock poisoned"))?;
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].0, schema.to_string());
        assert_eq!(deliveries[0].1, event);
        drop(deliveries);

        runtime.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_expand_bootstrap_root_to_required_contract_dependencies() -> anyhow::Result<()> {
        let scope = RuntimeScopeId::new("host");
        let contract = ContractKey::new("example.bootstrap-service", ContractVersion::new(1));
        let root = ExtensionInstanceId::new("runtime-provider-root");
        let provider = ExtensionInstanceId::new("service-provider");
        let definition = ExtensionInstanceId::new("service-definition");
        let unrelated = ExtensionInstanceId::new("unrelated");
        let mut engine = ExtensionEngine::new();

        let mut root_component = ContractComponent::new("runtime");
        root_component.consumer = Some(ContractConsumer::new(contract.clone(), true));
        engine.register_extension_instance(
            root.clone(),
            scope.clone(),
            test_manifest("runtime-provider-root"),
            vec![Box::new(root_component)],
        )?;

        let mut provider_component = ContractComponent::new("runtime");
        provider_component.provider = Some(ContractProvider::new(contract.clone()));
        engine.register_extension_instance(
            provider.clone(),
            scope.clone(),
            test_manifest("service-provider"),
            vec![Box::new(provider_component)],
        )?;

        let mut definition_component = ContractComponent::new("runtime");
        definition_component.definition = Some(ContractDefinition::new(
            contract,
            ContractResolutionPolicy::Single,
        ));
        engine.register_extension_instance(
            definition.clone(),
            scope.clone(),
            test_manifest("service-definition"),
            vec![Box::new(definition_component)],
        )?;
        engine.register_extension_instance(
            unrelated.clone(),
            scope.clone(),
            test_manifest("unrelated"),
            Vec::new(),
        )?;

        let error = engine
            .plan_extension_activation(std::slice::from_ref(&root))
            .expect_err("root alone must not resolve its required contract");
        let EngineError::ActivationPlan(reason) = error else {
            anyhow::bail!("expected activation-plan error");
        };
        let registered = HashSet::from([
            root.clone(),
            provider.clone(),
            definition.clone(),
            unrelated.clone(),
        ]);
        let mut bootstrap = HashSet::from([root.clone()]);

        assert!(expand_bootstrap_dependencies(
            &engine,
            &scope,
            &root,
            &reason,
            &registered,
            &mut bootstrap,
        ));
        assert!(bootstrap.contains(&provider));
        assert!(bootstrap.contains(&definition));
        assert!(!bootstrap.contains(&unrelated));
        Ok(())
    }

    struct FailingStopComponent {
        id: ComponentId,
    }

    impl Component for FailingStopComponent {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn stop(
            &mut self,
            _ctx: &mut dyn rintawa_sdk::context::ComponentContext,
        ) -> ExtensionResult<()> {
            Err(ExtensionError::Message(String::from("test stop failure")))
        }
    }

    #[test]
    fn test_should_aggregate_all_host_cleanup_failures() -> anyhow::Result<()> {
        let scope = RuntimeScopeId::new("host");
        let first = ExtensionInstanceId::new("first");
        let second = ExtensionInstanceId::new("second");
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            first.clone(),
            scope.clone(),
            test_manifest("first"),
            vec![Box::new(FailingStopComponent {
                id: ComponentId::new("runtime"),
            })],
        )?;
        engine.register_extension_instance(
            second.clone(),
            scope,
            test_manifest("second"),
            vec![Box::new(FailingStopComponent {
                id: ComponentId::new("runtime"),
            })],
        )?;
        engine.start_extension_instance(&first)?;
        engine.start_extension_instance(&second)?;

        let failures = cleanup_instances(
            &mut engine,
            &[first.clone(), second.clone()],
            &[first.clone(), second.clone()],
        );

        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].instance_id, second);
        assert_eq!(failures[0].operation, HostCleanupOperation::Stop);
        assert_eq!(failures[1].instance_id, first);
        assert_eq!(failures[1].operation, HostCleanupOperation::Stop);
        Ok(())
    }
}
