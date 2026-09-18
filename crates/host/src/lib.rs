//! Local host composition for exact RTW artifacts.
//!
//! This crate intentionally contains no repository, HTTP, update, or version-selection
//! policy. It persists exact artifact activations that a CLI, installer, or package
//! manager can request after obtaining RTW bytes by any means.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod profile;
mod runtime;

use std::{
    fmt,
    path::{Path, PathBuf},
};

use rintawa_artifacts::{
    ArtifactDigest, ArtifactStore, ContentType, ImportDisposition, RtwArchive, RtwLimits,
};
use rintawa_extension_engine::{
    ActivationPlanError, DeferredExecutionTarget, EngineError, ExtensionEngine, RtwExtensionLoader,
    UnresolvedContractReason,
};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey},
    runtime_permissions::RuntimePermission,
    types::{ExtensionInstanceId, RuntimeScopeId},
};
use thiserror::Error;

pub use profile::{
    ActivationRecord, BaselineProfile, PROFILE_SCHEMA, PreferredProviderSelection,
    RuntimePermissionGrant,
};
pub use runtime::HostRuntime;

/// Stable runtime scope used by the pre-world/bootstrap composition.
pub const HOST_SCOPE: &str = "host";
/// File name of the current baseline host profile.
pub const BASELINE_PROFILE_FILE: &str = "baseline.toml";
const EXTENSION_CONTENT_V1: &str = "rintawa.extension@1";

/// One baseline activation deferred because required execution targets are unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDeferredActivation {
    /// Handler-defined activation identity from the baseline profile.
    pub subject: String,
    /// Concrete runtime instance that could not be registered yet.
    pub instance_id: ExtensionInstanceId,
    /// Logical extension identity read from the validated artifact manifest.
    pub extension_id: String,
    /// Required execution targets currently missing from the Engine registry.
    pub missing_required_targets: Vec<DeferredExecutionTarget>,
}

/// One registered baseline activation blocked by required contract composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapBlockedActivation {
    /// Concrete runtime instance that could not enter `Active`.
    pub instance_id: ExtensionInstanceId,
    /// Exact activation-planner reason observed for this instance.
    pub reason: ActivationPlanError,
}

/// Structured diagnostics produced when baseline bootstrap reaches a fixed point.
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
        formatter.write_str("baseline bootstrap reached a fixed point")?;
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

/// Aggregated failures observed while shutting down a running baseline composition.
#[derive(Debug)]
pub struct HostShutdownFailures {
    /// Stop/unregister failures observed while shutdown continued.
    pub cleanup_failures: Vec<HostCleanupFailure>,
}

impl fmt::Display for HostShutdownFailures {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("baseline shutdown cleanup failed")?;
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
        self.cleanup_failures
            .first()
            .map(|failure| &failure.error as &(dyn std::error::Error + 'static))
    }
}

/// Errors returned by local host composition operations.
#[derive(Debug, Error)]
pub enum HostError {
    /// An RTW artifact operation failed.
    #[error(transparent)]
    Artifact(#[from] rintawa_artifacts::RtwError),
    /// Extension inspection or lifecycle failed.
    #[error(transparent)]
    Engine(#[from] rintawa_extension_engine::EngineError),
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Persisted host state could not be decoded.
    #[error("invalid host profile: {0}")]
    ProfileDecode(#[from] toml::de::Error),
    /// Host state could not be encoded.
    #[error("failed to encode host profile: {0}")]
    ProfileEncode(#[from] toml::ser::Error),
    /// The persisted profile schema is newer than this host understands.
    #[error("unsupported host profile schema {0}")]
    UnsupportedProfileSchema(u32),
    /// No activation with the requested subject exists in the baseline profile.
    #[error("activation `{0}` is not installed in the baseline profile")]
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
    /// Whether the baseline activation should start automatically.
    pub enabled: bool,
}

/// Result of importing and selecting a local RTW activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    /// Installed activation summary.
    pub activation: InstalledActivation,
    /// Whether new bytes entered the content-addressed store.
    pub disposition: ImportDisposition,
}

/// Persistent local Rintawa home containing CAS bytes and baseline composition.
pub struct HostHome {
    root: PathBuf,
    store: ArtifactStore,
    profile_path: PathBuf,
}

impl HostHome {
    /// Opens or creates one local Rintawa home.
    pub fn open(root: impl AsRef<Path>) -> HostResult<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let store = ArtifactStore::open(root.join("artifacts"), RtwLimits::default())?;
        let profile_path = root.join("profiles").join(BASELINE_PROFILE_FILE);
        Ok(Self {
            root,
            store,
            profile_path,
        })
    }

    /// Returns the canonical local Rintawa home path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the immutable artifact store.
    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.store
    }

    /// Loads the baseline pre-world composition.
    pub fn load_profile(&self) -> HostResult<BaselineProfile> {
        BaselineProfile::load(&self.profile_path)
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
        let archive = self.store.open_artifact(digest)?;
        let content = archive.manifest().content.clone();
        if content.to_string() != EXTENSION_CONTENT_V1 {
            return Err(HostError::UnsupportedContent(content.to_string()));
        }
        drop(archive);

        let engine = ExtensionEngine::new();
        let manifest =
            RtwExtensionLoader::new().read_stored_manifest(&engine, &self.store, digest)?;
        let mut profile = self.load_profile()?;
        let subject = manifest.id.to_string();
        let scope_id = RuntimeScopeId::new(HOST_SCOPE);
        let instance_id = ExtensionInstanceId::new(subject.clone());
        let enabled = profile.upsert(
            ActivationRecord {
                subject: subject.clone(),
                content: content.clone(),
                artifact: digest.clone(),
                instance_id: instance_id.clone(),
                scope_id: scope_id.clone(),
                enabled: true,
            },
            enabled,
        );
        profile.save(&self.profile_path)?;

        Ok(InstalledActivation {
            subject,
            content,
            name: manifest.name,
            version: Some(manifest.version),
            digest: digest.clone(),
            instance_id,
            scope_id,
            enabled,
        })
    }

    /// Changes the baseline enabled state for one handler-defined activation subject.
    pub fn set_enabled(&self, subject: &str, enabled: bool) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.set_enabled(subject, enabled)?;
        profile.save(&self.profile_path)
    }

