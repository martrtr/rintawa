//! Generic host-owned local artifact, asset, user-content, and composition access.
//!
//! These traits deliberately contain no repository, update, marketplace, package-manager,
//! or network-source semantics. The production host can expose them to any explicitly
//! authorized component principal.

use std::sync::Arc;

use rintawa_sdk::contracts::ComponentRef;

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

/// One logical item in the persistent generic user-content library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContentSummary {
    /// Stable host-local logical content identity.
    pub id: String,
    /// Exact versioned RTW content type.
    pub content: String,
    /// Exact immutable RTW revision digest.
    pub revision: String,
}

/// One generic user-content item plus its bounded root descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContentDocument {
    /// Logical item metadata.
    pub metadata: UserContentSummary,
    /// Exact bounded bytes stored at the RTW root entry path.
    pub descriptor: Vec<u8>,
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
    /// A user-content logical identifier is malformed.
    InvalidUserContentId,
    /// A user-content type filter is malformed.
    InvalidContentType,
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

/// Generic read-only access to the persistent user-content library.
pub trait UserContentAccess: Send + Sync {
    /// Lists logical items, optionally restricted to one exact versioned content type.
    fn list_user_content(&self, content: Option<&str>)
    -> HostAccessResult<Vec<UserContentSummary>>;

    /// Reads one bounded root content descriptor by logical identity.
    fn read_user_content(&self, id: &str) -> HostAccessResult<UserContentDocument>;
}

/// Host-generated identity for one accepted deferred user-content write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedUserContentWrite {
    /// Opaque operation identity scoped to the exact requesting component.
    pub operation_id: String,
}

/// Current state of one deferred user-content write operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserContentWriteStatus {
    /// The request is accepted but has not finished handler validation/publication.
    Pending,
    /// The request published one exact immutable revision into the logical library.
    Succeeded(UserContentSummary),
    /// Validation, handler policy, or persistence rejected the request.
    Failed(String),
}

/// Generic deferred mutation access to the persistent user-content library.
///
/// Requests are owner-scoped and must execute only after the current guest callback
/// unwinds so extension-provided content handlers are never re-entered synchronously.
pub trait UserContentWriteAccess: Send + Sync {
    /// Queues one new RTW content revision for handler validation and publication.
    fn request_import(
        &self,
        owner: &ComponentRef,
        rtw: &[u8],
    ) -> HostAccessResult<AcceptedUserContentWrite>;

    /// Queues replacement of one logical item while preserving its content type.
    fn request_replace(
        &self,
        owner: &ComponentRef,
        id: &str,
        rtw: &[u8],
    ) -> HostAccessResult<AcceptedUserContentWrite>;

    /// Reads status only when the operation belongs to the exact requesting component.
    fn write_status(
        &self,
        owner: &ComponentRef,
        operation_id: &str,
    ) -> HostAccessResult<UserContentWriteStatus>;
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

/// Actor selected for one host-authenticated authoritative world command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldCommandActor {
    /// The host-authenticated outside principal acts directly.
    Principal,
    /// The authenticated principal acts through one world entity subject to ControlGrant policy.
    Entity(String),
}

/// One bounded command request accepted from an ordinary extension component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldCommandRequest {
    /// Target authoritative world in canonical textual form.
    pub world_id: String,
    /// Exact versioned command schema.
    pub schema: String,
    /// Role on whose behalf the authenticated principal requests the command.
    pub actor: WorldCommandActor,
    /// Optional optimistic-concurrency world position.
    pub expected_position: Option<u64>,
    /// Extension-owned JSON command payload bytes.
    pub payload_json: Vec<u8>,
}

/// Stable identities allocated by the host for one accepted deferred world command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedWorldCommand {
    /// Host-generated idempotency identity of the command.
    pub command_id: String,
    /// Host-generated correlation identity of the new operation chain.
    pub correlation_id: String,
}

