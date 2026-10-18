//! Local host composition for exact RTW artifacts.
//!
//! This crate intentionally contains no repository, HTTP, update, or version-selection
//! policy. It persists exact artifact activations that a CLI, installer, or package
//! manager can request after obtaining RTW bytes by any means.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod identity;
mod profile;
mod runtime;
mod runtime_signal;
mod user_content;
mod world_catalog;

use std::{
    fmt,
    path::{Path, PathBuf},
};

use rintawa_artifacts::{
    ArtifactDigest, ArtifactStore, AssetStore, ContentType, ImportDisposition, RtwArchive,
    RtwLimits,
};
use rintawa_extension_engine::{
    ActivationPlanError, DeferredExecutionTarget, EngineError, ExtensionEngine, RtwExtensionLoader,
    UnresolvedContractReason,
};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey},
    runtime_permissions::RuntimePermission,
    types::{ComponentId, ExtensionInstanceId, RuntimeScopeId},
    ui::WorldPresentationDescriptor,
    world::{PrincipalId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::WorldSessionState;
use thiserror::Error;
use uuid::Uuid;

pub use identity::HOST_IDENTITY_SCHEMA;
pub use profile::{
    ActivationRecord, BaselineProfile, CompositionProfile, PROFILE_SCHEMA,
    PreferredProviderSelection, RuntimePermissionGrant,
};
pub use runtime::{
    EFFECT_JOB_LEASE_MILLIS, EFFECT_RETRY_BASE_MILLIS, EFFECT_RETRY_MAX_MILLIS, HostRuntime,
    MAX_EFFECT_JOBS_PER_PUMP, WorldCommandDispatchOutcome,
};
pub use runtime_signal::{
    DEFAULT_RUNTIME_SIGNAL_QUEUE_CAPACITY, MAX_RUNTIME_SIGNAL_MESSAGE_BYTES,
    MAX_RUNTIME_SIGNAL_TOPIC_BYTES,
};
pub use user_content::{USER_CONTENT_LIBRARY_SCHEMA, UserContentEntry, UserContentId};
pub use world_catalog::{
    MAX_WORLD_TITLE_BYTES, WORLD_CATALOG_METADATA_FILE, WORLD_CATALOG_METADATA_SCHEMA,
    WorldCatalogMetadata,
};

/// Stable runtime scope used by the pre-world/bootstrap composition.
pub const HOST_SCOPE: &str = "host";
/// Host-owned template scope materialized into newly created Worlds.
///
/// This scope is persisted composition policy only and is never activated as a runtime.
pub const DEFAULT_WORLD_SCOPE: &str = "default-world";
/// Prefix used for host-owned per-world runtime composition scopes.
pub const WORLD_SCOPE_PREFIX: &str = "world:";

/// Returns the canonical runtime scope owned by one authoritative world.
pub fn world_runtime_scope_id(world_id: WorldId) -> RuntimeScopeId {
    RuntimeScopeId::new(format!("{WORLD_SCOPE_PREFIX}{world_id}"))
}

/// File name of the current baseline host profile.
pub const BASELINE_PROFILE_FILE: &str = "baseline.toml";
/// File name of the composition recipe materialized into newly created Worlds.
pub const DEFAULT_WORLD_PROFILE_FILE: &str = "default-world.toml";
const EXTENSION_CONTENT_V1: &str = "rintawa.extension@1";
const HOST_IDENTITY_FILE: &str = "identity.toml";
const WORLD_DATABASE_FILE: &str = "world.sqlite";
const WORLD_COMPOSITION_FILE: &str = "composition.toml";
const WORLD_ID_CREATION_ATTEMPTS: usize = 8;
/// Maximum single raw asset size accepted by the local host asset store.
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// One composition activation deferred because required execution targets are unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeferredActivation {
    /// Handler-defined activation identity from the composition profile.
    pub subject: String,
    /// Concrete runtime instance that could not be registered yet.
    pub instance_id: ExtensionInstanceId,
    /// Logical extension identity read from the validated artifact manifest.
    pub extension_id: String,
    /// Required execution targets currently missing from the Engine registry.
    pub missing_required_targets: Vec<DeferredExecutionTarget>,
}

/// One registered composition activation blocked by required contract composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapBlockedActivation {
    /// Concrete runtime instance that could not enter `Active`.
    pub instance_id: ExtensionInstanceId,
    /// Exact activation-planner reason observed for this instance.
    pub reason: ActivationPlanError,
}

/// Structured diagnostics produced when composition activation reaches a fixed point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapStall {
    /// Activations waiting for execution targets that never appeared.
    pub deferred: Vec<BootstrapDeferredActivation>,
    /// Registered activations blocked by required contract composition.
    pub blocked: Vec<BootstrapBlockedActivation>,
    /// Batch-level planner failure, including dependency-cycle diagnostics when available.
    pub batch_error: Option<ActivationPlanError>,
}

impl fmt::Display for BootstrapStall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("composition activation reached a fixed point")?;
        for activation in &self.deferred {
            write!(
                formatter,
                "; instance `{}` ({}) waits for",
                activation.instance_id, activation.extension_id
            )?;
            for missing in &activation.missing_required_targets {
                write!(
                    formatter,
                    " component `{}` target `{}`",
                    missing.component_id, missing.target
                )?;
            }
        }
        for activation in &self.blocked {
            write!(
                formatter,
                "; instance `{}` is blocked: {}",
                activation.instance_id, activation.reason
            )?;
        }
        if let Some(error) = &self.batch_error {
            write!(formatter, "; batch activation plan: {error}")?;
        }
        Ok(())
    }
}

/// Host lifecycle cleanup operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCleanupOperation {
    /// Stopping an active extension instance.
    Stop,
    /// Unregistering an extension instance.
    Unregister,
}

impl fmt::Display for HostCleanupOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stop => formatter.write_str("stop"),
            Self::Unregister => formatter.write_str("unregister"),
        }
    }
}

/// One caller-visible failure encountered while cleaning up host lifecycle state.
#[derive(Debug)]
pub struct HostCleanupFailure {
    /// Runtime instance whose cleanup operation failed.
    pub instance_id: ExtensionInstanceId,
    /// Lifecycle operation that failed.
    pub operation: HostCleanupOperation,
    /// Exact Engine lifecycle error.
    pub error: EngineError,
}

/// Bootstrap error plus every cleanup failure observed while rolling back partial startup.
#[derive(Debug)]
pub struct BootstrapRollback {
    /// Original bootstrap failure that triggered rollback.
    pub primary: Box<HostError>,
    /// Stop/unregister failures observed while cleanup continued.
    pub cleanup_failures: Vec<HostCleanupFailure>,
}