    /// Removes one baseline activation and policy entries that reference its instance.
    ///
    /// Stored CAS bytes remain immutable and may still be referenced by another scope later.
    pub fn remove_activation(&self, subject: &str) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.remove_activation(subject)?;
        profile.save(&self.profile_path)
    }

    /// Reads one owner-scoped non-authoritative extension preference.
    pub fn get_preference(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<Option<String>> {
        self.load_profile()?.get_preference(scope_id, owner, key)
    }

    /// Persists one owner-scoped non-authoritative extension preference.
    pub fn set_preference(
        &self,
        scope_id: RuntimeScopeId,
        owner: ComponentRef,
        key: String,
        value: String,
    ) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.set_preference(scope_id, owner, key, value)?;
        profile.save(&self.profile_path)
    }

    /// Deletes one owner-scoped non-authoritative extension preference.
    pub fn delete_preference(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.delete_preference(scope_id, owner, key)?;
        profile.save(&self.profile_path)
    }

    /// Persists one explicit runtime capability approval for an exact baseline component.
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
        let mut profile = self.load_profile()?;
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
        profile.grant_runtime_permission(RuntimePermissionGrant::new(scope_id, owner, permission));
        profile.save(&self.profile_path)
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
        let mut profile = self.load_profile()?;
        profile.revoke_runtime_permission(scope_id, owner, permission);
        profile.save(&self.profile_path)
    }

    /// Persists one explicit preferred-provider choice for the baseline composition.
    ///
    /// The provider instance must already be installed in the same runtime scope.
    /// Provider capability itself is validated during bootstrap after registration.
    pub fn set_preferred_provider(
        &self,
        scope_id: RuntimeScopeId,
        contract: ContractKey,
        provider: ComponentRef,
    ) -> HostResult<()> {
        let mut profile = self.load_profile()?;
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
            scope_id, contract, provider,
        ));
        profile.save(&self.profile_path)
    }

    /// Clears an explicit preferred-provider choice from the baseline composition.
    pub fn clear_preferred_provider(
        &self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.clear_preferred_provider(scope_id, contract);
        profile.save(&self.profile_path)
    }

    /// Lists RTW activations selected by the baseline profile.
    pub fn list_activations(&self) -> HostResult<Vec<InstalledActivation>> {
        let engine = ExtensionEngine::new();
        let loader = RtwExtensionLoader::new();
        self.load_profile()?
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
                Ok(InstalledActivation {
                    subject: manifest.id.to_string(),
                    content: activation.content,
                    name: manifest.name,
                    version: Some(manifest.version),
                    digest: activation.artifact,
                    instance_id: activation.instance_id,
                    scope_id: activation.scope_id,
                    enabled: activation.enabled,
                })
            })
            .collect()
    }
}