/// Failure returned by generic authoritative world-command submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldCommandAccessError {
    /// Target WorldId is malformed.
    InvalidWorldId,
    /// Versioned command schema is malformed.
    InvalidSchema,
    /// Entity actor identity is malformed.
    InvalidActor,
    /// Command payload is not valid JSON.
    InvalidPayload,
    /// Target persistent world does not exist.
    NotFound,
    /// Target world is neither active nor accepted for deferred activation.
    WorldNotActive,
    /// Host-owned deferred command queue has no remaining capacity.
    QueueFull,
    /// Host policy or persistent state rejected the request.
    Rejected,
    /// Command submission is unavailable in the current host.
    Unavailable,
}

/// Result used by generic authoritative world-command submission.
pub type WorldCommandAccessResult<T> = Result<T, WorldCommandAccessError>;

/// Generic deferred submission boundary for authoritative world commands.
///
/// The guest never supplies a PrincipalId. Implementations bind the command to the
/// already authenticated host principal before it reaches authoritative validation.
pub trait WorldCommandAccess: Send + Sync {
    /// Validates and queues one command for execution after the current guest callback unwinds.
    fn submit_world_command(
        &self,
        request: WorldCommandRequest,
    ) -> WorldCommandAccessResult<AcceptedWorldCommand>;
}

/// One bounded policy-filtered projection request accepted from an ordinary extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldProjectionRequest {
    /// Target authoritative World in canonical textual form.
    pub world_id: String,
    /// Exact versioned Projection schema.
    pub schema: String,
    /// Extension-owned JSON input bytes.
    pub input_json: Vec<u8>,
}

/// Host-generated identity for one accepted deferred projection read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedWorldProjectionRead {
    /// Opaque operation identity scoped to the exact requesting component.
    pub operation_id: String,
}

/// One immutable policy-filtered view returned by an authoritative World Projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldProjectionReadView {
    /// Authoritative World represented by the view.
    pub world_id: String,
    /// Exact pinned World position used to construct the view.
    pub snapshot_position: u64,
    /// Exact versioned Projection schema.
    pub schema: String,
    /// Schema-validated feature-owned JSON view bytes.
    pub value_json: Vec<u8>,
}

/// Current state of one owner-scoped deferred projection read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldProjectionReadStatus {
    /// The request is accepted but has not yet been evaluated by the Host.
    Pending,
    /// Projection policy completed with one validated immutable view.
    Succeeded(WorldProjectionReadView),
    /// Projection evaluation failed after admission.
    Failed(String),
}

/// Failure returned while validating or inspecting a deferred projection read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldProjectionAccessError {
    /// Target WorldId is malformed.
    InvalidWorldId,
    /// Versioned Projection schema is malformed.
    InvalidSchema,
    /// Projection input is not valid JSON or exceeds the protocol bound.
    InvalidInput,
    /// Target persistent World does not exist.
    NotFound,
    /// Target World is neither active nor accepted for deferred activation.
    WorldNotActive,
    /// Host-owned deferred projection queue has no remaining capacity.
    QueueFull,
    /// Host policy rejected the operation or owner-scoped status lookup.
    Rejected,
    /// Projection access is unavailable in the current Host.
    Unavailable,
}

/// Result used by generic deferred policy-filtered World Projection reads.
pub type WorldProjectionAccessResult<T> = Result<T, WorldProjectionAccessError>;

/// Generic deferred read boundary for Principal-filtered authoritative World Projections.
///
/// The guest never supplies a PrincipalId. Implementations bind evaluation to the already
/// authenticated Host principal and keep operation status scoped to the exact component owner.
pub trait WorldProjectionAccess: Send + Sync {
    /// Validates and queues one projection request after the current guest callback unwinds.
    fn request_projection(
        &self,
        owner: &ComponentRef,
        request: WorldProjectionRequest,
    ) -> WorldProjectionAccessResult<AcceptedWorldProjectionRead>;

    /// Reads status only when the operation belongs to the exact requesting component.
    fn projection_status(
        &self,
        owner: &ComponentRef,
        operation_id: &str,
    ) -> WorldProjectionAccessResult<WorldProjectionReadStatus>;
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
struct UnavailableUserContentAccess;

impl UserContentAccess for UnavailableUserContentAccess {
    fn list_user_content(
        &self,
        _content: Option<&str>,
    ) -> HostAccessResult<Vec<UserContentSummary>> {
        Err(HostAccessError::Unavailable)
    }

