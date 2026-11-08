//! Generic host-owned local artifact, asset, and composition access.
//!
//! These traits deliberately contain no repository, update, marketplace, package-manager,
//! or network-source semantics. The production host can expose them to any explicitly
//! authorized component principal.

use std::sync::Arc;

/// One immutable raw asset imported into the local asset store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAsset {
    /// Exact content-addressed SHA-256 digest.
    pub digest: String,
    /// Exact byte length of the stored asset.
    pub size: u64,
    /// Canonical media type attached to the immutable reference.
    pub media_type: String,
}

/// Immutable asset reference attached to host-owned World catalog metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSessionAssetRef {
    /// Canonical SHA-256 asset digest.
    pub digest: String,
    /// Exact byte length.
    pub size: u64,
    /// Canonical media type.
    pub media_type: String,
}

/// One persistent world plus its host runtime lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSessionSummary {
    /// Stable persistent world identity in canonical textual form.
    pub world_id: String,
    /// Human-facing host-owned catalog title.
    pub title: String,
    /// Optional human-facing host-owned description.
    pub description: Option<String>,
    /// Optional immutable cover asset reference.
    pub cover: Option<WorldSessionAssetRef>,
    /// Whether the world currently owns a running authoritative runtime.
    pub active: bool,
    /// Requested active state awaiting the next host runtime pump, when any.
    pub pending_active: Option<bool>,
    /// Bounded diagnostic from the last failed lifecycle transition, when any.
    pub last_error: Option<String>,
}

/// One validated RTW object imported into the immutable local artifact store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedArtifact {
    /// Exact content-addressed digest.
    pub digest: String,
    /// Versioned RTW content type.
    pub content: String,
}

/// One exact activation selected by the host composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionActivation {
    /// Handler-defined stable activation subject.
    pub subject: String,
    /// Versioned RTW content type.
    pub content: String,
    /// Human-readable artifact name when exposed by its content handler.
    pub name: String,
    /// Content-specific version when available.
    pub version: Option<String>,
    /// Exact immutable artifact digest.
    pub digest: String,
    /// Concrete runtime instance identity.
    pub instance_id: String,
    /// Runtime composition scope.
    pub scope_id: String,
    /// Whether the activation is selected to start on next bootstrap.
    pub enabled: bool,
    /// Whether a baseline activation is materialized into newly created worlds.
    pub world_default: bool,
}

/// Generic failure returned by host artifact/asset/composition/preference access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAccessError {
    /// Input bytes are not a valid RTW artifact.
    InvalidArtifact,
    /// Raw asset bytes or media metadata violate asset-store policy.
    InvalidAsset,
    /// A digest string is malformed or does not identify a stored artifact.
    InvalidDigest,
    /// A world identifier is malformed.
    InvalidWorldId,
    /// A bounded host-owned request queue has no remaining capacity.
    QueueFull,
    /// The requested selection does not exist.
    NotFound,
    /// The host has no content handler for the selected RTW content type.
    UnsupportedContent,
    /// A textual runtime permission identity is not recognized.
    InvalidPermission,
    /// An owner-scoped preference key/value violates host bounds.
    InvalidPreference,
    /// The owner exceeded its bounded preference quota.
    PreferenceQuotaExceeded,
    /// Host policy or persistent state rejected the operation.
    Rejected,
    /// The capability is not attached to this host runtime.
    Unavailable,
}

/// Result used by generic host-access capabilities.
pub type HostAccessResult<T> = Result<T, HostAccessError>;

/// Host-owned mapping from opaque runtime scopes to authoritative World identity.
pub trait RuntimeContextAccess: Send + Sync {
    /// Returns the World owning `scope_id`, or `None` for a non-World runtime scope.
    fn world_id_for_scope(
        &self,
        scope_id: &rintawa_sdk::types::RuntimeScopeId,
    ) -> HostAccessResult<Option<rintawa_sdk::world::WorldId>>;
}

/// Generic immutable RTW artifact-store operations.
pub trait ArtifactStoreAccess: Send + Sync {
    /// Validates and imports exact RTW bytes without selecting them for activation.
    fn import_rtw(&self, bytes: &[u8]) -> HostAccessResult<ImportedArtifact>;
}

/// Generic immutable raw-asset store operations.
pub trait AssetStoreAccess: Send + Sync {
    /// Imports exact bytes and returns their immutable digest/size/media reference.
    fn import_asset(&self, bytes: &[u8], media_type: &str) -> HostAccessResult<ImportedAsset>;

    /// Reads one exact verified immutable asset under an explicit caller-owned byte bound.
    fn read_asset(
        &self,
        digest: &str,
        size: u64,
        media_type: &str,
        maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>>;
}

/// Opaque metadata for one bounded ephemeral user-selected resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserResourceRef {
    /// Unguessable Host-issued bearer identity valid only for the current HostRuntime.
    pub id: String,
    /// Exact selected byte length.
    pub size: u64,
    /// Canonical media type supplied by the trusted UI layer.
    pub media_type: String,
    /// Original user-facing file name when available.
    pub name: Option<String>,
}

