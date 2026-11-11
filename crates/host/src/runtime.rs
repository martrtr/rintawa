//! Runtime orchestration for exact artifact activations in a local Rintawa host.

use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rintawa_artifacts::{ArtifactDigest, AssetDigest, AssetMediaType, AssetRef};
use rintawa_extension_engine::{
    ActivationPlanError, ArtifactStoreAccess, AssetStoreAccess, CompositionAccess,
    CompositionActivation, EngineError, ExtensionEngine, HostAccessError, HostAccessResult,
    ImportedArtifact, ImportedAsset, PreferenceAccess, RtwExtensionLoadOutcome, RtwExtensionLoader,
    RuntimeArtifactPolicy, RuntimeContextAccess, RuntimeHttpOriginGrant, RuntimePolicyAccess,
    RuntimePolicyComponent, RuntimePolicyRequest, UnresolvedContractReason, UserResourceAccess,
    UserResourceRef, WasmWorldSystemEndpoint, WorldDataAccess, WorldSessionAccess,
    WorldSessionAssetRef, WorldSessionSummary,
};
use rintawa_sdk::{
    contracts::{
        ComponentRef, ContractKey, ContractResolutionPolicy, ContractVersion,
        host_shell_contract_key,
    },
    runtime_permissions::RuntimePermission,
    types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId},
    world::{PrincipalId, SchemaKey, WorldId},
};
use rintawa_world::{PropertyDefinition, RuntimeSchemaDefinition, SchemaKind, WorldCommand};
use rintawa_world_runtime::{
    SystemResult, WorldCommandOutcome, WorldRuntime, WorldRuntimeBuilder, WorldRuntimeClient,
    WorldSnapshot, WorldSystem,
};

use uuid::Uuid;

use crate::{
    BootstrapBlockedActivation, BootstrapDeferredActivation, BootstrapRollback, BootstrapStall,
    CompositionProfile, HOST_SCOPE, HostCleanupFailure, HostCleanupOperation, HostError, HostHome,
    HostResult, HostShutdownFailures, WorldRuntimeCleanupFailure, WorldSummary,
    world_runtime_scope_id,
};

/// Maximum one-shot user resources retained by one HostRuntime.
const MAX_USER_RESOURCES: usize = 32;
/// Maximum bytes accepted for one ephemeral user resource.
const MAX_USER_RESOURCE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum total bytes retained across all ephemeral user resources.
const MAX_USER_RESOURCE_TOTAL_BYTES: usize = 32 * 1024 * 1024;
/// Maximum UTF-8 bytes retained for one resource media type.
const MAX_USER_RESOURCE_MEDIA_TYPE_BYTES: usize = 128;
/// Maximum UTF-8 bytes retained for one optional original file name.
const MAX_USER_RESOURCE_NAME_BYTES: usize = 512;
/// Maximum distinct world lifecycle requests accepted before a host pump drains them.
const MAX_PENDING_WORLD_SESSION_REQUESTS: usize = 64;
/// Maximum diagnostic bytes retained for one failed deferred world lifecycle request.
const MAX_WORLD_SESSION_DIAGNOSTIC_BYTES: usize = 2 * 1024;

#[derive(Default)]
struct WorldSessionRuntimeState {
    active: HashSet<WorldId>,
    pending: BTreeMap<WorldId, bool>,
    last_errors: BTreeMap<WorldId, String>,
}

#[derive(Default)]
struct WorldDataRuntimeState {
    worlds: BTreeMap<WorldId, ActiveWorldData>,
}

struct ActiveWorldData {
    client: WorldRuntimeClient,
    command_owners: BTreeMap<SchemaKey, ComponentRef>,
}

#[derive(Clone, Default)]
struct WorldDataRuntimeControl {
    state: Arc<Mutex<WorldDataRuntimeState>>,
}

impl WorldDataRuntimeControl {
    fn activate(
        &self,
        world_id: WorldId,
        client: WorldRuntimeClient,
        command_owners: BTreeMap<SchemaKey, ComponentRef>,
    ) -> HostResult<()> {
        self.state
            .lock()
            .map_err(|_| HostError::WorldSessionControlUnavailable)?
            .worlds
            .insert(
                world_id,
                ActiveWorldData {
                    client,
                    command_owners,
                },
            );
        Ok(())
    }

    fn deactivate(&self, world_id: WorldId) {
        if let Ok(mut state) = self.state.lock() {
            state.worlds.remove(&world_id);
        }
    }

    fn client(&self, scope: &RuntimeScopeId) -> HostAccessResult<WorldRuntimeClient> {
        let Some(world_id) = canonical_scope_world_id(scope) else {
            return Err(HostAccessError::Unavailable);
        };
        self.state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?
            .worlds
            .get(&world_id)
            .map(|world| world.client.clone())
            .ok_or(HostAccessError::Unavailable)
    }

    fn command_client(
        &self,
        scope: &RuntimeScopeId,
        owner: &ComponentRef,
        schema: &SchemaKey,
    ) -> HostAccessResult<WorldRuntimeClient> {
        let Some(world_id) = canonical_scope_world_id(scope) else {
            return Err(HostAccessError::Unavailable);
        };
        let state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let world = state
            .worlds
            .get(&world_id)
            .ok_or(HostAccessError::Unavailable)?;
        if world.command_owners.get(schema) != Some(owner) {
            return Err(HostAccessError::Rejected);
        }
        Ok(world.client.clone())
    }
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
            title: world.title,
            description: world.description,
            cover: world.cover.map(|reference| WorldSessionAssetRef {
                digest: reference.digest.to_string(),
                size: reference.size,
                media_type: reference.media_type.to_string(),
            }),
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