impl fmt::Display for BootstrapRollback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "bootstrap failed: {}", self.primary)?;
        for failure in &self.cleanup_failures {
            write!(
                formatter,
                "; rollback {} for instance `{}` failed: {}",
                failure.operation, failure.instance_id, failure.error
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for BootstrapRollback {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

/// One failure observed while stopping an active authoritative world runtime.
#[derive(Debug)]
pub struct WorldRuntimeCleanupFailure {
    /// World whose worker could not shut down cleanly.
    pub world_id: WorldId,
    /// Exact authoritative runtime shutdown error.
    pub error: rintawa_world_runtime::WorldRuntimeError,
}

/// Aggregated failures observed while shutting down a running host composition.
#[derive(Debug)]
pub struct HostShutdownFailures {
    /// World runtime failures observed while shutdown continued.
    pub world_failures: Vec<WorldRuntimeCleanupFailure>,
    /// Stop/unregister failures observed while shutdown continued.
    pub cleanup_failures: Vec<HostCleanupFailure>,
}

impl fmt::Display for HostShutdownFailures {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("host shutdown cleanup failed")?;
        for failure in &self.world_failures {
            write!(
                formatter,
                "; world `{}` shutdown failed: {}",
                failure.world_id, failure.error
            )?;
        }
        for failure in &self.cleanup_failures {
            write!(
                formatter,
                "; {} for instance `{}` failed: {}",
                failure.operation, failure.instance_id, failure.error
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for HostShutdownFailures {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.world_failures
            .first()
            .map(|failure| &failure.error as &(dyn std::error::Error + 'static))
            .or_else(|| {
                self.cleanup_failures
                    .first()
                    .map(|failure| &failure.error as &(dyn std::error::Error + 'static))
            })
    }
}

/// Errors returned by local host composition operations.
#[derive(Debug, Error)]
pub enum HostError {
    /// An RTW artifact operation failed.
    #[error(transparent)]
    Artifact(#[from] rintawa_artifacts::RtwError),
    /// A raw asset reference/store operation failed.
    #[error(transparent)]
    Asset(#[from] rintawa_artifacts::AssetError),
    /// Extension inspection or lifecycle failed.
    #[error(transparent)]
    Engine(#[from] rintawa_extension_engine::EngineError),
    /// Authoritative world storage failed.
    #[error(transparent)]
    Storage(#[from] rintawa_storage::StorageError),
    /// Authoritative world command runtime failed.
    #[error(transparent)]
    WorldRuntime(#[from] rintawa_world_runtime::WorldRuntimeError),
    /// A Principal-specific world projection could not be built safely.
    #[error(transparent)]
    WorldProjection(#[from] rintawa_world_runtime::WorldProjectionError),
    /// No registered Projection schema is available under this exact key.
    #[error("world projection `{0}` is not available in the active world")]
    WorldProjectionUnavailable(SchemaKey),
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A world directory violates the host-owned storage layout.
    #[error("invalid world directory `{path}`: {reason}")]
    InvalidWorldDirectory {
        /// Rejected directory path.
        path: PathBuf,
        /// Layout invariant that was violated.
        reason: &'static str,
    },
    /// The directory identity and embedded database identity disagree.
    #[error("world directory `{directory_id}` contains database for `{database_id}`")]
    WorldDirectoryIdMismatch {
        /// World ID encoded in the directory name.
        directory_id: WorldId,
        /// World ID embedded in the SQLite database.
        database_id: WorldId,
    },
    /// A requested local world does not exist.
    #[error("world `{0}` is not present in the local host home")]
    WorldNotFound(WorldId),
    /// Human-facing World title is blank or exceeds the platform bound.
    #[error("world title must contain 1..={maximum_bytes} UTF-8 bytes; found {actual_bytes}")]
    InvalidWorldTitle {
        /// UTF-8 byte length after trimming surrounding whitespace.
        actual_bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
    /// Human-facing World description exceeds the platform bound.
    #[error(
        "world description must contain at most {maximum_bytes} UTF-8 bytes; found {actual_bytes}"
    )]
    InvalidWorldDescription {
        /// UTF-8 byte length after trimming surrounding whitespace.
        actual_bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
    /// A World catalog metadata sidecar is absent where one was explicitly requested.
    #[error("world catalog metadata file `{0}` is missing")]
    WorldCatalogMetadataMissing(PathBuf),
    /// A World catalog metadata path is not a safe regular file.
    #[error("invalid world catalog metadata file `{0}`")]
    InvalidWorldCatalogMetadataFile(PathBuf),
    /// Persisted World catalog metadata could not be decoded.
    #[error("invalid world catalog metadata: {source}")]
    WorldCatalogMetadataDecode {
        /// TOML decoding failure.
        #[source]
        source: toml::de::Error,
    },
    /// World catalog metadata could not be encoded.
    #[error("failed to encode world catalog metadata: {source}")]
    WorldCatalogMetadataEncode {
        /// TOML encoding failure.
        #[source]
        source: toml::ser::Error,
    },
    /// Persisted World catalog metadata uses a newer schema.
    #[error("unsupported world catalog metadata schema {0}")]
    UnsupportedWorldCatalogMetadataSchema(u32),
    /// A world runtime is already active in this host process.
    #[error("world `{0}` is already active")]
    WorldAlreadyActive(WorldId),
    /// A world runtime is not active in this host process.
    #[error("world `{0}` is not active")]
    WorldNotActive(WorldId),
    /// The selected World presentation entry surface is not currently mounted.
    #[error("world `{world_id}` presentation entry surface `{surface_id}` is not mounted")]
    WorldPresentationEntryUnavailable {
        /// Active World whose presentation could not be focused.
        world_id: WorldId,
        /// Exact owner-scoped entry surface declared by the selected provider.
        surface_id: String,
    },
    /// Persisted composition policy contains a record for another runtime scope.
    #[error("composition scope mismatch: expected `{expected_scope}`, found `{actual_scope}`")]
    CompositionScopeMismatch {
        /// Scope owning the profile file.
        expected_scope: String,
        /// Scope encoded by an invalid record.
        actual_scope: String,
    },
    /// Host persistence has no composition owner for this runtime scope.
    #[error("unsupported persistent composition scope `{0}`")]
    UnsupportedCompositionScope(String),
    /// Repeated UUID generation unexpectedly collided with existing world directories.
    #[error("failed to allocate a unique world identifier")]
    WorldIdCollision,
    /// World creation failed and cleanup of the partial directory also failed.
    #[error("world creation failed: {creation}; cleanup also failed: {cleanup}")]
    WorldCreateRollbackFailed {
        /// Original storage creation failure.
        creation: String,
        /// Filesystem cleanup failure.
        cleanup: String,
    },
    /// A runtime event envelope could not be serialized for extension delivery.
    #[error("failed to encode runtime event envelope: {0}")]
    EventEnvelopeEncode(#[from] serde_json::Error),
    /// An extension declared malformed JSON for one immutable world schema.
    #[error("world schema `{schema}` contains invalid JSON: {source}")]
    InvalidWorldSchemaJson {
        /// Rejected versioned schema identity.
        schema: rintawa_sdk::world::SchemaKey,
        /// JSON syntax error reported before durable publication.
        #[source]
        source: serde_json::Error,
    },
    /// One delivery batch mixed events owned by different authoritative worlds.
    #[error("world event delivery batch contains multiple WorldIds")]
    MixedWorldEventBatch,
    /// An ephemeral runtime signal used an empty or oversized routing topic.
    #[error("runtime signal topic must be non-empty and within host bounds")]
    InvalidRuntimeSignalTopic,
    /// Shared deferred user-content write control state became unavailable.
    #[error("user-content deferred write control is unavailable")]
    UserContentWriteControlUnavailable,
    /// Shared deferred world-session lifecycle control state became unavailable.
    #[error("world-session lifecycle control is unavailable")]
    WorldSessionControlUnavailable,
    /// Shared deferred world-command control state became unavailable.
    #[error("world-command deferred control is unavailable")]
    WorldCommandControlUnavailable,
    /// Shared deferred world-projection control state became unavailable.
    #[error("world-projection deferred control is unavailable")]
    WorldProjectionControlUnavailable,
    /// An active world's bounded ephemeral signal queue has no remaining capacity.
    #[error("runtime signal queue for world `{0}` is full")]
    RuntimeSignalQueueFull(WorldId),
    /// A runtime signal envelope exceeded the host message bound.
    #[error("runtime signal envelope is {actual_bytes} bytes; maximum is {maximum_bytes}")]
    RuntimeSignalMessageTooLarge {
        /// Encoded envelope size observed by the host.
        actual_bytes: usize,
        /// Maximum encoded envelope size accepted by the host.
        maximum_bytes: usize,
    },
    /// A runtime signal envelope could not be serialized for extension delivery.
    #[error("failed to encode runtime signal envelope: {source}")]
    RuntimeSignalEncode {
        /// Serialization failure from the host envelope codec.
        #[source]
        source: serde_json::Error,
    },
    /// Host identity metadata path is not a safe regular file.
    #[error("invalid host identity file `{path}`: {reason}")]
    InvalidHostIdentityFile {
        /// Rejected host identity path.
        path: PathBuf,
        /// Identity storage invariant that was violated.
        reason: &'static str,
    },
    /// Persisted host identity metadata could not be decoded.
    #[error("invalid host identity metadata: {0}")]
    HostIdentityDecode(toml::de::Error),
    /// Host identity metadata could not be encoded.
    #[error("failed to encode host identity metadata: {0}")]
    HostIdentityEncode(toml::ser::Error),
    /// Persisted host identity schema is newer than this host understands.
    #[error("unsupported host identity schema {0}")]
    UnsupportedHostIdentitySchema(u32),
    /// Persisted host state could not be decoded.
    #[error("invalid host profile: {0}")]
    ProfileDecode(#[from] toml::de::Error),
    /// Host state could not be encoded.
    #[error("failed to encode host profile: {0}")]
    ProfileEncode(#[from] toml::ser::Error),
    /// The persisted profile schema is newer than this host understands.
    #[error("unsupported host profile schema {0}")]
    UnsupportedProfileSchema(u32),
    /// No activation with the requested subject exists in the selected composition.
    #[error("activation `{0}` is not installed in the selected composition")]
    ActivationNotFound(String),
    /// The baseline profile contains multiple choices for the same scoped contract.
    #[error(
        "duplicate preferred provider selection for contract `{contract}` in scope `{scope_id}`"
    )]
    DuplicatePreferredProviderSelection {
        /// Runtime scope containing the duplicate selection.
        scope_id: String,
        /// Contract selected more than once.
        contract: String,
    },
    /// The baseline profile contains the same runtime capability approval more than once.
    #[error(
        "duplicate runtime permission `{permission}` for component `{component_id}` in instance `{instance_id}` scope `{scope_id}`"
    )]
    DuplicateRuntimePermissionGrant {
        /// Runtime scope containing the duplicate approval.
        scope_id: String,
        /// Runtime instance receiving the duplicate approval.
        instance_id: String,
        /// Component receiving the duplicate approval.
        component_id: String,
        /// Runtime permission approved more than once.
        permission: String,
    },
    /// A preferred provider references an activation not installed in the baseline profile.
    #[error("preferred provider instance `{0}` is not installed in the baseline profile")]
    PreferredProviderActivationNotFound(String),
    /// A preferred provider points at an activation in another runtime scope.
    #[error(
        "preferred provider instance `{instance_id}` belongs to scope `{actual_scope}`, not `{selected_scope}`"
    )]
    PreferredProviderScopeMismatch {
        /// Provider runtime instance.
        instance_id: String,
        /// Scope persisted by the provider selection.
        selected_scope: String,
        /// Scope of the installed activation.
        actual_scope: String,
    },
    /// A runtime permission approval references an activation absent from the baseline profile.
    #[error("runtime permission instance `{0}` is not installed in the baseline profile")]
    RuntimePermissionActivationNotFound(String),
    /// A runtime permission approval references an activation in another runtime scope.
    #[error(
        "runtime permission instance `{instance_id}` belongs to scope `{actual_scope}`, not `{selected_scope}`"
    )]
    RuntimePermissionScopeMismatch {
        /// Runtime instance receiving the approval.
        instance_id: String,
        /// Scope selected by policy.
        selected_scope: String,
        /// Scope containing the installed activation.
        actual_scope: String,
    },
    /// A runtime permission approval references a component absent from the exact artifact.
    #[error(
        "component `{component_id}` is not present in runtime permission instance `{instance_id}`"
    )]
    RuntimePermissionComponentNotFound {
        /// Runtime instance containing the exact artifact.
        instance_id: String,
        /// Missing component identifier.
        component_id: String,
    },
    /// A runtime permission approval would exceed the exact artifact manifest request.
    #[error(
        "runtime permission `{permission}` was not requested by component `{component_id}` in instance `{instance_id}`"
    )]
    RuntimePermissionNotRequested {
        /// Runtime instance containing the component.
        instance_id: String,
        /// Component whose manifest did not request the permission.
        component_id: String,
        /// Permission policy attempted to approve.
        permission: String,
    },

    /// A host-consumed contract role could not resolve its selected active provider.
    #[error("contract `{contract}` in scope `{scope_id}` is unavailable: {reason}")]
    ContractRoleUnavailable {
        /// Runtime scope containing the role.
        scope_id: String,
        /// Versioned contract key.
        contract: String,
        /// Provider-resolution failure.
        reason: UnresolvedContractReason,
    },
    /// Baseline bootstrap reached a fixed point without satisfying every activation.
    #[error("{0}")]
    BootstrapStalled(Box<BootstrapStall>),
    /// Baseline bootstrap failed and rollback also reported lifecycle failures.
    #[error("{0}")]
    BootstrapRollback(Box<BootstrapRollback>),
    /// Running baseline shutdown reported one or more lifecycle cleanup failures.
    #[error("{0}")]
    ShutdownFailed(Box<HostShutdownFailures>),
    /// A persisted preference record duplicates the same owner/key pair.
    #[error(
        "duplicate preference `{key}` for component `{component_id}` in instance `{instance_id}` scope `{scope_id}`"
    )]
    DuplicatePreference {
        /// Runtime scope owning the preference.
        scope_id: String,
        /// Runtime instance owning the preference.
        instance_id: String,
        /// Component owning the preference.
        component_id: String,
        /// Duplicate preference key.
        key: String,
    },
    /// A preference key/value violates host bounds.
    #[error("invalid extension preference: {0}")]
    InvalidPreference(String),
    /// A component exceeded its bounded persistent preference quota.
    #[error("extension preference quota exceeded")]
    PreferenceQuotaExceeded,
    /// A persisted activation uses a content type for which this host has no handler.
    #[error("no activation handler is available for RTW content `{0}`")]
    UnsupportedContent(String),
    /// The generic user-content library path is not a regular host-owned file.
    #[error("invalid user-content library file `{0}`")]
    InvalidUserContentLibraryFile(PathBuf),
    /// The persisted generic user-content library uses an unsupported schema.
    #[error("unsupported user-content library schema {0}")]
    UnsupportedUserContentLibrarySchema(u32),
    /// The persisted generic user-content library contains a duplicate logical ID.
    #[error("duplicate user-content library id `{0}`")]
    DuplicateUserContentId(UserContentId),
    /// The requested logical user-content item is absent from the library index.
    #[error("user-content item `{0}` is not present in the library")]
    UserContentNotFound(UserContentId),
    /// An edit attempted to change the versioned content type of one logical item.
    #[error("user-content item `{id}` has type `{expected}`, not `{actual}`")]
    UserContentTypeMismatch {
        /// Stable logical content identity.
        id: UserContentId,
        /// Existing versioned content type.
        expected: String,
        /// Rejected replacement content type.
        actual: String,
    },
    /// The persisted user-content library could not be decoded.
    #[error("invalid user-content library: {source}")]
    UserContentLibraryDecode {
        /// TOML decoding failure.
        #[source]
        source: toml::de::Error,
    },
    /// The user-content library could not be encoded for persistence.
    #[error("failed to encode user-content library: {source}")]
    UserContentLibraryEncode {
        /// TOML encoding failure.
        #[source]
        source: toml::ser::Error,
    },
    /// A content-handler request could not be serialized.
    #[error("failed to encode RTW content-handler request for `{content}`: {source}")]
    ContentHandlerRequestEncode {
        /// Exact versioned RTW content type.
        content: String,
        /// JSON encoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// The selected content-handler service could not complete validation.
    #[error("RTW content handler for `{content}` is unavailable: {source}")]
    ContentHandlerCall {
        /// Exact versioned RTW content type.
        content: String,
        /// Generic service transport failure.
        #[source]
        source: rintawa_sdk::services::ServiceCallError,
    },
    /// A content handler returned malformed protocol bytes.
    #[error("RTW content handler for `{content}` returned an invalid response: {source}")]
    ContentHandlerResponseDecode {
        /// Exact versioned RTW content type.
        content: String,
        /// JSON decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A content handler rejected an immutable RTW descriptor.
    #[error("RTW content `{content}` was rejected by its handler: {diagnostic}")]
    UserContentRejected {
        /// Exact versioned RTW content type.
        content: String,
        /// Bounded handler diagnostic.
        diagnostic: String,
    },
    /// A content handler returned a rejection diagnostic above the protocol bound.
    #[error("RTW content handler diagnostic for `{content}` exceeds {maximum_bytes} bytes")]
    ContentHandlerDiagnosticTooLarge {
        /// Exact versioned RTW content type.
        content: String,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
}

/// Result type used by local host composition operations.
pub type HostResult<T> = Result<T, HostError>;

/// Summary of one installed RTW activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledActivation {
    /// Handler-defined stable identity. For `rintawa.extension@1` this is the extension ID.
    pub subject: String,
    /// RTW content type that owns this activation.
    pub content: ContentType,
    /// Human-readable extension name.
    pub name: String,
    /// Content-specific version when the handler exposes one.
    pub version: Option<String>,
    /// Exact immutable artifact selected for this activation.
    pub digest: ArtifactDigest,
    /// Concrete runtime instance identifier.
    pub instance_id: ExtensionInstanceId,
    /// Runtime composition scope.
    pub scope_id: RuntimeScopeId,
    /// Whether the selected composition activation should start automatically.
    pub enabled: bool,
    /// Compatibility view indicating that this exact enabled baseline artifact is selected
    /// in the independent default-World recipe. Always false outside the baseline view.
    pub world_default: bool,
}

/// Requested and granted runtime permissions for one exact composition component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePermissionPolicyEntry {
    /// Runtime scope containing the component.
    pub scope_id: RuntimeScopeId,
    /// Concrete runtime instance identity.
    pub instance_id: ExtensionInstanceId,
    /// Component within the runtime instance.
    pub component_id: ComponentId,
    /// Permissions requested by the exact selected artifact manifest.
    pub requested: Vec<RuntimePermission>,
    /// Host-approved permissions persisted for the exact component principal.
    pub granted: Vec<RuntimePermission>,
}

/// Result of importing and selecting a local RTW activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    /// Installed activation summary.
    pub activation: InstalledActivation,
    /// Whether new bytes entered the content-addressed store.
    pub disposition: ImportDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistedCompositionScope {
    Baseline,
    DefaultWorld,
    World(WorldId),
}

/// Summary of one persistent authoritative world available in the local host home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSummary {
    /// Stable authoritative world identity.
    pub id: WorldId,
    /// Human-facing host-owned catalog title.
    pub title: String,
    /// Optional human-facing description.
    pub description: Option<String>,
    /// Optional immutable cover asset reference.
    pub cover: Option<rintawa_artifacts::AssetRef>,
    /// Last committed local world position.
    pub commit_position: u64,
}

/// One resolved entry point for an active World's presentation in a concrete UI session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldPresentationTarget {
    /// Active authoritative World being presented.
    pub world_id: WorldId,
    /// Exact active component selected by the World's composition policy.
    pub provider: ComponentRef,
    /// Owner-scoped entry surface and optional semantic intent registered by the provider.
    pub descriptor: WorldPresentationDescriptor,
}

/// Persistent local Rintawa home containing CAS bytes, worlds, and baseline composition.
pub struct HostHome {
    root: PathBuf,
    store: ArtifactStore,
    asset_store: AssetStore,
    local_principal: PrincipalId,
    profile_path: PathBuf,
    default_world_profile_path: PathBuf,
    user_content_path: PathBuf,
    worlds_directory: PathBuf,
}

impl HostHome {
    /// Opens or creates one local Rintawa home.
    pub fn open(root: impl AsRef<Path>) -> HostResult<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let store = ArtifactStore::open(root.join("artifacts"), RtwLimits::default())?;
        let asset_store = AssetStore::open(root.join("assets"), MAX_ASSET_BYTES)?;
        let local_principal =
            identity::load_or_create_local_principal(&root.join(HOST_IDENTITY_FILE))?;
        let profiles_directory = root.join("profiles");
        ensure_real_directory(&profiles_directory)?;
        let profile_path = profiles_directory.join(BASELINE_PROFILE_FILE);
        let default_world_profile_path = profiles_directory.join(DEFAULT_WORLD_PROFILE_FILE);
        let content_directory = root.join("content");
        ensure_real_directory(&content_directory)?;
        let user_content_path = content_directory.join("library.toml");
        let worlds_directory = root.join("worlds");
        ensure_real_directory(&worlds_directory)?;
        let home = Self {
            root,
            store,
            asset_store,
            local_principal,
            profile_path,
            default_world_profile_path,
            user_content_path,
            worlds_directory,
        };
        home.migrate_legacy_world_defaults()?;
        Ok(home)
    }

    /// Returns the canonical local Rintawa home path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the stable authenticated Principal of this local single-user host.
    pub const fn local_principal(&self) -> PrincipalId {
        self.local_principal
    }

    /// Returns the immutable artifact store.
    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.store
    }

    /// Returns the immutable raw asset store used by [`rintawa_artifacts::AssetRef`].
    pub fn asset_store(&self) -> &AssetStore {
        &self.asset_store
    }

    /// Creates one empty persistent authoritative world.
    ///
    /// # Errors
    ///
    /// Returns a filesystem or storage error if the world directory/database
    /// cannot be created safely.
    pub fn create_world(&self) -> HostResult<WorldSummary> {
        let default_world = self.load_default_world_recipe()?;
        for _ in 0..WORLD_ID_CREATION_ATTEMPTS {
            let world_id = WorldId::new();
            let directory = self.worlds_directory.join(world_id.to_string());
            match std::fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }

            let database = directory.join(WORLD_DATABASE_FILE);
            let creation: HostResult<WorldSummary> =
                match SqliteWorldStorage::create(&database, world_id) {
                    Ok(storage) => (|| {
                        let state = storage.load_session()?;
                        drop(storage);
                        let metadata = WorldCatalogMetadata::new("New World", None)?;
                        metadata.save(&directory.join(WORLD_CATALOG_METADATA_FILE))?;
                        let scope_id = world_runtime_scope_id(world_id);
                        let profile = default_world.materialize_all(scope_id);
                        self.save_world_composition(world_id, &profile)?;
                        Ok(world_summary(&state, metadata))
                    })(),
                    Err(error) => Err(error.into()),
                };
            match creation {
                Ok(summary) => return Ok(summary),
                Err(error) => {
                    if let Err(cleanup) = std::fs::remove_dir_all(&directory) {
                        return Err(HostError::WorldCreateRollbackFailed {
                            creation: error.to_string(),
                            cleanup: cleanup.to_string(),
                        });
                    }
                    return Err(error);
                }
            }
        }

        Err(HostError::WorldIdCollision)
    }

    /// Lists all persistent worlds in deterministic identifier order.
    ///
    /// # Errors
    ///
    /// Returns an error if any host-owned world directory is malformed or its
    /// embedded database identity disagrees with the directory identity.
    pub fn list_worlds(&self) -> HostResult<Vec<WorldSummary>> {
        let mut entries =
            std::fs::read_dir(&self.worlds_directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);

        let mut worlds = Vec::with_capacity(entries.len());
        for (index, entry) in entries.into_iter().enumerate() {
            let directory = entry.path();
            validate_world_directory(&directory)?;
            let name =
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| HostError::InvalidWorldDirectory {
                        path: directory.clone(),
                        reason: "directory name must be a UTF-8 WorldId",
                    })?;
            let world_id =
                name.parse::<WorldId>()
                    .map_err(|_| HostError::InvalidWorldDirectory {
                        path: directory.clone(),
                        reason: "directory name must be a canonical WorldId",
                    })?;
            let storage = open_world_database(&directory, world_id)?;
            let metadata_path = directory.join(WORLD_CATALOG_METADATA_FILE);
            let metadata = match WorldCatalogMetadata::load(&metadata_path) {
                Ok(metadata) => metadata,
                Err(HostError::WorldCatalogMetadataMissing(_)) => {
                    let metadata = WorldCatalogMetadata::new(format!("World {}", index + 1), None)?;
                    metadata.save(&metadata_path)?;
                    metadata
                }
                Err(error) => return Err(error),
            };
            worlds.push(world_summary(&storage.load_session()?, metadata));
        }

        worlds.sort_by_key(|world| world.id);
        Ok(worlds)
    }

    /// Updates host-owned human-facing metadata for one persistent World.
    ///
    /// The cover, when supplied, must already exist in the immutable host asset store.
    ///
    /// # Errors
    ///
    /// Returns an error when the World is absent, title is invalid, cover verification fails,
    /// or the metadata sidecar cannot be persisted atomically.
    pub fn update_world_catalog_metadata(
        &self,
        world_id: WorldId,
        title: impl Into<String>,
        description: Option<String>,
        cover: Option<rintawa_artifacts::AssetRef>,
    ) -> HostResult<WorldSummary> {
        let directory = self.worlds_directory.join(world_id.to_string());
        let storage = self.open_world_storage(world_id)?;
        if let Some(reference) = &cover {
            self.asset_store.verify(reference)?;
        }
        let metadata = WorldCatalogMetadata::new(title, cover)?.with_description(description)?;
        metadata.save(&directory.join(WORLD_CATALOG_METADATA_FILE))?;
        Ok(world_summary(&storage.load_session()?, metadata))
    }

    /// Deletes one inactive persistent World including its exact world composition.
    ///
    /// Shared immutable artifact and asset stores are intentionally left untouched.
    ///
    /// # Errors
    ///
    /// Returns an error when the World does not exist, its directory identity is unsafe,
    /// or the host cannot remove the validated World directory.
    pub fn delete_world(&self, world_id: WorldId) -> HostResult<()> {
        let directory = self.worlds_directory.join(world_id.to_string());
        let storage = open_world_database(&directory, world_id)?;
        drop(storage);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    /// Reads host-owned human-facing metadata for one persistent World.
    ///
    /// # Errors
    ///
    /// Returns an error when the World or metadata sidecar is absent or invalid.
    pub fn load_world_catalog_metadata(
        &self,
        world_id: WorldId,
    ) -> HostResult<WorldCatalogMetadata> {
        let directory = self.worlds_directory.join(world_id.to_string());
        self.open_world_storage(world_id)?;
        WorldCatalogMetadata::load(&directory.join(WORLD_CATALOG_METADATA_FILE))
    }

    pub(crate) fn open_world_storage(&self, world_id: WorldId) -> HostResult<SqliteWorldStorage> {
        let directory = self.worlds_directory.join(world_id.to_string());
        match std::fs::symlink_metadata(&directory) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(HostError::WorldNotFound(world_id));
            }
            Err(error) => return Err(error.into()),
        }
        validate_world_directory(&directory)?;
        open_world_database(&directory, world_id)
    }

    /// Loads durable metadata for one persistent world.
    ///
    /// # Errors
    ///
    /// Returns WorldNotFound when the world directory is absent, or a storage
    /// integrity/version error when the world cannot be opened safely.
    pub fn load_world_state(&self, world_id: WorldId) -> HostResult<WorldSessionState> {
        Ok(self.open_world_storage(world_id)?.load_session()?)
    }

    /// Loads the baseline pre-world composition.
    pub fn load_profile(&self) -> HostResult<BaselineProfile> {
        let profile = BaselineProfile::load(&self.profile_path)?;
        profile.validate_scope(&RuntimeScopeId::new(HOST_SCOPE))?;
        Ok(profile)
    }

    /// Loads the host-owned composition recipe used only for future World creation.
    pub fn load_default_world_recipe(&self) -> HostResult<CompositionProfile> {
        let profile = CompositionProfile::load(&self.default_world_profile_path)?;
        profile.validate_scope(&RuntimeScopeId::new(DEFAULT_WORLD_SCOPE))?;
        Ok(profile)
    }

    fn save_default_world_recipe(&self, profile: &CompositionProfile) -> HostResult<()> {
        profile.validate_scope(&RuntimeScopeId::new(DEFAULT_WORLD_SCOPE))?;
        profile.save(&self.default_world_profile_path)
    }

    fn migrate_legacy_world_defaults(&self) -> HostResult<()> {
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        let default_scope = RuntimeScopeId::new(DEFAULT_WORLD_SCOPE);
        let mut baseline = BaselineProfile::load(&self.profile_path)?;
        baseline.validate_scope(&host_scope)?;
        if !baseline
            .activations
            .iter()
            .any(|activation| activation.world_default)
        {
            return Ok(());
        }

        if self.default_world_profile_path.exists() {
            let recipe = CompositionProfile::load(&self.default_world_profile_path)?;
            recipe.validate_scope(&default_scope)?;
        } else {
            let recipe = baseline.materialize_legacy_world_defaults(default_scope);
            recipe.save(&self.default_world_profile_path)?;
        }

        baseline.clear_legacy_world_defaults();
        baseline.save(&self.profile_path)
    }

    /// Loads the exact composition overlay persisted with one world.
    pub fn load_world_composition(&self, world_id: WorldId) -> HostResult<CompositionProfile> {
        let path = self.world_composition_path(world_id)?;
        let profile = CompositionProfile::load(&path)?;
        profile.validate_scope(&world_runtime_scope_id(world_id))?;
        Ok(profile)
    }

    fn save_world_composition(
        &self,
        world_id: WorldId,
        profile: &CompositionProfile,
    ) -> HostResult<()> {
        profile.validate_scope(&world_runtime_scope_id(world_id))?;
        profile.save(&self.world_composition_path(world_id)?)
    }

    fn world_composition_path(&self, world_id: WorldId) -> HostResult<PathBuf> {
        let storage = self.open_world_storage(world_id)?;
        drop(storage);
        Ok(self
            .worlds_directory
            .join(world_id.to_string())
            .join(WORLD_COMPOSITION_FILE))
    }

    fn persisted_composition_scope(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> HostResult<PersistedCompositionScope> {
        if scope_id.as_str() == HOST_SCOPE {
            return Ok(PersistedCompositionScope::Baseline);
        }
        if scope_id.as_str() == DEFAULT_WORLD_SCOPE {
            return Ok(PersistedCompositionScope::DefaultWorld);
        }
        let Some(raw) = scope_id.as_str().strip_prefix(WORLD_SCOPE_PREFIX) else {
            return Err(HostError::UnsupportedCompositionScope(scope_id.to_string()));
        };
        let world_id = raw
            .parse::<WorldId>()
            .map_err(|_| HostError::UnsupportedCompositionScope(scope_id.to_string()))?;
        if world_runtime_scope_id(world_id) != *scope_id {
            return Err(HostError::UnsupportedCompositionScope(scope_id.to_string()));
        }
        Ok(PersistedCompositionScope::World(world_id))
    }

    fn load_composition_for_scope(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> HostResult<CompositionProfile> {
        match self.persisted_composition_scope(scope_id)? {
            PersistedCompositionScope::Baseline => self.load_profile(),
            PersistedCompositionScope::DefaultWorld => self.load_default_world_recipe(),
            PersistedCompositionScope::World(world_id) => self.load_world_composition(world_id),
        }
    }

    fn save_composition_for_scope(
        &self,
        scope_id: &RuntimeScopeId,
        profile: &CompositionProfile,
    ) -> HostResult<()> {
        profile.validate_scope(scope_id)?;
        match self.persisted_composition_scope(scope_id)? {
            PersistedCompositionScope::Baseline => profile.save(&self.profile_path),
            PersistedCompositionScope::DefaultWorld => self.save_default_world_recipe(profile),
            PersistedCompositionScope::World(world_id) => {
                self.save_world_composition(world_id, profile)
            }
        }
    }

    /// Imports a local RTW supported by this host and selects it in the baseline profile.
    ///
    /// The current built-in content handler is `rintawa.extension@1`. Unsupported
    /// content is rejected before bytes are published into the CAS.
    ///
    /// Existing activations keep their enabled state unless `enabled` explicitly
    /// overrides it. New activations default to enabled.
    pub fn install_local_rtw(
        &self,
        source: impl AsRef<Path>,
        enabled: Option<bool>,
    ) -> HostResult<InstallResult> {
        let source = source.as_ref();
        let archive = RtwArchive::open(source, RtwLimits::default())?;
        let content = archive.manifest().content.clone();
        if content.to_string() != EXTENSION_CONTENT_V1 {
            return Err(HostError::UnsupportedContent(content.to_string()));
        }
        drop(archive);

        let imported = self.store.import(source)?;
        let activation = self.select_stored_rtw(imported.digest(), enabled)?;
        Ok(InstallResult {
            disposition: imported.disposition(),
            activation,
        })
    }

    /// Imports validated RTW bytes into the immutable CAS without selecting them.
    ///
    /// This primitive is source-agnostic: repository URLs, update policy, and version
    /// selection stay outside the host.
    pub fn import_rtw_bytes(&self, bytes: &[u8]) -> HostResult<rintawa_artifacts::ArtifactImport> {
        Ok(self.store.import_bytes(bytes)?)
    }

    /// Selects one already-stored exact RTW artifact in the baseline profile.
    ///
    /// Existing selections preserve their enabled state unless `enabled` overrides it.
    pub fn select_stored_rtw(
        &self,
        digest: &ArtifactDigest,
        enabled: Option<bool>,
    ) -> HostResult<InstalledActivation> {
        self.select_stored_rtw_in_scope(RuntimeScopeId::new(HOST_SCOPE), digest, enabled)
    }

    /// Selects one already-stored exact RTW artifact in a world's composition overlay.
    ///
    /// The same logical extension may be active in baseline and multiple worlds.
    /// Each world owns an opaque host-generated instance ID that remains stable when
    /// the same activation subject is repointed to another exact artifact.
    pub fn select_world_stored_rtw(
        &self,
        world_id: WorldId,
        digest: &ArtifactDigest,
        enabled: Option<bool>,
    ) -> HostResult<InstalledActivation> {
        self.select_stored_rtw_in_scope(world_runtime_scope_id(world_id), digest, enabled)
    }

    fn select_stored_rtw_in_scope(
        &self,
        scope_id: RuntimeScopeId,
        digest: &ArtifactDigest,
        enabled: Option<bool>,
    ) -> HostResult<InstalledActivation> {
        let archive = self.store.open_artifact(digest)?;
        let content = archive.manifest().content.clone();
        if content.to_string() != EXTENSION_CONTENT_V1 {
            return Err(HostError::UnsupportedContent(content.to_string()));
        }
        drop(archive);

        let engine = ExtensionEngine::new();
        let manifest =
            RtwExtensionLoader::new().read_stored_manifest(&engine, &self.store, digest)?;
        let composition_scope = self.persisted_composition_scope(&scope_id)?;
        let mut profile = self.load_composition_for_scope(&scope_id)?;
        let subject = manifest.id.to_string();
        let instance_id = match composition_scope {
            PersistedCompositionScope::Baseline => ExtensionInstanceId::new(subject.clone()),
            PersistedCompositionScope::DefaultWorld | PersistedCompositionScope::World(_) => {
                profile
                    .activations
                    .iter()
                    .find(|activation| {
                        activation.subject == subject && activation.scope_id == scope_id
                    })
                    .map(|activation| activation.instance_id.clone())
                    .unwrap_or_else(|| ExtensionInstanceId::new(Uuid::now_v7().to_string()))
            }
        };
        let enabled = profile.upsert(
            ActivationRecord {
                subject: subject.clone(),
                content: content.clone(),
                artifact: digest.clone(),
                instance_id: instance_id.clone(),
                scope_id: scope_id.clone(),
                enabled: true,
                world_default: false,
            },
            enabled,
        );
        profile.runtime_permissions.retain(|grant| {
            if grant.scope_id != scope_id || grant.instance_id != instance_id {
                return true;
            }
            manifest.components.iter().any(|component| {
                component.id == grant.component_id
                    && component.permissions.runtime.contains(&grant.permission)
            })
        });
        self.save_composition_for_scope(&scope_id, &profile)?;
        let world_default = matches!(composition_scope, PersistedCompositionScope::Baseline)
            && self.is_exact_default_world_activation(&subject, digest)?;

        Ok(InstalledActivation {
            subject,
            content,
            name: manifest.name,
            version: Some(manifest.version),
            digest: digest.clone(),
            instance_id,
            scope_id,
            enabled,
            world_default,
        })
    }

    /// Changes one world-overlay activation's enabled state.
    pub fn set_world_enabled(
        &self,
        world_id: WorldId,
        subject: &str,
        enabled: bool,
    ) -> HostResult<()> {
        self.set_enabled_in_scope(&world_runtime_scope_id(world_id), subject, enabled)
    }

    /// Removes one activation and its scoped policy from a world overlay.
    pub fn remove_world_activation(&self, world_id: WorldId, subject: &str) -> HostResult<()> {
        self.remove_activation_in_scope(&world_runtime_scope_id(world_id), subject)
    }

    /// Changes the baseline enabled state for one handler-defined activation subject.
    pub fn set_enabled(&self, subject: &str, enabled: bool) -> HostResult<()> {
        self.set_enabled_in_scope(&RuntimeScopeId::new(HOST_SCOPE), subject, enabled)
    }

    /// Compatibility helper that adds or removes the current baseline artifact from the
    /// independent default-World recipe.
    ///
    /// Existing Worlds and later baseline policy changes are never propagated implicitly.
    pub fn set_world_default(&self, subject: &str, world_default: bool) -> HostResult<()> {
        let baseline = self.load_profile()?;
        let activation = baseline
            .activations
            .iter()
            .find(|activation| activation.subject == subject)
            .ok_or_else(|| HostError::ActivationNotFound(subject.to_string()))?;
        let default_scope = RuntimeScopeId::new(DEFAULT_WORLD_SCOPE);
        let recipe = self.load_default_world_recipe()?;
        let has_recipe_activation = recipe
            .activations
            .iter()
            .any(|candidate| candidate.subject == subject);

        if !world_default {
            if has_recipe_activation {
                self.remove_activation_in_scope(&default_scope, subject)?;
            }
            return Ok(());
        }

        let selected = self.select_stored_rtw_in_scope(
            default_scope.clone(),
            &activation.artifact,
            Some(true),
        )?;
        if has_recipe_activation {
            return Ok(());
        }

        let mut recipe = self.load_default_world_recipe()?;
        for grant in baseline
            .runtime_permissions
            .iter()
            .filter(|grant| grant.instance_id == activation.instance_id)
        {
            recipe.grant_runtime_permission(RuntimePermissionGrant {
                scope_id: default_scope.clone(),
                instance_id: selected.instance_id.clone(),
                component_id: grant.component_id.clone(),
                permission: grant.permission,
            });
        }
        for preference in baseline
            .preferences
            .iter()
            .filter(|preference| preference.instance_id == activation.instance_id)
        {
            recipe.set_preference(
                default_scope.clone(),
                ComponentRef::new(
                    selected.instance_id.clone(),
                    preference.component_id.clone(),
                ),
                preference.key.clone(),
                preference.value.clone(),
            )?;
        }
        for selection in baseline
            .preferred_providers
            .iter()
            .filter(|selection| selection.provider_instance_id == activation.instance_id)
        {
            recipe.set_preferred_provider(PreferredProviderSelection::new(
                default_scope.clone(),
                selection.contract(),
                ComponentRef::new(
                    selected.instance_id.clone(),
                    selection.provider_component_id.clone(),
                ),
            ));
        }
        self.save_default_world_recipe(&recipe)
    }

    /// Selects one already-stored exact artifact in the future-World recipe.
    pub fn select_default_world_stored_rtw(
        &self,
        digest: &ArtifactDigest,
        enabled: Option<bool>,
    ) -> HostResult<InstalledActivation> {
        self.select_stored_rtw_in_scope(RuntimeScopeId::new(DEFAULT_WORLD_SCOPE), digest, enabled)
    }

    /// Changes enabled state for one future-World recipe activation.
    pub fn set_default_world_enabled(&self, subject: &str, enabled: bool) -> HostResult<()> {
        self.set_enabled_in_scope(&RuntimeScopeId::new(DEFAULT_WORLD_SCOPE), subject, enabled)
    }

    /// Removes one activation and its policy from the future-World recipe.
    pub fn remove_default_world_activation(&self, subject: &str) -> HostResult<()> {
        self.remove_activation_in_scope(&RuntimeScopeId::new(DEFAULT_WORLD_SCOPE), subject)
    }

    fn set_enabled_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
        subject: &str,
        enabled: bool,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(scope_id)?;
        profile.set_enabled(subject, enabled)?;
        self.save_composition_for_scope(scope_id, &profile)
    }

    /// Removes one baseline activation and policy entries that reference its instance.
    ///
    /// Stored CAS bytes remain immutable and may still be referenced by another scope later.
    pub fn remove_activation(&self, subject: &str) -> HostResult<()> {
        self.remove_activation_in_scope(&RuntimeScopeId::new(HOST_SCOPE), subject)
    }

    fn remove_activation_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
        subject: &str,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(scope_id)?;
        profile.remove_activation(subject)?;
        self.save_composition_for_scope(scope_id, &profile)
    }

    /// Reads one owner-scoped non-authoritative extension preference.
    pub fn get_preference(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<Option<String>> {
        self.load_composition_for_scope(scope_id)?
            .get_preference(scope_id, owner, key)
    }

    /// Persists one owner-scoped non-authoritative extension preference.
    pub fn set_preference(
        &self,
        scope_id: RuntimeScopeId,
        owner: ComponentRef,
        key: String,
        value: String,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(&scope_id)?;
        profile.set_preference(scope_id.clone(), owner, key, value)?;
        self.save_composition_for_scope(&scope_id, &profile)
    }

    /// Deletes one owner-scoped non-authoritative extension preference.
    pub fn delete_preference(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(scope_id)?;
        profile.delete_preference(scope_id, owner, key)?;
        self.save_composition_for_scope(scope_id, &profile)
    }

    /// Lists requested and granted runtime permissions for the baseline composition.
    pub fn list_runtime_permission_policy(&self) -> HostResult<Vec<RuntimePermissionPolicyEntry>> {
        self.list_runtime_permission_policy_in_scope(&RuntimeScopeId::new(HOST_SCOPE))
    }

    /// Lists requested and granted runtime permissions for one exact composition scope.
    pub fn list_runtime_permission_policy_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> HostResult<Vec<RuntimePermissionPolicyEntry>> {
        let profile = self.load_composition_for_scope(scope_id)?;
        let engine = ExtensionEngine::new();
        let loader = RtwExtensionLoader::new();
        let mut entries = Vec::new();

        for activation in &profile.activations {
            if activation.content.to_string() != EXTENSION_CONTENT_V1 {
                return Err(HostError::UnsupportedContent(
                    activation.content.to_string(),
                ));
            }
            let manifest =
                loader.read_stored_manifest(&engine, &self.store, &activation.artifact)?;
            for component in manifest.components {
                let owner = ComponentRef::new(activation.instance_id.clone(), component.id.clone());
                let granted = profile
                    .runtime_permissions
                    .iter()
                    .filter(|grant| {
                        grant.scope_id == activation.scope_id
                            && grant.instance_id == owner.instance_id
                            && grant.component_id == owner.component_id
                    })
                    .map(|grant| grant.permission)
                    .collect();
                entries.push(RuntimePermissionPolicyEntry {
                    scope_id: activation.scope_id.clone(),
                    instance_id: activation.instance_id.clone(),
                    component_id: component.id,
                    requested: component.permissions.runtime,
                    granted,
                });
            }
        }

        entries.sort_by(|left, right| {
            left.scope_id
                .as_str()
                .cmp(right.scope_id.as_str())
                .then_with(|| left.instance_id.as_str().cmp(right.instance_id.as_str()))
                .then_with(|| left.component_id.as_str().cmp(right.component_id.as_str()))
        });
        Ok(entries)
    }

    /// Persists one explicit runtime capability approval for an exact composition component.
    ///
    /// The activation must exist in the same scope and its exact stored manifest must
    /// request the permission. This method never expands package-declared capability
    /// requests.
    pub fn grant_runtime_permission(
        &self,
        scope_id: RuntimeScopeId,
        owner: ComponentRef,
        permission: RuntimePermission,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(&scope_id)?;
        let activation = profile
            .activations
            .iter()
            .find(|activation| activation.instance_id == owner.instance_id)
            .ok_or_else(|| {
                HostError::RuntimePermissionActivationNotFound(owner.instance_id.to_string())
            })?;
        if activation.scope_id != scope_id {
            return Err(HostError::RuntimePermissionScopeMismatch {
                instance_id: owner.instance_id.to_string(),
                selected_scope: scope_id.to_string(),
                actual_scope: activation.scope_id.to_string(),
            });
        }
        if activation.content.to_string() != EXTENSION_CONTENT_V1 {
            return Err(HostError::UnsupportedContent(
                activation.content.to_string(),
            ));
        }
        let engine = ExtensionEngine::new();
        let manifest = RtwExtensionLoader::new().read_stored_manifest(
            &engine,
            &self.store,
            &activation.artifact,
        )?;
        let component = manifest
            .components
            .iter()
            .find(|component| component.id == owner.component_id)
            .ok_or_else(|| HostError::RuntimePermissionComponentNotFound {
                instance_id: owner.instance_id.to_string(),
                component_id: owner.component_id.to_string(),
            })?;
        if !component.permissions.runtime.contains(&permission) {
            return Err(HostError::RuntimePermissionNotRequested {
                instance_id: owner.instance_id.to_string(),
                component_id: owner.component_id.to_string(),
                permission: permission.to_string(),
            });
        }
        profile.grant_runtime_permission(RuntimePermissionGrant::new(
            scope_id.clone(),
            owner,
            permission,
        ));
        self.save_composition_for_scope(&scope_id, &profile)
    }

    /// Removes one persisted runtime capability approval.
    ///
    /// Revocation intentionally does not inspect the current artifact so stale or
    /// broken policy can always be repaired.
    pub fn revoke_runtime_permission(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        permission: RuntimePermission,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(scope_id)?;
        profile.revoke_runtime_permission(scope_id, owner, permission);
        self.save_composition_for_scope(scope_id, &profile)
    }

    /// Persists one explicit preferred-provider choice for one host composition.
    ///
    /// The provider instance must already be installed in the same runtime scope.
    /// Provider capability itself is validated during composition activation.
    pub fn set_preferred_provider(
        &self,
        scope_id: RuntimeScopeId,
        contract: ContractKey,
        provider: ComponentRef,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(&scope_id)?;
        let activation = profile
            .activations
            .iter()
            .find(|activation| activation.instance_id == provider.instance_id)
            .ok_or_else(|| {
                HostError::PreferredProviderActivationNotFound(provider.instance_id.to_string())
            })?;
        if activation.scope_id != scope_id {
            return Err(HostError::PreferredProviderScopeMismatch {
                instance_id: provider.instance_id.to_string(),
                selected_scope: scope_id.to_string(),
                actual_scope: activation.scope_id.to_string(),
            });
        }
        profile.set_preferred_provider(PreferredProviderSelection::new(
            scope_id.clone(),
            contract,
            provider,
        ));
        self.save_composition_for_scope(&scope_id, &profile)
    }

    /// Clears an explicit preferred-provider choice from one host composition.
    pub fn clear_preferred_provider(
        &self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) -> HostResult<()> {
        let mut profile = self.load_composition_for_scope(scope_id)?;
        profile.clear_preferred_provider(scope_id, contract);
        self.save_composition_for_scope(scope_id, &profile)
    }

    fn is_exact_default_world_activation(
        &self,
        subject: &str,
        digest: &ArtifactDigest,
    ) -> HostResult<bool> {
        Ok(self
            .load_default_world_recipe()?
            .activations
            .iter()
            .any(|activation| {
                activation.subject == subject
                    && activation.artifact == *digest
                    && activation.enabled
            }))
    }

    /// Lists RTW activations selected by the future-World recipe.
    pub fn list_default_world_activations(&self) -> HostResult<Vec<InstalledActivation>> {
        self.list_activations_in_scope(&RuntimeScopeId::new(DEFAULT_WORLD_SCOPE))
    }

    /// Lists RTW activations selected by one world composition overlay.
    pub fn list_world_activations(
        &self,
        world_id: WorldId,
    ) -> HostResult<Vec<InstalledActivation>> {
        self.list_activations_in_scope(&world_runtime_scope_id(world_id))
    }

    /// Lists RTW activations selected by the baseline profile.
    pub fn list_activations(&self) -> HostResult<Vec<InstalledActivation>> {
        self.list_activations_in_scope(&RuntimeScopeId::new(HOST_SCOPE))
    }

    fn list_activations_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> HostResult<Vec<InstalledActivation>> {
        let engine = ExtensionEngine::new();
        let loader = RtwExtensionLoader::new();
        let is_baseline = matches!(
            self.persisted_composition_scope(scope_id)?,
            PersistedCompositionScope::Baseline
        );
        let default_world = if is_baseline {
            Some(self.load_default_world_recipe()?)
        } else {
            None
        };
        self.load_composition_for_scope(scope_id)?
            .activations
            .into_iter()
            .map(|activation| {
                if activation.content.to_string() != EXTENSION_CONTENT_V1 {
                    return Err(HostError::UnsupportedContent(
                        activation.content.to_string(),
                    ));
                }
                let manifest =
                    loader.read_stored_manifest(&engine, &self.store, &activation.artifact)?;
                let is_world_default = default_world.as_ref().is_some_and(|recipe| {
                    recipe.activations.iter().any(|candidate| {
                        candidate.subject == activation.subject
                            && candidate.artifact == activation.artifact
                            && candidate.enabled
                    })
                });
                Ok(InstalledActivation {
                    subject: manifest.id.to_string(),
                    content: activation.content,
                    name: manifest.name,
                    version: Some(manifest.version),
                    digest: activation.artifact,
                    instance_id: activation.instance_id,
                    scope_id: activation.scope_id,
                    enabled: activation.enabled,
                    world_default: is_world_default,
                })
            })
            .collect()
    }
}

fn ensure_real_directory(path: &Path) -> HostResult<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(HostError::InvalidWorldDirectory {
                    path: path.to_path_buf(),
                    reason: "host worlds path must be a real directory",
                });
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_world_directory(path: &Path) -> HostResult<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(HostError::InvalidWorldDirectory {
            path: path.to_path_buf(),
            reason: "world path must be a real directory",
        });
    }
    Ok(())
}

fn open_world_database(directory: &Path, directory_id: WorldId) -> HostResult<SqliteWorldStorage> {
    validate_world_directory(directory)?;
    let storage = SqliteWorldStorage::open(directory.join(WORLD_DATABASE_FILE))?;
    if storage.world_id() != directory_id {
        return Err(HostError::WorldDirectoryIdMismatch {
            directory_id,
            database_id: storage.world_id(),
        });
    }
    Ok(storage)
}

fn world_summary(state: &WorldSessionState, metadata: WorldCatalogMetadata) -> WorldSummary {
    WorldSummary {
        id: state.id(),
        title: metadata.title().to_string(),
        description: metadata.description().map(str::to_string),
        cover: metadata.cover().cloned(),
        commit_position: state.commit_position(),
    }
}