/// Generic ephemeral user-resource access.
///
/// Resources are intentionally non-enumerable and non-persistent. Possession of an exact
/// [`UserResourceRef`] plus the corresponding runtime permission is required to read bytes.
pub trait UserResourceAccess: Send + Sync {
    /// Creates one bounded HostRuntime-local resource from explicit user-selected bytes.
    fn import_resource(
        &self,
        bytes: &[u8],
        media_type: &str,
        name: Option<&str>,
    ) -> HostAccessResult<UserResourceRef>;

    /// Reads one exact resource after verifying all reference metadata and the caller bound.
    fn read_resource(
        &self,
        reference: &UserResourceRef,
        maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>>;

    /// Explicitly releases one exact ephemeral resource. Missing/mismatched refs fail closed.
    fn release_resource(&self, reference: &UserResourceRef) -> HostAccessResult<()>;
}

/// Generic persistent-world catalog and lifecycle request operations.
pub trait WorldSessionAccess: Send + Sync {
    /// Lists persistent worlds together with current/pending runtime lifecycle state.
    fn list_worlds(&self) -> HostAccessResult<Vec<WorldSessionSummary>>;

    /// Creates one empty persistent authoritative world.
    fn create_world(&self) -> HostAccessResult<WorldSessionSummary>;

    /// Replaces host-owned human-facing metadata for one persistent World.
    fn set_metadata(
        &self,
        world_id: &str,
        title: &str,
        description: Option<&str>,
        cover: Option<WorldSessionAssetRef>,
    ) -> HostAccessResult<WorldSessionSummary>;

    /// Deletes one persistent World when it has no active or pending lifecycle state.
    fn delete_world(&self, world_id: &str) -> HostAccessResult<()>;

    /// Requests the desired active state to be applied after guest execution unwinds.
    fn set_active(&self, world_id: &str, active: bool) -> HostAccessResult<()>;
}

/// Owner-scoped non-authoritative preference storage for ordinary extensions.
///
/// Values are isolated by runtime scope + exact component principal. This is not world
/// state, secret storage, or a composition control channel.
pub trait PreferenceAccess: Send + Sync {
    /// Reads one preference value owned by the exact component principal.
    fn get(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<Option<String>>;

    /// Persists one preference value owned by the exact component principal.
    fn set(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
        value: &str,
    ) -> HostAccessResult<()>;

    /// Deletes one preference owned by the exact component principal.
    fn delete(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<()>;
}

/// Requested runtime permissions for one component in an exact artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePolicyRequest {
    /// Component identifier declared by the artifact manifest.
    pub component_id: String,
    /// Runtime permissions requested by that component.
    pub requested: Vec<String>,
}

/// Read-only runtime policy metadata from one exact imported artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArtifactPolicy {
    /// Logical extension identity declared by the artifact.
    pub subject: String,
    /// Human-readable extension name.
    pub name: String,
    /// Extension version declared by the artifact.
    pub version: String,
    /// Component-scoped runtime permission requests.
    pub components: Vec<RuntimePolicyRequest>,
}

/// Runtime permission policy for one exact baseline component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePolicyComponent {
    /// Runtime scope containing the component principal.
    pub scope_id: String,
    /// Concrete runtime instance identity.
    pub instance_id: String,
    /// Component within the runtime instance.
    pub component_id: String,
    /// Permissions requested by the exact selected artifact manifest.
    pub requested: Vec<String>,
    /// Host-approved permissions currently persisted for this exact principal.
    pub granted: Vec<String>,
}

/// One exact HTTP loopback origin granted to a component in one runtime scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHttpOriginGrant {
    /// Scope of the exact owning runtime instance.
    pub scope_id: String,
    /// Concrete extension instance authorized by the Host.
    pub instance_id: String,
    /// Component within the authorized instance.
    pub component_id: String,
    /// Canonical HTTP origin including its exact port.
    pub origin: String,
}

/// Generic administrative access to runtime permission policy.
pub trait RuntimePolicyAccess: Send + Sync {
    /// Inspects requested runtime permissions in one exact imported artifact.
    fn inspect_artifact(&self, digest: &str) -> HostAccessResult<RuntimeArtifactPolicy>;

    /// Lists requested and granted runtime permissions for baseline components.
    fn list_components(&self) -> HostAccessResult<Vec<RuntimePolicyComponent>>;