    fn delete_inactive<F>(&self, world_id: WorldId, delete: F) -> HostAccessResult<()>
    where
        F: FnOnce() -> HostResult<()>,
    {
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        if state.active.contains(&world_id) || state.pending.contains_key(&world_id) {
            return Err(HostAccessError::Rejected);
        }
        delete().map_err(map_host_access_error)?;
        state.last_errors.remove(&world_id);
        Ok(())
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

#[derive(Clone)]
struct StoredUserResource {
    reference: UserResourceRef,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct UserResourceRuntimeState {
    resources: BTreeMap<String, StoredUserResource>,
    total_bytes: usize,
}

#[derive(Clone, Default)]
struct UserResourceRuntimeControl {
    state: Arc<Mutex<UserResourceRuntimeState>>,
}

impl UserResourceRuntimeControl {
    fn import(
        &self,
        bytes: &[u8],
        media_type: &str,
        name: Option<&str>,
    ) -> HostAccessResult<UserResourceRef> {
        if bytes.is_empty() || bytes.len() > MAX_USER_RESOURCE_BYTES {
            return Err(HostAccessError::Rejected);
        }
        if media_type.is_empty()
            || media_type.len() > MAX_USER_RESOURCE_MEDIA_TYPE_BYTES
            || media_type != media_type.to_ascii_lowercase()
            || !media_type.contains('/')
        {
            return Err(HostAccessError::Rejected);
        }
        if name.is_some_and(|value| {
            value.trim().is_empty()
                || value.len() > MAX_USER_RESOURCE_NAME_BYTES
                || value.contains(['/', '\\'])
        }) {
            return Err(HostAccessError::Rejected);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let next_total = state.total_bytes.saturating_add(bytes.len());
        if state.resources.len() >= MAX_USER_RESOURCES || next_total > MAX_USER_RESOURCE_TOTAL_BYTES
        {
            return Err(HostAccessError::QueueFull);
        }
        let id = Uuid::now_v7().to_string();
        if state.resources.contains_key(&id) {
            return Err(HostAccessError::Rejected);
        }
        let size = u64::try_from(bytes.len()).map_err(|_| HostAccessError::Rejected)?;
        let reference = UserResourceRef {
            id: id.clone(),
            size,
            media_type: media_type.to_string(),
            name: name.map(str::to_string),
        };
        state.resources.insert(
            id,
            StoredUserResource {
                reference: reference.clone(),
                bytes: bytes.to_vec(),
            },
        );
        state.total_bytes = next_total;
        Ok(reference)
    }

    fn read(&self, reference: &UserResourceRef, maximum_bytes: usize) -> HostAccessResult<Vec<u8>> {
        let state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let resource = state
            .resources
            .get(&reference.id)
            .ok_or(HostAccessError::NotFound)?;
        if resource.reference != *reference {
            return Err(HostAccessError::Rejected);
        }
        if resource.bytes.len() > maximum_bytes {
            return Err(HostAccessError::Rejected);
        }
        Ok(resource.bytes.clone())
    }

    fn release(&self, reference: &UserResourceRef) -> HostAccessResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let resource = state
            .resources
            .get(&reference.id)
            .ok_or(HostAccessError::NotFound)?;
        if resource.reference != *reference {
            return Err(HostAccessError::Rejected);
        }
        let size = resource.bytes.len();
        state.resources.remove(&reference.id);
        state.total_bytes = state.total_bytes.saturating_sub(size);
        Ok(())
    }
}

struct LocalHostAccess {
    home_root: PathBuf,
    user_resources: UserResourceRuntimeControl,
    world_sessions: WorldSessionRuntimeControl,
}

impl LocalHostAccess {
    fn new(
        home_root: &Path,
        user_resources: UserResourceRuntimeControl,
        world_sessions: WorldSessionRuntimeControl,
    ) -> Self {
        Self {
            home_root: home_root.to_path_buf(),
            user_resources,
            world_sessions,
        }
    }

    fn home(&self) -> HostAccessResult<HostHome> {
        HostHome::open(&self.home_root).map_err(map_host_access_error)
    }
}

impl RuntimeContextAccess for LocalHostAccess {
    fn world_id_for_scope(&self, scope_id: &RuntimeScopeId) -> HostAccessResult<Option<WorldId>> {
        let Some(value) = scope_id.as_str().strip_prefix(crate::WORLD_SCOPE_PREFIX) else {
            return Ok(None);
        };
        let Ok(world_id) = value.parse::<WorldId>() else {
            return Ok(None);
        };
        if world_runtime_scope_id(world_id) != *scope_id {
            return Ok(None);
        }
        Ok(Some(world_id))
    }
}

fn canonical_scope_world_id(scope_id: &RuntimeScopeId) -> Option<WorldId> {
    let value = scope_id.as_str().strip_prefix(crate::WORLD_SCOPE_PREFIX)?;
    let world_id = value.parse::<WorldId>().ok()?;
    (world_runtime_scope_id(world_id) == *scope_id).then_some(world_id)
}

impl WorldDataAccess for WorldDataRuntimeControl {
    fn load_item(
        &self,
        scope: &RuntimeScopeId,
        owner: &ExtensionId,
        item: rintawa_sdk::world::ItemId,
    ) -> HostAccessResult<Option<String>> {
        let value = self
            .client(scope)?
            .snapshot_for_extension(owner.clone())
            .map_err(|_| HostAccessError::Unavailable)?
            .load_item(item)
            .map_err(|_| HostAccessError::Unavailable)?;
        value
            .map(|item| serde_json::to_string(&item).map_err(|_| HostAccessError::Unavailable))
            .transpose()
    }

    fn load_property(
        &self,
        scope: &RuntimeScopeId,
        owner: &ExtensionId,
        item: rintawa_sdk::world::ItemId,
        key: &SchemaKey,
    ) -> HostAccessResult<Option<String>> {
        let value = self
            .client(scope)?
            .snapshot_for_extension(owner.clone())
            .map_err(|_| HostAccessError::Unavailable)?
            .load_property(item, key)
            .map_err(|error| match error {
                rintawa_world_runtime::WorldReadError::PropertyAccessDenied(_) => {
                    HostAccessError::Rejected
                }
                _ => HostAccessError::Unavailable,
            })?;
        value
            .map(|property| {
                serde_json::to_string(&property).map_err(|_| HostAccessError::Unavailable)
            })
            .transpose()
    }

    fn find_properties(
        &self,
        scope: &RuntimeScopeId,
        owner: &ExtensionId,
        key: &SchemaKey,
        after: Option<rintawa_sdk::world::ItemId>,
        limit: usize,
    ) -> HostAccessResult<Vec<String>> {
        if !(1..=256).contains(&limit) {
            return Err(HostAccessError::Rejected);
        }
        self.client(scope)?
            .snapshot_for_extension(owner.clone())
            .map_err(|_| HostAccessError::Unavailable)?
            .find_properties(key, after, limit)
            .map_err(|error| match error {
                rintawa_world_runtime::WorldReadError::PropertyAccessDenied(_) => {
                    HostAccessError::Rejected
                }
                _ => HostAccessError::Unavailable,
            })?
            .into_iter()
            .map(|property| {
                serde_json::to_string(&property).map_err(|_| HostAccessError::Unavailable)
            })
            .collect()
    }

    fn submit(
        &self,
        scope: &RuntimeScopeId,
        owner: &ComponentRef,
        schema: SchemaKey,
        payload: serde_json::Value,
    ) -> HostAccessResult<rintawa_world_runtime::WorldCommandTicket> {
        self.command_client(scope, owner, &schema)?
            .submit(WorldCommand::system(schema, payload))
            .map_err(|error| match error {
                rintawa_world_runtime::WorldRuntimeError::QueueFull => HostAccessError::QueueFull,
                rintawa_world_runtime::WorldRuntimeError::RuntimeStopped => {
                    HostAccessError::Unavailable
                }
                _ => HostAccessError::Rejected,
            })
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

    fn read_asset(
        &self,
        digest: &str,
        size: u64,
        media_type: &str,
        maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>> {
        let maximum_bytes = u64::try_from(maximum_bytes).map_err(|_| HostAccessError::Rejected)?;
        if size == 0 || size > maximum_bytes {
            return Err(HostAccessError::Rejected);
        }
        let digest = AssetDigest::parse(digest).map_err(|_| HostAccessError::InvalidDigest)?;
        let reference =
            AssetRef::new(digest, size, media_type).map_err(|_| HostAccessError::InvalidAsset)?;
        let file = self
            .home()?
            .asset_store()
            .open_asset(&reference)
            .map_err(map_asset_store_read_error)?;
        let capacity = usize::try_from(size).map_err(|_| HostAccessError::Rejected)?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take(maximum_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| HostAccessError::Unavailable)?;
        if bytes.len() != capacity {
            return Err(HostAccessError::Unavailable);
        }
        Ok(bytes)
    }
}

impl UserResourceAccess for LocalHostAccess {
    fn import_resource(
        &self,
        bytes: &[u8],
        media_type: &str,
        name: Option<&str>,
    ) -> HostAccessResult<UserResourceRef> {
        self.user_resources.import(bytes, media_type, name)
    }

    fn read_resource(
        &self,
        reference: &UserResourceRef,
        maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>> {
        self.user_resources.read(reference, maximum_bytes)
    }

    fn release_resource(&self, reference: &UserResourceRef) -> HostAccessResult<()> {
        self.user_resources.release(reference)
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

    fn set_metadata(
        &self,
        world_id: &str,
        title: &str,
        description: Option<&str>,
        cover: Option<WorldSessionAssetRef>,
    ) -> HostAccessResult<WorldSessionSummary> {
        let world_id = world_id
            .parse::<WorldId>()
            .map_err(|_| HostAccessError::InvalidWorldId)?;
        let cover = cover
            .map(|reference| {
                Ok(AssetRef {
                    digest: AssetDigest::parse(reference.digest)
                        .map_err(|_| HostAccessError::InvalidAsset)?,
                    size: reference.size,
                    media_type: AssetMediaType::parse(reference.media_type)
                        .map_err(|_| HostAccessError::InvalidAsset)?,
                })
            })
            .transpose()?;
        let world = self
            .home()?
            .update_world_metadata(world_id, title, description.map(str::to_string), cover)
            .map_err(map_host_access_error)?;
        self.world_sessions.summary(world)
    }

    fn delete_world(&self, world_id: &str) -> HostAccessResult<()> {
        let world_id = world_id
            .parse::<WorldId>()
            .map_err(|_| HostAccessError::InvalidWorldId)?;
        let home = self.home()?;
        home.load_world_state(world_id)
            .map_err(map_host_access_error)?;
        self.world_sessions
            .delete_inactive(world_id, || home.delete_world(world_id))
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
    fn list_http_origins(&self, scope_id: &str) -> HostAccessResult<Vec<RuntimeHttpOriginGrant>> {
        self.home()?
            .list_http_loopback_origins(&RuntimeScopeId::new(scope_id))
            .map(|origins| {
                origins
                    .into_iter()
                    .map(|grant| RuntimeHttpOriginGrant {
                        scope_id: grant.scope_id.to_string(),
                        instance_id: grant.instance_id.to_string(),
                        component_id: grant.component_id.to_string(),
                        origin: grant.origin,
                    })
                    .collect()
            })
            .map_err(map_host_access_error)
    }

    fn grant_http_origin(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        origin: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .grant_http_loopback_origin(
                RuntimeScopeId::new(scope_id),
                ComponentRef::new(instance_id, component_id),
                origin,
            )
            .map_err(map_host_access_error)
    }

    fn revoke_http_origin(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        origin: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .revoke_http_loopback_origin(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                origin,
            )
            .map_err(map_host_access_error)
    }

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

    fn set_preferred_provider_in_scope(
        &self,
        scope_id: &str,
        contract_id: &str,
        contract_version: u32,
        provider_instance_id: &str,
        provider_component_id: &str,
    ) -> HostAccessResult<()> {
        if scope_id.is_empty()
            || contract_id.is_empty()
            || contract_version == 0
            || provider_instance_id.is_empty()
            || provider_component_id.is_empty()
        {
            return Err(HostAccessError::Rejected);
        }
        self.home()?
            .set_preferred_provider(
                RuntimeScopeId::new(scope_id),
                ContractKey::new(contract_id, ContractVersion::new(contract_version)),
                ComponentRef::new(provider_instance_id, provider_component_id),
            )
            .map_err(map_host_access_error)
    }

    fn clear_preferred_provider_in_scope(
        &self,
        scope_id: &str,
        contract_id: &str,
        contract_version: u32,
    ) -> HostAccessResult<()> {
        if scope_id.is_empty() || contract_id.is_empty() || contract_version == 0 {
            return Err(HostAccessError::Rejected);
        }
        self.home()?
            .clear_preferred_provider(
                &RuntimeScopeId::new(scope_id),
                &ContractKey::new(contract_id, ContractVersion::new(contract_version)),
            )
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

fn map_asset_store_read_error(error: rintawa_artifacts::AssetError) -> HostAccessError {
    match error {
        rintawa_artifacts::AssetError::InvalidDigest(_) => HostAccessError::InvalidDigest,
        rintawa_artifacts::AssetError::InvalidMediaType { .. } => HostAccessError::InvalidAsset,
        rintawa_artifacts::AssetError::StoredAssetNotFound(_) => HostAccessError::NotFound,
        rintawa_artifacts::AssetError::Io(_)
        | rintawa_artifacts::AssetError::UnsupportedSource(_)
        | rintawa_artifacts::AssetError::AssetTooLarge { .. }
        | rintawa_artifacts::AssetError::StoreCorruption(_)
        | rintawa_artifacts::AssetError::SizeMismatch { .. }
        | rintawa_artifacts::AssetError::InvalidStoreEntry { .. } => HostAccessError::Unavailable,
    }
}

fn map_host_access_error(error: HostError) -> HostAccessError {
    match error {
        HostError::ActivationNotFound(_) | HostError::WorldNotFound(_) => HostAccessError::NotFound,
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

struct ActiveWorldRuntimeParts {
    runtime: WorldRuntime,
    command_owners: BTreeMap<SchemaKey, ComponentRef>,
}

/// Host-bound adapter from an active WASM component to one authoritative command schema.
///
/// The extension cannot choose this schema, owner, World or principal. The runtime
/// validates the command before this adapter runs and validates its transaction
/// again before committing.
struct WasmWorldSystem {
    command_schema: SchemaKey,
    endpoint: WasmWorldSystemEndpoint,
}

impl WorldSystem for WasmWorldSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<rintawa_world::WorldTransaction> {
        self.endpoint.evaluate(snapshot.clone(), command)
    }
}

struct ActiveWorld {
    runtime: WorldRuntime,
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
    host_access: Option<Arc<LocalHostAccess>>,
    world_data: WorldDataRuntimeControl,
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
        let user_resources = UserResourceRuntimeControl::default();
        let world_sessions = WorldSessionRuntimeControl::default();
        let world_data = WorldDataRuntimeControl::default();
        let access = Arc::new(LocalHostAccess::new(
            home.root(),
            user_resources,
            world_sessions,
        ));
        let artifact_store_access: Arc<dyn ArtifactStoreAccess> = access.clone();
        let asset_store_access: Arc<dyn AssetStoreAccess> = access.clone();
        let user_resource_access: Arc<dyn UserResourceAccess> = access.clone();
        let world_session_access: Arc<dyn WorldSessionAccess> = access.clone();
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
        let runtime_context_access: Arc<dyn RuntimeContextAccess> = access.clone();
        engine.attach_runtime_context_access(runtime_context_access);
        engine.attach_user_resource_access(user_resource_access);
        engine.attach_world_data_access(Arc::new(world_data.clone()));
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        let host_shell_contract = host_shell_contract_key();
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            host_shell_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;

        let activated = activate_composition(&mut engine, home, &profile, &host_scope)?;
        let registered = activated.registered_instances;
        let started = activated.started_instances;
        let mut host_shell_provider = None;

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
            host_access: Some(access),
            world_data,
        })
    }

    /// Returns the selected active provider of the platform Host Shell role.
    ///
    /// `None` is a valid headless composition with no eligible Host Shell provider.
    pub fn host_shell_provider(&self) -> Option<&ComponentRef> {
        self.host_shell_provider.as_ref()
    }

    /// Returns whether one authoritative world currently owns a running worker.
    pub fn is_world_active(&self, world_id: WorldId) -> bool {
        self.active_worlds.contains_key(&world_id)
    }

    /// Opens one persistent world and starts its authoritative command worker.
    ///
    /// Property definitions come from the exact artifacts installed in this world's
    /// composition overlay and are persisted in the world vocabulary.
    ///
    /// # Errors
    ///
    /// Returns WorldAlreadyActive for duplicate activation, world/storage errors,
    /// or World Runtime startup errors.
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
        let overlay = match activate_composition(&mut self.engine, home, &profile, &scope_id) {
            Ok(overlay) => overlay,
            Err(error) => {
                clear_world_scope_topology(&mut self.engine, &scope_id);
                return Err(error);
            }
        };
        let prepared = prepare_world_runtime(self, home, world_id, &overlay.registered_instances);
        let runtime_parts = match prepared {
            Ok(parts) => parts,
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

        self.world_data.activate(
            world_id,
            runtime_parts.runtime.client(),
            runtime_parts.command_owners.clone(),
        )?;
        self.active_worlds.insert(
            world_id,
            ActiveWorld {
                runtime: runtime_parts.runtime,
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
        self.world_data.deactivate(world_id);
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

    /// Executes one authoritative command in an active world.
    ///
    /// Returns the single-writer result only after the SQLite transaction commits.
    /// Failures are returned to the caller rather than logged as deferred work.
    ///
    /// # Errors
    ///
    /// Returns WorldNotActive or a world runtime command/commit error.
    pub fn submit_world_command(
        &mut self,
        world_id: WorldId,
        command: WorldCommand,
    ) -> HostResult<WorldCommandOutcome> {
        let active = self
            .active_worlds
            .get(&world_id)
            .ok_or(HostError::WorldNotActive(world_id))?;
        Ok(active.runtime.submit(command)?.wait()?)
    }

    /// Submit a command from a native, previously authenticated human session.
    ///
    /// The Principal comes from trusted Host ingress, never an extension payload or
    /// a claimed UI action. Guest submissions cannot call this native method.
    ///
    /// # Errors
    ///
    /// Refuses Principal impersonation and normal World/runtime failures.
    pub fn submit_interactive_world_command(
        &mut self,
        world_id: WorldId,
        authenticated_principal: PrincipalId,
        command: WorldCommand,
    ) -> HostResult<WorldCommandOutcome> {
        if command.interactive_principal() != Some(authenticated_principal) {
            return Err(HostError::InteractiveWorldCommandPrincipalMismatch);
        }
        self.submit_world_command(world_id, command)
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

    /// Executes one cooperative runtime pump for active baseline and world components.
    ///
    /// The returned duration is the earliest requested next wake-up. `None` means
    /// no active component currently owns scheduled cooperative work.
    pub fn poll_runtime(&mut self) -> HostResult<Option<Duration>> {
        self.pump_world_session_requests()?;
        let engine_delay = self.engine.poll_runtime()?;
        self.pump_world_session_requests()?;
        Ok(engine_delay)
    }

    /// Stops every active world, then unregisters baseline runtime instances.
    pub fn shutdown(mut self) -> HostResult<()> {
        if let Ok(mut state) = self.world_data.state.lock() {
            state.worlds.clear();
        }
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

/// Builds the single-writer command runtime for one world from installed artifacts.
///
/// Property definitions come from the exact installed world artifacts and are
/// persisted in the world vocabulary. Command schemas are transient runtime API
/// descriptions and each is bound to exactly one active WASM System callback.
///
/// # Errors
///
/// Returns world/storage errors or schema definition conflicts.
fn prepare_world_runtime(
    host: &mut HostRuntime,
    home: &HostHome,
    world_id: WorldId,
    instance_ids: &[ExtensionInstanceId],
) -> HostResult<ActiveWorldRuntimeParts> {
    let storage = home.open_world_storage(world_id)?;
    let schemas = installed_world_schemas(&host.engine, instance_ids)?;
    let property_definitions: Vec<PropertyDefinition> = schemas
        .iter()
        .filter(|schema| schema.kind == SchemaKind::Property)
        .map(|schema| {
            PropertyDefinition::new(
                schema.key.clone(),
                schema.owner.clone(),
                schema.definition.clone(),
            )
        })
        .collect();
    let mut builder = WorldRuntimeBuilder::new(storage);
    let command_owners = schemas
        .iter()
        .filter(|schema| schema.kind == SchemaKind::Command)
        .map(|schema| (schema.key.clone(), schema.declarer.clone()))
        .collect();
    for schema in schemas
        .iter()
        .filter(|schema| schema.kind != SchemaKind::Property)
    {
        builder.register_runtime_schema(RuntimeSchemaDefinition::new(
            schema.key.clone(),
            schema.kind,
            schema.owner.clone(),
            schema.definition.clone(),
        ))?;
    }
    for schema in schemas
        .iter()
        .filter(|schema| schema.kind == SchemaKind::Command)
    {
        let endpoint = host.engine.world_system_endpoint(&schema.declarer)?;
        builder.register_system(WasmWorldSystem {
            command_schema: schema.key.clone(),
            endpoint,
        })?;
    }
    let runtime = builder.start()?;
    runtime.register_property_definitions(&property_definitions)?;

    Ok(ActiveWorldRuntimeParts {
        runtime,
        command_owners,
    })
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
    engine.clear_authoritative_transport_providers_in_scope(scope_id);
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

        for selection in &profile.authoritative_providers {
            engine.set_authoritative_transport_provider_in_scope(
                &selection.scope_id,
                &selection.point,
                &selection.provider(),
            )?;
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
                        for grant in profile.http_loopback_origins.iter().filter(|grant| {
                            grant.scope_id == activation.scope_id
                                && grant.instance_id == activation.instance_id
                        }) {
                            engine.approve_requested_http_loopback_origin_for_instance(
                                &activation.instance_id,
                                &grant.component_id,
                                &grant.origin,
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
        engine.clear_authoritative_transport_providers_in_scope(scope_id);
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

/// One immutable world schema declared by an exact installed extension artifact.
struct InstalledWorldSchema {
    key: SchemaKey,
    kind: SchemaKind,
    owner: ExtensionId,
    declarer: ComponentRef,
    definition: serde_json::Value,
}

/// Collects the world schemas declared by the exact installed world artifacts.
///
/// Ownership comes from the installed artifact manifest and its exact registered
/// component, never from a guest-supplied identity. Until property owner
/// bindings include concrete instances, one logical extension may register
/// World schemas from only one instance per World. Otherwise a same-ID instance
/// could read/write its sibling's properties. Duplicate keys declared by
/// different principals are also rejected before durable publication.
///
/// # Errors
///
/// Returns an Engine error for an unknown runtime instance, or
/// [`HostError::InvalidWorldSchemaJson`] when a declaration is not valid JSON.
fn installed_world_schemas(
    engine: &ExtensionEngine,
    instance_ids: &[ExtensionInstanceId],
) -> HostResult<Vec<InstalledWorldSchema>> {
    let mut schemas = Vec::new();
    let mut extension_instances = BTreeMap::<String, ExtensionInstanceId>::new();
    for instance_id in instance_ids {
        for registered in engine.registered_world_schemas(instance_id)? {
            let extension_id = registered.extension_id();
            if let Some(previous) =
                extension_instances.insert(extension_id.to_string(), instance_id.clone())
                && previous != *instance_id
            {
                return Err(HostError::AmbiguousWorldExtensionInstance {
                    extension_id: extension_id.to_string(),
                    first: previous.to_string(),
                    second: instance_id.to_string(),
                });
            }
            let contribution = registered.contribution();
            let definition =
                serde_json::from_str(contribution.definition_json()).map_err(|source| {
                    HostError::InvalidWorldSchemaJson {
                        schema: contribution.key().clone(),
                        source,
                    }
                })?;
            schemas.push(InstalledWorldSchema {
                key: contribution.key().clone(),
                kind: contribution.kind(),
                owner: registered.extension_id().clone(),
                declarer: ComponentRef::new(instance_id.clone(), registered.component_id().clone()),
                definition,
            });
        }
    }

    schemas.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| left.owner.as_str().cmp(right.owner.as_str()))
            .then_with(|| {
                left.declarer
                    .instance_id
                    .as_str()
                    .cmp(right.declarer.instance_id.as_str())
            })
            .then_with(|| {
                left.declarer
                    .component_id
                    .as_str()
                    .cmp(right.declarer.component_id.as_str())
            })
    });
    for adjacent in schemas.windows(2) {
        if adjacent[0].key == adjacent[1].key && adjacent[0].declarer != adjacent[1].declarer {
            let principal =
                |owner: &ComponentRef| format!("{}/{}", owner.instance_id, owner.component_id);
            return Err(HostError::AmbiguousWorldSchemaDeclaration {
                schema: adjacent[0].key.clone(),
                first: principal(&adjacent[0].declarer),
                second: principal(&adjacent[1].declarer),
            });
        }
    }
    schemas.dedup_by(|left, right| {
        left.key == right.key
            && left.kind == right.kind
            && left.declarer == right.declarer
            && left.definition == right.definition
    });
    Ok(schemas)
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
    engine.clear_authoritative_transport_providers_in_scope(scope_id);
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
        context::RegistrationContext,
        contracts::{
            ContractConsumer, ContractDefinition, ContractKey, ContractProvider, ContractVersion,
        },
        contributions::WorldSchemaContribution,
        errors::{ExtensionError, ExtensionResult},
        manifest::ExtensionManifest,
        prelude::{Component, ComponentId, ContractResolutionPolicy, ExtensionId},
        world::{ItemId, PrincipalId, SchemaKey, WorldId},
    };
    use rintawa_world::{
        PropertyDefinition, RuntimeSchemaDefinition, SchemaKind, StateChange, WorldCommand,
        WorldError, WorldTransaction,
    };
    use rintawa_world_runtime::{WorldRuntimeBuilder, WorldRuntimeError, WorldSystem};
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_should_resolve_world_identity_only_for_canonical_world_runtime_scope()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let access = LocalHostAccess::new(
            root.path(),
            UserResourceRuntimeControl::default(),
            WorldSessionRuntimeControl::default(),
        );
        let world_id = WorldId::new();
        assert_eq!(
            RuntimeContextAccess::world_id_for_scope(&access, &RuntimeScopeId::new(HOST_SCOPE)),
            Ok(None)
        );
        assert_eq!(
            RuntimeContextAccess::world_id_for_scope(&access, &world_runtime_scope_id(world_id)),
            Ok(Some(world_id))
        );
        assert_eq!(
            RuntimeContextAccess::world_id_for_scope(
                &access,
                &RuntimeScopeId::new("world:not-a-world-id"),
            ),
            Ok(None)
        );
        Ok(())
    }

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
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

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

    /// Records every evaluated command and writes one item plus one property.
    struct RecordingWorldSystem {
        command_schema: SchemaKey,
        property_schema: SchemaKey,
        observed: Arc<Mutex<Vec<WorldCommand>>>,
        created: Arc<Mutex<Vec<ItemId>>>,
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
            let item = ItemId::new();
            self.created
                .lock()
                .map_err(|_| {
                    rintawa_world_runtime::SystemError::Failed(String::from(
                        "test item observer lock poisoned",
                    ))
                })?
                .push(item);
            let mut transaction = WorldTransaction::new();
            transaction.push_change(StateChange::CreateObject { id: item });
            transaction.push_change(StateChange::SetProperty {
                item,
                key: self.property_schema.clone(),
                data: command.payload().clone(),
            });
            Ok(transaction)
        }
    }

    struct WorldSchemaComponent {
        id: ComponentId,
        schemas: Vec<WorldSchemaContribution>,
    }

    impl Component for WorldSchemaComponent {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

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

    #[test]
    fn test_should_register_installed_world_property_and_runtime_schemas() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let scope = world_runtime_scope_id(world_id);
        let owner = ExtensionId::new("rintawa.schema-owner");
        let instance = ExtensionInstanceId::new("schema-owner-instance");
        let property: SchemaKey = "rintawa.test.mood@1".parse()?;
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            instance.clone(),
            scope,
            test_manifest(owner.as_str()),
            vec![Box::new(WorldSchemaComponent {
                id: ComponentId::new("schemas"),
                schemas: vec![WorldSchemaContribution::new(
                    property.clone(),
                    SchemaKind::Property,
                    r#"{"type":"object"}"#,
                )],
            })],
        )?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            host_access: None,
            world_data: WorldDataRuntimeControl::default(),
        };
        let parts = prepare_world_runtime(&mut host, &home, world_id, &[instance])?;

        let inspection = home.open_world_storage(world_id)?;
        let session = inspection.load_session()?;
        let registered = session
            .properties()
            .get(&property)
            .expect("property definition must persist in the world vocabulary");
        // The owner is Host composition metadata and is not persisted in the
        // world vocabulary; only the exact name, version and schema document are.
        assert_eq!(
            registered.schema(),
            &serde_json::json!({ "type": "object" })
        );
        assert_eq!(session.properties().len(), 1);

        // Exact re-registration is idempotent across world restarts.
        let repeated = PropertyDefinition::new(
            property.clone(),
            owner.clone(),
            serde_json::json!({ "type": "object" }),
        );
        parts.runtime.register_property_definitions(&[repeated])?;
        assert_eq!(
            home.open_world_storage(world_id)?
                .load_session()?
                .properties()
                .len(),
            1
        );

        parts.runtime.shutdown()?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_execute_wasm_world_system_commit_rollback_and_owner_denial() -> anyhow::Result<()>
    {
        const WORLD_SYSTEM_FIXTURE: &[u8] =
            include_bytes!("../../extension-engine/tests/fixtures/world-system/component.wasm");

        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let scope = world_runtime_scope_id(world_id);
        let item = ItemId::new();
        let mut seed = WorldTransaction::new();
        seed.push_change(StateChange::CreateObject { id: item });
        home.open_world_storage(world_id)?.apply(&seed)?;

        let owner_instance = ExtensionInstanceId::new("wasm-world-system");
        let foreign_instance = ExtensionInstanceId::new("foreign-property-owner");
        let world_data = WorldDataRuntimeControl::default();
        let mut engine = ExtensionEngine::new();
        engine.attach_world_data_access(Arc::new(world_data.clone()));
        let wasm_component = engine
            .wasm_runtime_engine()?
            .load_component_from_bytes(ComponentId::new("wasm-system"), WORLD_SYSTEM_FIXTURE)?;
        engine.register_extension_instance(
            owner_instance.clone(),
            scope.clone(),
            test_manifest("fixture.world-system"),
            vec![Box::new(wasm_component)],
        )?;
        engine.register_extension_instance(
            foreign_instance.clone(),
            scope,
            test_manifest("fixture.foreign-owner"),
            vec![Box::new(WorldSchemaComponent {
                id: ComponentId::new("foreign-schemas"),
                schemas: vec![WorldSchemaContribution::new(
                    "fixture.foreign-property@1".parse()?,
                    SchemaKind::Property,
                    r#"{"type":"integer"}"#,
                )],
            })],
        )?;
        engine.start_extension_instance(&owner_instance)?;
        engine.start_extension_instance(&foreign_instance)?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            host_access: None,
            world_data,
        };
        let parts = prepare_world_runtime(
            &mut host,
            &home,
            world_id,
            &[owner_instance.clone(), foreign_instance],
        )?;
        assert!(matches!(
            host.world_data.load_item(
                &RuntimeScopeId::new(HOST_SCOPE),
                &ExtensionId::new("fixture.world-system"),
                item,
            ),
            Err(HostAccessError::Unavailable)
        ));
        host.world_data.activate(
            world_id,
            parts.runtime.client(),
            parts.command_owners.clone(),
        )?;
        let commit: SchemaKey = "fixture.commit@1".parse()?;
        let reject: SchemaKey = "fixture.reject@1".parse()?;
        let foreign: SchemaKey = "fixture.foreign@1".parse()?;
        let payload = serde_json::json!({ "item": item.to_string() });
        assert!(matches!(
            host.world_data.submit(
                &world_runtime_scope_id(world_id),
                &ComponentRef::new("foreign-property-owner", "foreign-schemas"),
                commit.clone(),
                payload.clone(),
            ),
            Err(HostAccessError::Rejected)
        ));
        parts
            .runtime
            .submit(WorldCommand::system(commit, payload.clone()))?
            .wait()?;
        assert_eq!(
            parts
                .runtime
                .snapshot()?
                .load_property(item, &"fixture.value@1".parse()?)?
                .expect("WASM transaction must commit")
                .data(),
            &serde_json::json!(7)
        );
        assert!(
            parts
                .runtime
                .submit(WorldCommand::system(reject, payload.clone()))?
                .wait()
                .is_err()
        );
        let foreign_error = parts
            .runtime
            .submit(WorldCommand::system(foreign, payload))?
            .wait()
            .expect_err("foreign property write must be denied");
        assert!(matches!(
            foreign_error,
            WorldRuntimeError::InvalidTransaction {
                source: WorldError::PropertyAccessDenied { .. },
                ..
            }
        ));
        assert!(
            parts
                .runtime
                .snapshot()?
                .load_property(item, &"fixture.foreign-property@1".parse()?)?
                .is_none()
        );

        // This is the ordinary guest path: the active component submits its
        // own authoritative command from an event callback and only later
        // polls the nonblocking ticket before reading its committed property.
        host.engine.dispatch_runtime_event_in_scope(
            &world_runtime_scope_id(world_id),
            "fixture.world-data",
            b"invalid",
        )?;
        host.engine.dispatch_runtime_event_in_scope(
            &world_runtime_scope_id(world_id),
            "fixture.world-data",
            format!("foreign:{item}").as_bytes(),
        )?;
        host.engine.dispatch_runtime_event_in_scope(
            &world_runtime_scope_id(world_id),
            "fixture.world-data",
            format!("submit:{item}").as_bytes(),
        )?;
        for _ in 0..100 {
            if parts
                .runtime
                .snapshot()?
                .load_property(item, &"fixture.value@1".parse()?)?
                .is_some()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        host.engine.dispatch_runtime_event_in_scope(
            &world_runtime_scope_id(world_id),
            "fixture.world-data",
            format!("verify:{item}").as_bytes(),
        )?;
        host.engine.stop_extension_instance(&owner_instance)?;
        assert!(
            parts
                .runtime
                .submit(WorldCommand::system(
                    "fixture.commit@1".parse()?,
                    serde_json::json!({ "item": item.to_string() }),
                ))?
                .wait()
                .is_err()
        );
        parts.runtime.shutdown()?;
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_reject_same_extension_id_with_disjoint_world_schema_keys() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let scope = world_runtime_scope_id(world_id);
        let mut engine = ExtensionEngine::new();
        let mut instances = Vec::new();

        for (instance_id, schema_name) in [
            ("first-version", "rintawa.test.first-property@1"),
            ("second-version", "rintawa.test.second-property@1"),
        ] {
            let instance = ExtensionInstanceId::new(instance_id);
            let schema: SchemaKey = schema_name.parse()?;
            engine.register_extension_instance(
                instance.clone(),
                scope.clone(),
                test_manifest("rintawa.same-logical-extension"),
                vec![Box::new(WorldSchemaComponent {
                    id: ComponentId::new("schemas"),
                    schemas: vec![WorldSchemaContribution::new(
                        schema,
                        SchemaKind::Property,
                        r#"{"type":"object"}"#,
                    )],
                })],
            )?;
            instances.push(instance);
        }

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            host_access: None,
            world_data: WorldDataRuntimeControl::default(),
        };
        assert!(matches!(
            prepare_world_runtime(&mut host, &home, world_id, &instances),
            Err(HostError::AmbiguousWorldExtensionInstance {
                extension_id,
                first,
                second,
            }) if extension_id == "rintawa.same-logical-extension"
                && first == "first-version" && second == "second-version"
        ));
        assert!(
            home.open_world_storage(world_id)?
                .load_session()?
                .properties()
                .is_empty()
        );
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_reject_duplicate_world_schema_from_distinct_component_principals()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
        let scope = world_runtime_scope_id(world_id);
        let property: SchemaKey = "rintawa.test.duplicate-property@1".parse()?;
        let mut engine = ExtensionEngine::new();
        let mut instances = Vec::new();

        for (instance_id, extension_id) in [
            ("different-owner-first", "rintawa.first-extension"),
            ("different-owner-second", "rintawa.second-extension"),
        ] {
            let instance = ExtensionInstanceId::new(instance_id);
            engine.register_extension_instance(
                instance.clone(),
                scope.clone(),
                test_manifest(extension_id),
                vec![Box::new(WorldSchemaComponent {
                    id: ComponentId::new("schemas"),
                    schemas: vec![WorldSchemaContribution::new(
                        property.clone(),
                        SchemaKind::Property,
                        r#"{"type":"object"}"#,
                    )],
                })],
            )?;
            instances.push(instance);
        }

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            host_access: None,
            world_data: WorldDataRuntimeControl::default(),
        };
        assert!(matches!(
            prepare_world_runtime(&mut host, &home, world_id, &instances),
            Err(HostError::AmbiguousWorldSchemaDeclaration { schema, .. })
                if schema == property
        ));
        assert!(
            home.open_world_storage(world_id)?
                .load_session()?
                .properties()
                .is_empty()
        );
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_reject_malformed_extension_schema_without_partial_publication()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_id = home.create_world()?.id;
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
                        SchemaKind::Property,
                        r#"{"type":"object"}"#,
                    ),
                    WorldSchemaContribution::new(invalid.clone(), SchemaKind::Command, "{"),
                ],
            })],
        )?;

        let mut host = HostRuntime {
            active_worlds: BTreeMap::new(),
            engine,
            started_instances: Vec::new(),
            host_shell_provider: None,
            host_access: None,
            world_data: WorldDataRuntimeControl::default(),
        };
        assert!(matches!(
            prepare_world_runtime(&mut host, &home, world_id, &[instance]),
            Err(HostError::InvalidWorldSchemaJson { schema, .. }) if schema == invalid
        ));
        assert!(
            home.open_world_storage(world_id)?
                .load_session()?
                .properties()
                .is_empty()
        );
        assert!(host.active_worlds.is_empty());
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_bound_verify_and_release_ephemeral_user_resources() -> anyhow::Result<()> {
        let control = UserResourceRuntimeControl::default();
        let reference = control
            .import(
                br#"{"spec":"chara_card_v3"}"#,
                "application/json",
                Some("character.json"),
            )
            .map_err(|error| anyhow::anyhow!("resource import failed: {error:?}"))?;
        assert_eq!(reference.size, 24);
        assert_eq!(reference.media_type, "application/json");
        assert_eq!(reference.name.as_deref(), Some("character.json"));
        assert_eq!(
            control
                .read(&reference, 1024)
                .map_err(|error| anyhow::anyhow!("resource read failed: {error:?}"))?,
            br#"{"spec":"chara_card_v3"}"#
        );

        let mut spoofed = reference.clone();
        spoofed.size = spoofed.size.saturating_add(1);
        assert_eq!(control.read(&spoofed, 1024), Err(HostAccessError::Rejected));

        let mut spoofed_media_type = reference.clone();
        spoofed_media_type.media_type = String::from("text/plain");
        assert_eq!(
            control.read(&spoofed_media_type, 1024),
            Err(HostAccessError::Rejected)
        );

        let mut spoofed_name = reference.clone();
        spoofed_name.name = Some(String::from("other.json"));
        assert_eq!(
            control.read(&spoofed_name, 1024),
            Err(HostAccessError::Rejected)
        );
        assert_eq!(control.read(&reference, 4), Err(HostAccessError::Rejected));

        control
            .release(&reference)
            .map_err(|error| anyhow::anyhow!("resource release failed: {error:?}"))?;
        assert_eq!(
            control.read(&reference, 1024),
            Err(HostAccessError::NotFound)
        );
        Ok(())
    }

    #[test]
    fn test_should_reject_invalid_user_resource_metadata_and_capacity() -> anyhow::Result<()> {
        let control = UserResourceRuntimeControl::default();

        assert_eq!(
            control.import(&[], "application/json", Some("empty.json")),
            Err(HostAccessError::Rejected)
        );
        assert_eq!(
            control.import(b"{}", "Application/JSON", Some("upper.json")),
            Err(HostAccessError::Rejected)
        );
        assert_eq!(
            control.import(b"{}", "application/json", Some("../escape.json")),
            Err(HostAccessError::Rejected)
        );
        assert_eq!(
            control.import(
                &vec![0_u8; MAX_USER_RESOURCE_BYTES.saturating_add(1)],
                "application/octet-stream",
                Some("large.bin"),
            ),
            Err(HostAccessError::Rejected)
        );

        let mut references = Vec::with_capacity(MAX_USER_RESOURCES);
        for index in 0..MAX_USER_RESOURCES {
            references.push(
                control
                    .import(
                        &[u8::try_from(index % 255).unwrap_or_default()],
                        "application/octet-stream",
                        Some("bounded.bin"),
                    )
                    .map_err(|error| anyhow::anyhow!("bounded import failed: {error:?}"))?,
            );
        }
        assert_eq!(
            control.import(b"x", "application/octet-stream", Some("overflow.bin"),),
            Err(HostAccessError::QueueFull)
        );

        control
            .release(&references[0])
            .map_err(|error| anyhow::anyhow!("resource release failed: {error:?}"))?;
        control
            .import(b"x", "application/octet-stream", Some("replacement.bin"))
            .map_err(|error| anyhow::anyhow!("replacement import failed: {error:?}"))?;
        Ok(())
    }

    #[test]
    fn test_should_import_raw_asset_through_generic_host_access() -> anyhow::Result<()> {
        let root = tempfile::TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let access = LocalHostAccess::new(
            home.root(),
            UserResourceRuntimeControl::default(),
            WorldSessionRuntimeControl::default(),
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
            AssetStoreAccess::read_asset(&access, &imported.digest, imported.size, "image/png", 64,),
            Ok(b"portrait".to_vec())
        );
        assert_eq!(
            AssetStoreAccess::read_asset(&access, &imported.digest, imported.size, "image/png", 4,),
            Err(HostAccessError::Rejected)
        );
        assert_eq!(
            AssetStoreAccess::import_asset(&access, b"portrait", "invalid media type"),
            Err(HostAccessError::InvalidAsset)
        );
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
            title: String::from("Test World"),
            description: None,
            cover: None,
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
                title: String::from("Test World"),
                description: None,
                cover: None,
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

        let deletable = WorldSessionAccess::create_world(access.as_ref())
            .expect("world-session access must create an inactive world for deletion");
        WorldSessionAccess::delete_world(access.as_ref(), &deletable.world_id)
            .expect("inactive world must be deletable");
        assert!(
            WorldSessionAccess::list_worlds(access.as_ref())
                .expect("world-session catalog must remain readable after delete")
                .iter()
                .all(|world| world.world_id != deletable.world_id)
        );

        let created = WorldSessionAccess::create_world(access.as_ref())
            .expect("world-session access must create an empty world");
        let world_id: WorldId = created.world_id.parse()?;
        assert!(!created.active);
        assert_eq!(created.pending_active, None);
        assert!(!host.is_world_active(world_id));

        WorldSessionAccess::set_active(access.as_ref(), &created.world_id, true)
            .expect("world-session access must queue activation");
        assert_eq!(
            WorldSessionAccess::delete_world(access.as_ref(), &created.world_id),
            Err(HostAccessError::Rejected)
        );
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
        assert_eq!(
            WorldSessionAccess::delete_world(access.as_ref(), &created.world_id),
            Err(HostAccessError::Rejected)
        );

        WorldSessionAccess::set_active(access.as_ref(), &created.world_id, false)
            .expect("world-session access must queue deactivation");
        let pending = WorldSessionAccess::list_worlds(access.as_ref())
            .expect("world-session access must list pending deactivation")
            .into_iter()
            .find(|world| world.world_id == created.world_id)
            .expect("pending world must remain listed");
        assert!(pending.active);
        assert_eq!(pending.pending_active, Some(false));
        assert_eq!(
            WorldSessionAccess::delete_world(access.as_ref(), &created.world_id),
            Err(HostAccessError::Rejected)
        );

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
        WorldSessionAccess::delete_world(access.as_ref(), &created.world_id)
            .expect("stopped world must be deletable");
        assert!(
            WorldSessionAccess::list_worlds(access.as_ref())
                .expect("catalog must remain readable after deleting stopped world")
                .iter()
                .all(|world| world.world_id != created.world_id)
        );
        Ok(())
    }

    #[test]
    fn test_should_return_native_world_command_result_only_after_commit() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world = home.create_world()?;
        let command_schema: SchemaKey = "test.native-command@1".parse()?;
        let property_schema: SchemaKey = "test.native-property@1".parse()?;
        let owner = ExtensionId::new("test.native-system");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let created = Arc::new(Mutex::new(Vec::new()));
        let mut builder = WorldRuntimeBuilder::new(home.open_world_storage(world.id)?);
        builder.register_runtime_schema(RuntimeSchemaDefinition::new(
            command_schema.clone(),
            SchemaKind::Command,
            owner.clone(),
            serde_json::json!({
                "type": "object",
                "properties": {"kind": {"type": "string"}},
                "required": ["kind"]
            }),
        ))?;
        builder.register_system(RecordingWorldSystem {
            command_schema: command_schema.clone(),
            property_schema: property_schema.clone(),
            observed: Arc::clone(&observed),
            created: Arc::clone(&created),
        })?;
        let runtime = builder.start()?;
        runtime.register_property_definitions(&[PropertyDefinition::new(
            property_schema.clone(),
            owner,
            serde_json::json!({"type": "object"}),
        )])?;

        let mut host = HostRuntime::start(&home)?;
        host.active_worlds.insert(
            world.id,
            ActiveWorld {
                runtime,
                registered_instances: Vec::new(),
                started_instances: Vec::new(),
            },
        );
        let principal = PrincipalId::new();
        assert!(matches!(
            host.submit_interactive_world_command(
                world.id,
                principal,
                WorldCommand::system(
                    command_schema.clone(),
                    serde_json::json!({"kind": "committed"}),
                ),
            ),
            Err(HostError::InteractiveWorldCommandPrincipalMismatch)
        ));
        let good = WorldCommand::interactive(
            command_schema.clone(),
            principal,
            serde_json::json!({"kind": "committed"}),
        );
        assert!(matches!(
            host.submit_interactive_world_command(world.id, PrincipalId::new(), good.clone()),
            Err(HostError::InteractiveWorldCommandPrincipalMismatch)
        ));
        assert!(
            created
                .lock()
                .map_err(|_| anyhow::anyhow!("creation lock poisoned"))?
                .is_empty()
        );
        let expected_command_id = good.id();
        let committed = host.submit_interactive_world_command(world.id, principal, good)?;
        assert_eq!(committed.command_id(), expected_command_id);
        let item = {
            let items = created
                .lock()
                .map_err(|_| anyhow::anyhow!("creation lock poisoned"))?;
            assert_eq!(items.len(), 1);
            items[0]
        };
        assert!(
            home.open_world_storage(world.id)?
                .load_item(item)?
                .is_some()
        );
        let persisted = home
            .open_world_storage(world.id)?
            .load_property(item, &property_schema)?;
        assert_eq!(
            persisted
                .ok_or_else(|| anyhow::anyhow!("committed property is missing"))?
                .data(),
            &serde_json::json!({"kind": "committed"})
        );

        let malformed =
            WorldCommand::interactive(command_schema, principal, serde_json::json!({"kind": 99}));
        assert!(host.submit_world_command(world.id, malformed).is_err());
        assert_eq!(
            created
                .lock()
                .map_err(|_| anyhow::anyhow!("creation lock poisoned"))?
                .len(),
            1
        );
        assert_eq!(
            observed
                .lock()
                .map_err(|_| anyhow::anyhow!("observer lock poisoned"))?
                .len(),
            1
        );
        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_keep_multiple_worlds_active_independently() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world_a = home.create_world()?.id;
        let world_b = home.create_world()?.id;
        let mut host = HostRuntime::start(&home)?;

        host.activate_world(&home, world_a)?;
        host.activate_world(&home, world_b)?;

        assert!(host.is_world_active(world_a));
        assert!(host.is_world_active(world_b));
        assert_eq!(host.active_worlds.len(), 2);

        host.poll_runtime()?;
        assert!(host.is_world_active(world_a));
        assert!(host.is_world_active(world_b));

        host.deactivate_world(world_a)?;
        assert!(!host.is_world_active(world_a));
        assert!(host.is_world_active(world_b));
        assert_eq!(host.active_worlds.len(), 1);

        host.shutdown()?;
        Ok(())
    }

    #[test]
    fn test_should_supervise_world_activation_and_deactivation() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let created = home.create_world()?;
        let world_id = created.id;
        let command_schema: SchemaKey = "rintawa.test.supervised-command@1".parse()?;

        let mut host = HostRuntime::start(&home)?;
        host.activate_world(&home, world_id)?;
        assert!(host.is_world_active(world_id));
        assert!(matches!(
            host.activate_world(&home, world_id),
            Err(HostError::WorldAlreadyActive(id)) if id == world_id
        ));

        host.poll_runtime()?;
        assert!(host.is_world_active(world_id));

        host.deactivate_world(world_id)?;
        assert!(!host.is_world_active(world_id));
        let command =
            WorldCommand::system(command_schema, serde_json::json!({ "kind": "after-stop" }));
        assert!(matches!(
            host.submit_world_command(world_id, command),
            Err(HostError::WorldNotActive(id)) if id == world_id
        ));
        assert!(matches!(
            host.deactivate_world(world_id),
            Err(HostError::WorldNotActive(id)) if id == world_id
        ));

        // A stopped world reopens from its durable four-table state.
        host.activate_world(&home, world_id)?;
        assert!(host.is_world_active(world_id));
        assert_eq!(host.active_worlds.len(), 1);

        host.shutdown()?;
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
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

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
