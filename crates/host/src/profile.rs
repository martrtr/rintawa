use std::{collections::HashSet, io::Write, path::Path};

use rintawa_artifacts::{ArtifactDigest, ContentType};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey, ContractVersion},
    runtime_permissions::RuntimePermission,
    types::{ComponentId, ContractId, ExtensionInstanceId, RuntimeScopeId},
};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{HostError, HostResult};

/// Current baseline profile schema.
pub const PROFILE_SCHEMA: u32 = 4;
const LEGACY_PROFILE_SCHEMA_V1: u32 = 1;
const LEGACY_PROFILE_SCHEMA_V2: u32 = 2;
const LEGACY_PROFILE_SCHEMA_V3: u32 = 3;

const MAX_PREFERENCE_ENTRIES_PER_COMPONENT: usize = 256;
const MAX_PREFERENCE_KEY_BYTES: usize = 128;
const MAX_PREFERENCE_VALUE_BYTES: usize = 64 * 1024;
const MAX_PREFERENCE_TOTAL_BYTES_PER_COMPONENT: usize = 1024 * 1024;

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

/// One explicit host-approved runtime permission for an exact baseline component.
///
/// The extension manifest must request the permission separately. This record is
/// profile policy only and never expands package-declared capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RuntimePermissionGrant {
    /// Runtime scope containing the component principal.
    pub scope_id: RuntimeScopeId,
    /// Concrete runtime instance receiving the approval.
    pub instance_id: ExtensionInstanceId,
    /// Component within the runtime instance.
    pub component_id: ComponentId,
    /// Exact runtime capability approved by the host profile.
    pub permission: RuntimePermission,
}

impl RuntimePermissionGrant {
    /// Creates one persisted approval for an exact component principal.
    pub fn new(
        scope_id: RuntimeScopeId,
        owner: ComponentRef,
        permission: RuntimePermission,
    ) -> Self {
        Self {
            scope_id,
            instance_id: owner.instance_id,
            component_id: owner.component_id,
            permission,
        }
    }

    /// Returns the exact component principal receiving this approval.
    pub fn owner(&self) -> ComponentRef {
        ComponentRef::new(self.instance_id.clone(), self.component_id.clone())
    }
}

/// One owner-scoped, non-authoritative extension preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PreferenceRecord {
    /// Runtime scope containing the component principal.
    pub scope_id: RuntimeScopeId,
    /// Concrete runtime instance owning the preference.
    pub instance_id: ExtensionInstanceId,
    /// Component within the runtime instance.
    pub component_id: ComponentId,
    /// Extension-defined opaque preference key.
    pub key: String,
    /// Extension-defined UTF-8 value.
    pub value: String,
}

