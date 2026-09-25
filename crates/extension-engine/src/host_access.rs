//! Generic host-owned local artifact, asset, and composition access for sandboxed extensions.
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

/// One persistent world plus its host runtime lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSessionSummary {
    /// Stable persistent world identity in canonical textual form.
    pub world_id: String,
    /// Last committed authoritative world position.
    pub commit_position: u64,
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

/// Generic immutable RTW artifact-store operations.
pub trait ArtifactStoreAccess: Send + Sync {
    /// Validates and imports exact RTW bytes without selecting them for activation.
    fn import_rtw(&self, bytes: &[u8]) -> HostAccessResult<ImportedArtifact>;
}

/// Generic immutable raw-asset store operations.
pub trait AssetStoreAccess: Send + Sync {
    /// Imports exact bytes and returns their immutable digest/size/media reference.
    fn import_asset(&self, bytes: &[u8], media_type: &str) -> HostAccessResult<ImportedAsset>;
}

/// Generic persistent-world catalog and lifecycle request operations.
pub trait WorldSessionAccess: Send + Sync {
    /// Lists persistent worlds together with current/pending runtime lifecycle state.
    fn list_worlds(&self) -> HostAccessResult<Vec<WorldSessionSummary>>;

    /// Creates one empty persistent authoritative world.
    fn create_world(&self) -> HostAccessResult<WorldSessionSummary>;

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

    /// Marks whether one baseline selection is inherited by newly created worlds.
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

    fn set_active(&self, _world_id: &str, _active: bool) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
}

#[derive(Default)]
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
    pub(crate) artifact_store: Arc<dyn ArtifactStoreAccess>,
    pub(crate) asset_store: Arc<dyn AssetStoreAccess>,
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
            artifact_store,
            asset_store,
            world_sessions,
            composition,
            preferences,
            runtime_policy,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            artifact_store: Arc::new(UnavailableArtifactStoreAccess),
            asset_store: Arc::new(UnavailableAssetStoreAccess),
            world_sessions: Arc::new(UnavailableWorldSessionAccess),
            composition: Arc::new(UnavailableCompositionAccess),
            preferences: Arc::new(UnavailablePreferenceAccess),
            runtime_policy: Arc::new(UnavailableRuntimePolicyAccess),
        }
    }
}