    fn read_user_content(&self, _id: &str) -> HostAccessResult<UserContentDocument> {
        Err(HostAccessError::Unavailable)
    }
}

struct UnavailableUserContentWriteAccess;

impl UserContentWriteAccess for UnavailableUserContentWriteAccess {
    fn request_import(
        &self,
        _owner: &ComponentRef,
        _rtw: &[u8],
    ) -> HostAccessResult<AcceptedUserContentWrite> {
        Err(HostAccessError::Unavailable)
    }

    fn request_replace(
        &self,
        _owner: &ComponentRef,
        _id: &str,
        _rtw: &[u8],
    ) -> HostAccessResult<AcceptedUserContentWrite> {
        Err(HostAccessError::Unavailable)
    }

    fn write_status(
        &self,
        _owner: &ComponentRef,
        _operation_id: &str,
    ) -> HostAccessResult<UserContentWriteStatus> {
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
struct UnavailableWorldCommandAccess;

impl WorldCommandAccess for UnavailableWorldCommandAccess {
    fn submit_world_command(
        &self,
        _request: WorldCommandRequest,
    ) -> WorldCommandAccessResult<AcceptedWorldCommand> {
        Err(WorldCommandAccessError::Unavailable)
    }
}

#[derive(Default)]
struct UnavailableWorldProjectionAccess;

impl WorldProjectionAccess for UnavailableWorldProjectionAccess {
    fn request_projection(
        &self,
        _owner: &ComponentRef,
        _request: WorldProjectionRequest,
    ) -> WorldProjectionAccessResult<AcceptedWorldProjectionRead> {
        Err(WorldProjectionAccessError::Unavailable)
    }

    fn projection_status(
        &self,
        _owner: &ComponentRef,
        _operation_id: &str,
    ) -> WorldProjectionAccessResult<WorldProjectionReadStatus> {
        Err(WorldProjectionAccessError::Unavailable)
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
    pub(crate) user_content: Arc<dyn UserContentAccess>,
    pub(crate) user_content_write: Arc<dyn UserContentWriteAccess>,
    pub(crate) world_sessions: Arc<dyn WorldSessionAccess>,
    pub(crate) world_commands: Arc<dyn WorldCommandAccess>,
    pub(crate) world_projections: Arc<dyn WorldProjectionAccess>,
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
            user_content: Arc::new(UnavailableUserContentAccess),
            user_content_write: Arc::new(UnavailableUserContentWriteAccess),
            world_sessions,
            world_commands: Arc::new(UnavailableWorldCommandAccess),
            world_projections: Arc::new(UnavailableWorldProjectionAccess),
            composition,
            preferences,
            runtime_policy,
        }
    }

    pub(crate) fn with_user_content_access(
        mut self,
        user_content: Arc<dyn UserContentAccess>,
    ) -> Self {
        self.user_content = user_content;
        self
    }

    pub(crate) fn with_user_content_write_access(
        mut self,
        user_content_write: Arc<dyn UserContentWriteAccess>,
    ) -> Self {
        self.user_content_write = user_content_write;
        self
    }

    pub(crate) fn with_world_command_access(
        mut self,
        world_commands: Arc<dyn WorldCommandAccess>,
    ) -> Self {
        self.world_commands = world_commands;
        self
    }

    pub(crate) fn with_world_projection_access(
        mut self,
        world_projections: Arc<dyn WorldProjectionAccess>,
    ) -> Self {
        self.world_projections = world_projections;
        self
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            artifact_store: Arc::new(UnavailableArtifactStoreAccess),
            asset_store: Arc::new(UnavailableAssetStoreAccess),
            user_content: Arc::new(UnavailableUserContentAccess),
            user_content_write: Arc::new(UnavailableUserContentWriteAccess),
            world_sessions: Arc::new(UnavailableWorldSessionAccess),
            world_commands: Arc::new(UnavailableWorldCommandAccess),
            world_projections: Arc::new(UnavailableWorldProjectionAccess),
            composition: Arc::new(UnavailableCompositionAccess),
            preferences: Arc::new(UnavailablePreferenceAccess),
            runtime_policy: Arc::new(UnavailableRuntimePolicyAccess),
        }
    }
}