impl PreferenceRecord {
    fn owner_matches(&self, scope_id: &RuntimeScopeId, owner: &ComponentRef) -> bool {
        &self.scope_id == scope_id
            && self.instance_id == owner.instance_id
            && self.component_id == owner.component_id
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
    /// Explicit runtime capabilities approved for exact component principals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_permissions: Vec<RuntimePermissionGrant>,
    /// Owner-scoped non-authoritative extension preferences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferences: Vec<PreferenceRecord>,
}

impl Default for BaselineProfile {
    fn default() -> Self {
        Self {
            schema: PROFILE_SCHEMA,
            activations: Vec::new(),
            preferred_providers: Vec::new(),
            runtime_permissions: Vec::new(),
            preferences: Vec::new(),
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
            LEGACY_PROFILE_SCHEMA_V1 | LEGACY_PROFILE_SCHEMA_V2 | LEGACY_PROFILE_SCHEMA_V3 => {
                profile.schema = PROFILE_SCHEMA;
            }
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
        let mut runtime_grants = HashSet::new();
        for grant in &self.runtime_permissions {
            let key = (
                grant.scope_id.clone(),
                grant.instance_id.clone(),
                grant.component_id.clone(),
                grant.permission,
            );
            if !runtime_grants.insert(key) {
                return Err(HostError::DuplicateRuntimePermissionGrant {
                    scope_id: grant.scope_id.to_string(),
                    instance_id: grant.instance_id.to_string(),
                    component_id: grant.component_id.to_string(),
                    permission: grant.permission.to_string(),
                });
            }
        }

        let mut preference_keys = HashSet::new();
        for preference in &self.preferences {
            Self::validate_preference_value(&preference.key, &preference.value)?;
            let key = (
                preference.scope_id.clone(),
                preference.instance_id.clone(),
                preference.component_id.clone(),
                preference.key.clone(),
            );
            if !preference_keys.insert(key) {
                return Err(HostError::DuplicatePreference {
                    scope_id: preference.scope_id.to_string(),
                    instance_id: preference.instance_id.to_string(),
                    component_id: preference.component_id.to_string(),
                    key: preference.key.clone(),
                });
            }
        }

        let mut owners = HashSet::new();
        for preference in &self.preferences {
            owners.insert((
                preference.scope_id.clone(),
                preference.instance_id.clone(),
                preference.component_id.clone(),
            ));
        }
        for (scope_id, instance_id, component_id) in owners {
            self.validate_preference_owner_quota(&scope_id, &instance_id, &component_id)?;
        }
        Ok(())
    }

    fn validate_preference_value(key: &str, value: &str) -> HostResult<()> {
        if key.is_empty() || key.len() > MAX_PREFERENCE_KEY_BYTES {
            return Err(HostError::InvalidPreference(format!(
                "key must be 1..={MAX_PREFERENCE_KEY_BYTES} UTF-8 bytes"
            )));
        }
        if value.len() > MAX_PREFERENCE_VALUE_BYTES {
            return Err(HostError::InvalidPreference(format!(
                "value exceeds {MAX_PREFERENCE_VALUE_BYTES} UTF-8 bytes"
            )));
        }
        Ok(())
    }

    fn validate_preference_owner_quota(
        &self,
        scope_id: &RuntimeScopeId,
        instance_id: &ExtensionInstanceId,
        component_id: &ComponentId,
    ) -> HostResult<()> {
        let owned: Vec<_> = self
            .preferences
            .iter()
            .filter(|item| {
                &item.scope_id == scope_id
                    && &item.instance_id == instance_id
                    && &item.component_id == component_id
            })
            .collect();
        if owned.len() > MAX_PREFERENCE_ENTRIES_PER_COMPONENT {
            return Err(HostError::PreferenceQuotaExceeded);
        }
        let bytes = owned.iter().fold(0_usize, |total, item| {
            total
                .saturating_add(item.key.len())
                .saturating_add(item.value.len())
        });
        if bytes > MAX_PREFERENCE_TOTAL_BYTES_PER_COMPONENT {
            return Err(HostError::PreferenceQuotaExceeded);
        }
        Ok(())
    }

    pub(crate) fn get_preference(
        &self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<Option<String>> {
        Self::validate_preference_value(key, "")?;
        Ok(self
            .preferences
            .iter()
            .find(|item| item.owner_matches(scope_id, owner) && item.key == key)
            .map(|item| item.value.clone()))
    }

    pub(crate) fn set_preference(
        &mut self,
        scope_id: RuntimeScopeId,
        owner: ComponentRef,
        key: String,
        value: String,
    ) -> HostResult<()> {
        Self::validate_preference_value(&key, &value)?;

        let current = self
            .preferences
            .iter()
            .find(|item| item.owner_matches(&scope_id, &owner) && item.key == key);
        let existing_bytes = current.map_or(0, |item| item.key.len() + item.value.len());
        let owned_count = self
            .preferences
            .iter()
            .filter(|item| item.owner_matches(&scope_id, &owner))
            .count();
        let owned_bytes = self
            .preferences
            .iter()
            .filter(|item| item.owner_matches(&scope_id, &owner))
            .fold(0_usize, |total, item| {
                total
                    .saturating_add(item.key.len())
                    .saturating_add(item.value.len())
            });
        let next_count = if current.is_some() {
            owned_count
        } else {
            owned_count.saturating_add(1)
        };
        let next_bytes = owned_bytes
            .saturating_sub(existing_bytes)
            .saturating_add(key.len())
            .saturating_add(value.len());

        if next_count > MAX_PREFERENCE_ENTRIES_PER_COMPONENT
            || next_bytes > MAX_PREFERENCE_TOTAL_BYTES_PER_COMPONENT
        {
            return Err(HostError::PreferenceQuotaExceeded);
        }

        if let Some(existing) = self
            .preferences
            .iter_mut()
            .find(|item| item.owner_matches(&scope_id, &owner) && item.key == key)
        {
            existing.value = value;
        } else {
            self.preferences.push(PreferenceRecord {
                scope_id,
                instance_id: owner.instance_id,
                component_id: owner.component_id,
                key,
                value,
            });
        }
        Ok(())
    }

    pub(crate) fn delete_preference(
        &mut self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        key: &str,
    ) -> HostResult<()> {
        Self::validate_preference_value(key, "")?;
        self.preferences
            .retain(|item| !(item.owner_matches(scope_id, owner) && item.key == key));
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

    pub(crate) fn grant_runtime_permission(&mut self, grant: RuntimePermissionGrant) {
        if !self.runtime_permissions.contains(&grant) {
            self.runtime_permissions.push(grant);
        }
    }

    pub(crate) fn revoke_runtime_permission(
        &mut self,
        scope_id: &RuntimeScopeId,
        owner: &ComponentRef,
        permission: RuntimePermission,
    ) {
        self.runtime_permissions.retain(|grant| {
            &grant.scope_id != scope_id
                || grant.instance_id != owner.instance_id
                || grant.component_id != owner.component_id
                || grant.permission != permission
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

    pub(crate) fn remove_activation(&mut self, subject: &str) -> HostResult<()> {
        let index = self
            .activations
            .iter()
            .position(|activation| activation.subject == subject)
            .ok_or_else(|| HostError::ActivationNotFound(subject.to_string()))?;
        let removed = self.activations.remove(index);

        self.runtime_permissions.retain(|grant| {
            grant.scope_id != removed.scope_id || grant.instance_id != removed.instance_id
        });
        self.preferred_providers.retain(|selection| {
            selection.scope_id != removed.scope_id
                || selection.provider_instance_id != removed.instance_id
        });
        Ok(())
    }
}