    /// Lists requested and granted runtime permissions in one exact composition scope.
    ///
    /// Implementations that do not support scoped compositions fail closed.
    fn list_components_in_scope(
        &self,
        _scope_id: &str,
    ) -> HostAccessResult<Vec<RuntimePolicyComponent>> {
        Err(HostAccessError::Rejected)
    }

    /// Lists approved local HTTP origins in one exact composition scope.
    ///
    /// Host adapters without this capability fail closed.
    fn list_http_origins(&self, _scope_id: &str) -> HostAccessResult<Vec<RuntimeHttpOriginGrant>> {
        Err(HostAccessError::Rejected)
    }

    /// Approves one exact local origin for one manifest-authorized principal.
    fn grant_http_origin(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _origin: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Revokes one exact local origin independently of other granted ports.
    fn revoke_http_origin(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _origin: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Grants one permission to an exact component principal.
    fn grant(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        permission: &str,
    ) -> HostAccessResult<()>;

    /// Revokes one permission from an exact component principal.
    fn revoke(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        permission: &str,
    ) -> HostAccessResult<()>;
}

/// Generic host composition operations over already-local exact artifacts.
pub trait CompositionAccess: Send + Sync {
    /// Lists current baseline selections.
    fn list_activations(&self) -> HostAccessResult<Vec<CompositionActivation>>;

    /// Selects one already-imported exact artifact for the next host bootstrap.
    fn select_artifact(
        &self,
        digest: &str,
        enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation>;

    /// Changes persisted enabled state for an exact selection.
    fn set_enabled(&self, subject: &str, enabled: bool) -> HostAccessResult<()>;

    /// Compatibility helper for selecting one baseline artifact in the independent
    /// default-World recipe used only by future World creation.
    fn set_world_default(&self, _subject: &str, _world_default: bool) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Removes one persisted selection and policy entries that reference it.
    fn remove_activation(&self, subject: &str) -> HostAccessResult<()>;

    /// Lists exact selections in one explicit composition scope.
    ///
    /// Implementations that do not support scoped compositions fail closed.
    fn list_activations_in_scope(
        &self,
        _scope_id: &str,
    ) -> HostAccessResult<Vec<CompositionActivation>> {
        Err(HostAccessError::Rejected)
    }

    /// Selects one exact artifact in one explicit composition scope.
    fn select_artifact_in_scope(
        &self,
        _scope_id: &str,
        _digest: &str,
        _enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation> {
        Err(HostAccessError::Rejected)
    }

    /// Changes persisted enabled state in one explicit composition scope.
    fn set_enabled_in_scope(
        &self,
        _scope_id: &str,
        _subject: &str,
        _enabled: bool,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Selects one exact provider component for a versioned contract in one scope.
    fn set_preferred_provider_in_scope(
        &self,
        _scope_id: &str,
        _contract_id: &str,
        _contract_version: u32,
        _provider_instance_id: &str,
        _provider_component_id: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Clears one explicit provider choice for a versioned contract in one scope.
    fn clear_preferred_provider_in_scope(
        &self,
        _scope_id: &str,
        _contract_id: &str,
        _contract_version: u32,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }

    /// Removes one exact selection from one explicit composition scope.
    fn remove_activation_in_scope(&self, _scope_id: &str, _subject: &str) -> HostAccessResult<()> {
        Err(HostAccessError::Rejected)
    }
}

#[derive(Default)]
struct UnavailableArtifactStoreAccess;

impl ArtifactStoreAccess for UnavailableArtifactStoreAccess {
    fn import_rtw(&self, _bytes: &[u8]) -> HostAccessResult<ImportedArtifact> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableAssetStoreAccess;

impl AssetStoreAccess for UnavailableAssetStoreAccess {
    fn import_asset(&self, _bytes: &[u8], _media_type: &str) -> HostAccessResult<ImportedAsset> {
        Err(HostAccessError::Unavailable)
    }

    fn read_asset(
        &self,
        _digest: &str,
        _size: u64,
        _media_type: &str,
        _maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableUserResourceAccess;

impl UserResourceAccess for UnavailableUserResourceAccess {
    fn import_resource(
        &self,
        _bytes: &[u8],
        _media_type: &str,
        _name: Option<&str>,
    ) -> HostAccessResult<UserResourceRef> {
        Err(HostAccessError::Unavailable)
    }

    fn read_resource(
        &self,
        _reference: &UserResourceRef,
        _maximum_bytes: usize,
    ) -> HostAccessResult<Vec<u8>> {
        Err(HostAccessError::Unavailable)
    }

    fn release_resource(&self, _reference: &UserResourceRef) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableWorldSessionAccess;

impl WorldSessionAccess for UnavailableWorldSessionAccess {
    fn list_worlds(&self) -> HostAccessResult<Vec<WorldSessionSummary>> {
        Err(HostAccessError::Unavailable)
    }

    fn create_world(&self) -> HostAccessResult<WorldSessionSummary> {
        Err(HostAccessError::Unavailable)
    }

    fn set_metadata(
        &self,
        _world_id: &str,
        _title: &str,
        _description: Option<&str>,
        _cover: Option<WorldSessionAssetRef>,
    ) -> HostAccessResult<WorldSessionSummary> {
        Err(HostAccessError::Unavailable)
    }

    fn delete_world(&self, _world_id: &str) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }

    fn set_active(&self, _world_id: &str, _active: bool) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

struct UnavailablePreferenceAccess;

impl PreferenceAccess for UnavailablePreferenceAccess {
    fn get(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _key: &str,
    ) -> HostAccessResult<Option<String>> {
        Err(HostAccessError::Unavailable)
    }

    fn set(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _key: &str,
        _value: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }

    fn delete(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _key: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableRuntimePolicyAccess;

impl RuntimePolicyAccess for UnavailableRuntimePolicyAccess {
    fn inspect_artifact(&self, _digest: &str) -> HostAccessResult<RuntimeArtifactPolicy> {
        Err(HostAccessError::Unavailable)
    }

    fn list_components(&self) -> HostAccessResult<Vec<RuntimePolicyComponent>> {
        Err(HostAccessError::Unavailable)
    }

    fn grant(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _permission: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }

    fn revoke(
        &self,
        _scope_id: &str,
        _instance_id: &str,
        _component_id: &str,
        _permission: &str,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableRuntimeContextAccess;

impl RuntimeContextAccess for UnavailableRuntimeContextAccess {
    fn world_id_for_scope(
        &self,
        _scope_id: &rintawa_sdk::types::RuntimeScopeId,
    ) -> HostAccessResult<Option<rintawa_sdk::world::WorldId>> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableCompositionAccess;

impl CompositionAccess for UnavailableCompositionAccess {
    fn list_activations(&self) -> HostAccessResult<Vec<CompositionActivation>> {
        Err(HostAccessError::Unavailable)
    }

    fn select_artifact(
        &self,
        _digest: &str,
        _enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation> {
        Err(HostAccessError::Unavailable)
    }

    fn set_enabled(&self, _subject: &str, _enabled: bool) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }

    fn set_world_default(&self, _subject: &str, _world_default: bool) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }

    fn remove_activation(&self, _subject: &str) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Clone)]
pub(crate) struct HostAccessServices {
    pub(crate) runtime_context: Arc<dyn RuntimeContextAccess>,
    pub(crate) artifact_store: Arc<dyn ArtifactStoreAccess>,
    pub(crate) asset_store: Arc<dyn AssetStoreAccess>,
    pub(crate) user_resources: Arc<dyn UserResourceAccess>,
    pub(crate) world_sessions: Arc<dyn WorldSessionAccess>,
    pub(crate) composition: Arc<dyn CompositionAccess>,
    pub(crate) preferences: Arc<dyn PreferenceAccess>,
    pub(crate) runtime_policy: Arc<dyn RuntimePolicyAccess>,
}

impl HostAccessServices {
    pub(crate) fn new(
        artifact_store: Arc<dyn ArtifactStoreAccess>,
        asset_store: Arc<dyn AssetStoreAccess>,
        world_sessions: Arc<dyn WorldSessionAccess>,
        composition: Arc<dyn CompositionAccess>,
        preferences: Arc<dyn PreferenceAccess>,
        runtime_policy: Arc<dyn RuntimePolicyAccess>,
    ) -> Self {
        Self {
            runtime_context: Arc::new(UnavailableRuntimeContextAccess),
            artifact_store,
            asset_store,
            user_resources: Arc::new(UnavailableUserResourceAccess),
            world_sessions,
            composition,
            preferences,
            runtime_policy,
        }
    }

    pub(crate) fn with_runtime_context_access(
        mut self,
        runtime_context: Arc<dyn RuntimeContextAccess>,
    ) -> Self {
        self.runtime_context = runtime_context;
        self
    }

    pub(crate) fn with_user_resource_access(
        mut self,
        user_resources: Arc<dyn UserResourceAccess>,
    ) -> Self {
        self.user_resources = user_resources;
        self
    }
    pub(crate) fn unavailable() -> Self {
        Self {
            runtime_context: Arc::new(UnavailableRuntimeContextAccess),
            artifact_store: Arc::new(UnavailableArtifactStoreAccess),
            asset_store: Arc::new(UnavailableAssetStoreAccess),
            user_resources: Arc::new(UnavailableUserResourceAccess),
            world_sessions: Arc::new(UnavailableWorldSessionAccess),
            composition: Arc::new(UnavailableCompositionAccess),
            preferences: Arc::new(UnavailablePreferenceAccess),
            runtime_policy: Arc::new(UnavailableRuntimePolicyAccess),
        }
    }
}
