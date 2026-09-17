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
    ActivationPlanError, DeferredExecutionTarget, ExtensionEngine, RtwExtensionLoader,
    UnresolvedContractReason,
};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey},
    types::{ExtensionInstanceId, RuntimeScopeId},
};
use thiserror::Error;

pub use profile::{ActivationRecord, BaselineProfile, PROFILE_SCHEMA, PreferredProviderSelection};
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
        let digest = imported.digest().clone();
        let engine = ExtensionEngine::new();
        let manifest =
            RtwExtensionLoader::new().read_stored_manifest(&engine, &self.store, &digest)?;

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

        Ok(InstallResult {
            disposition: imported.disposition(),
            activation: InstalledActivation {
                subject,
                content,
                name: manifest.name,
                version: Some(manifest.version),
                digest,
                instance_id,
                scope_id,
                enabled,
            },
        })
    }

    /// Changes the baseline enabled state for one handler-defined activation subject.
    pub fn set_enabled(&self, subject: &str, enabled: bool) -> HostResult<()> {
        let mut profile = self.load_profile()?;
        profile.set_enabled(subject, enabled)?;
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
