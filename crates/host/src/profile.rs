use std::{collections::HashSet, io::Write, path::Path};

use rintawa_artifacts::{ArtifactDigest, ContentType};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey, ContractVersion},
    types::{ComponentId, ContractId, ExtensionInstanceId, RuntimeScopeId},
};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{HostError, HostResult};

/// Current baseline profile schema.
pub const PROFILE_SCHEMA: u32 = 2;
const LEGACY_PROFILE_SCHEMA_V1: u32 = 1;

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

/// One explicit provider choice for a versioned contract in one runtime scope.
///
/// The persistent representation is deliberately flat and owned by the host
/// profile schema rather than by the SDK serialization layout of `ComponentRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PreferredProviderSelection {
    /// Runtime scope in which the choice applies.
    pub scope_id: RuntimeScopeId,
    /// Stable identifier of the selected contract.
    pub contract_id: ContractId,
    /// Major version of the selected contract.
    pub contract_version: ContractVersion,
    /// Runtime instance containing the selected provider.
    pub provider_instance_id: ExtensionInstanceId,
    /// Component providing the selected contract.
    pub provider_component_id: ComponentId,
}

impl PreferredProviderSelection {
    /// Creates a persistent selection from runtime contract and component references.
    pub fn new(scope_id: RuntimeScopeId, contract: ContractKey, provider: ComponentRef) -> Self {
        Self {
            scope_id,
            contract_id: contract.id,
            contract_version: contract.version,
            provider_instance_id: provider.instance_id,
            provider_component_id: provider.component_id,
        }
    }

    /// Returns the versioned contract key represented by this selection.
    pub fn contract(&self) -> ContractKey {
        ContractKey::new(self.contract_id.clone(), self.contract_version)
    }

    /// Returns the exact component selected as provider.
    pub fn provider(&self) -> ComponentRef {
        ComponentRef::new(
            self.provider_instance_id.clone(),
            self.provider_component_id.clone(),
        )
    }
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
    /// Explicit provider choices applied before activation planning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_providers: Vec<PreferredProviderSelection>,
}

impl Default for BaselineProfile {
    fn default() -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            activations: Vec::new(),
            preferred_providers: Vec::new(),
        }
    }
}

impl BaselineProfile {
    pub(crate) fn load(path: &Path) -> HostResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = std::fs::read_to_string(path)?;
        let mut profile: Self = toml::from_str(&source)?;
        match profile.schema {
            PROFILE_SCHEMA => {}
            LEGACY_PROFILE_SCHEMA_V1 => profile.schema = PROFILE_SCHEMA,
            unsupported => return Err(HostError::UnsupportedProfileSchema(unsupported)),
        }
        profile.validate()?;
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

    fn validate(&self) -> HostResult<()> {
        let mut selections = HashSet::new();
        for selection in &self.preferred_providers {
            let contract = selection.contract();
            let key = (selection.scope_id.clone(), contract.clone());
            if !selections.insert(key) {
                return Err(HostError::DuplicatePreferredProviderSelection {
                    scope_id: selection.scope_id.to_string(),
                    contract: contract.to_string(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn set_preferred_provider(&mut self, selection: PreferredProviderSelection) {
        let contract = selection.contract();
        if let Some(existing) = self.preferred_providers.iter_mut().find(|existing| {
            existing.scope_id == selection.scope_id && existing.contract() == contract
        }) {
            *existing = selection;
        } else {
            self.preferred_providers.push(selection);
        }
    }

    pub(crate) fn clear_preferred_provider(
        &mut self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) {
        self.preferred_providers.retain(|selection| {
            &selection.scope_id != scope_id || &selection.contract() != contract
        });
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
