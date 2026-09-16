use std::{io::Write, path::Path};

use rintawa_artifacts::{ArtifactDigest, ContentType};
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{HostError, HostResult};

/// Current baseline profile schema.
pub const PROFILE_SCHEMA: u32 = 1;

/// One exact activation selected for the pre-world host composition.
///
/// `subject` is defined by the RTW content handler. For `rintawa.extension@1`
/// it is the logical extension ID. Future world state can overlay these records
/// without changing CAS identity or the RTW container format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ActivationRecord {
    /// Handler-defined stable identity for this activation.
    pub subject: String,
    /// RTW content type that owns interpretation and activation semantics.
    pub content: ContentType,
    /// Exact immutable RTW bytes selected for this activation.
    pub artifact: ArtifactDigest,
    /// Concrete runtime instance identifier.
    pub instance_id: ExtensionInstanceId,
    /// Runtime composition scope.
    pub scope_id: RuntimeScopeId,
    /// Whether the activation starts automatically in this profile.
    pub enabled: bool,
}

/// Persistent baseline composition used before any State Engine world is opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineProfile {
    /// Version of this persistence schema.
    pub schema: u32,
    /// Ordered exact activations. Order remains explicit for future composition policy.
    #[serde(default)]
    pub activations: Vec<ActivationRecord>,
}

impl Default for BaselineProfile {
    fn default() -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            activations: Vec::new(),
        }
    }
}

impl BaselineProfile {
    pub(crate) fn load(path: &Path) -> HostResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = std::fs::read_to_string(path)?;
        let profile: Self = toml::from_str(&source)?;
        if profile.schema != PROFILE_SCHEMA {
            return Err(HostError::UnsupportedProfileSchema(profile.schema));
        }
        Ok(profile)
    }

    pub(crate) fn save(&self, path: &Path) -> HostResult<()> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "profile path has no parent",
            )
        })?;
        std::fs::create_dir_all(parent)?;
        let source = toml::to_string_pretty(self)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(source.as_bytes())?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| HostError::Io(error.error))?;
        Ok(())
    }

    pub(crate) fn upsert(
        &mut self,
        activation: ActivationRecord,
        override_enabled: Option<bool>,
    ) -> bool {
        if let Some(existing) = self.activations.iter_mut().find(|existing| {
            existing.subject == activation.subject && existing.scope_id == activation.scope_id
        }) {
            let enabled = override_enabled.unwrap_or(existing.enabled);
            *existing = ActivationRecord {
                enabled,
                ..activation
            };
            return enabled;
        }
        let enabled = override_enabled.unwrap_or(activation.enabled);
        self.activations.push(ActivationRecord {
            enabled,
            ..activation
        });
        enabled
    }

    pub(crate) fn set_enabled(&mut self, subject: &str, enabled: bool) -> HostResult<()> {
        let activation = self
            .activations
            .iter_mut()
            .find(|activation| activation.subject == subject)
            .ok_or_else(|| HostError::ActivationNotFound(subject.to_string()))?;
        activation.enabled = enabled;
        Ok(())
    }
}
