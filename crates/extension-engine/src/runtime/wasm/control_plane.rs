//! WASM adapters for host-owned artifact, preference, runtime-policy, and composition control.

use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, types::RuntimeScopeId,
};

use crate::{
    host_access::{CompositionActivation, HostAccessError, RuntimePolicyComponent},
    runtime::wasm::{
        ArtifactStoreError, ArtifactStoreHost, CompositionError, CompositionHost, PreferenceError,
        PreferencesHost, RuntimePermissionCheck, RuntimePolicyError, RuntimePolicyHost,
        WasmHostState, WitCompositionActivation, WitImportedArtifact, WitRuntimeArtifactPolicy,
        WitRuntimePolicyComponent, WitRuntimePolicyRequest,
    },
};

impl ArtifactStoreHost for WasmHostState {
    fn import_rtw(&mut self, bytes: Vec<u8>) -> Result<WitImportedArtifact, ArtifactStoreError> {
        if !self.host_access_active {
            return Err(ArtifactStoreError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::ArtifactImport)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => ArtifactStoreError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => ArtifactStoreError::Unavailable,
            })?;
        if bytes.len() > self.max_artifact_import_bytes {
            return Err(ArtifactStoreError::MessageTooLarge);
        }

        let imported = self
            .host_access
            .artifact_store
            .import_rtw(&bytes)
            .map_err(map_artifact_store_access_error)?;
        Ok(WitImportedArtifact {
            digest: imported.digest,
            content: imported.content,
        })
    }
}

impl PreferencesHost for WasmHostState {
    fn get(&mut self, key: String) -> Result<Option<String>, PreferenceError> {
        let (scope_id, owner) = self.preference_owner()?;
        if key.len() > self.max_host_message_bytes {
            return Err(PreferenceError::MessageTooLarge);
        }
        self.host_access
            .preferences
            .get(
                scope_id.as_str(),
                owner.instance_id.as_str(),
                owner.component_id.as_str(),
                &key,
            )
            .map_err(map_preference_access_error)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), PreferenceError> {
        let (scope_id, owner) = self.preference_owner()?;
        if key.len().saturating_add(value.len()) > self.max_host_message_bytes {
            return Err(PreferenceError::MessageTooLarge);
        }
        self.host_access
            .preferences
            .set(
                scope_id.as_str(),
                owner.instance_id.as_str(),
                owner.component_id.as_str(),
                &key,
                &value,
            )
            .map_err(map_preference_access_error)
    }

    fn delete(&mut self, key: String) -> Result<(), PreferenceError> {
        let (scope_id, owner) = self.preference_owner()?;
        if key.len() > self.max_host_message_bytes {
            return Err(PreferenceError::MessageTooLarge);
        }
        self.host_access
            .preferences
            .delete(
                scope_id.as_str(),
                owner.instance_id.as_str(),
                owner.component_id.as_str(),
                &key,
            )
            .map_err(map_preference_access_error)
    }
}

impl WasmHostState {
    fn preference_owner(&self) -> Result<(RuntimeScopeId, ComponentRef), PreferenceError> {
        if !self.host_access_active {
            return Err(PreferenceError::AccessNotActive);
        }
        let scope_id = self
            .scope_id
            .clone()
            .ok_or(PreferenceError::AccessNotActive)?;
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(PreferenceError::AccessNotActive)?;
        Ok((scope_id, owner))
    }
}

impl RuntimePolicyHost for WasmHostState {
    fn inspect_artifact(
        &mut self,
        digest: String,
    ) -> Result<WitRuntimeArtifactPolicy, RuntimePolicyError> {
        if !self.host_access_active {
            return Err(RuntimePolicyError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::RuntimePolicyRead)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => RuntimePolicyError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => RuntimePolicyError::Unavailable,
            })?;
        if digest.len() > self.max_host_message_bytes {
            return Err(RuntimePolicyError::MessageTooLarge);
        }

        let policy = self
            .host_access
            .runtime_policy
            .inspect_artifact(&digest)
            .map_err(map_runtime_policy_access_error)?;
        let message_bytes = policy
            .subject
            .len()
            .saturating_add(policy.name.len())
            .saturating_add(policy.version.len())
            .saturating_add(policy.components.iter().fold(0_usize, |total, component| {
                total
                    .saturating_add(component.component_id.len())
                    .saturating_add(component.requested.iter().map(String::len).sum::<usize>())
            }));
        if message_bytes > self.max_host_message_bytes {
            return Err(RuntimePolicyError::MessageTooLarge);
        }

        Ok(WitRuntimeArtifactPolicy {
            subject: policy.subject,
            name: policy.name,
            version: policy.version,
            components: policy
                .components
                .into_iter()
                .map(|component| WitRuntimePolicyRequest {
                    component_id: component.component_id,
                    requested: component.requested,
                })
                .collect(),
        })
    }

    fn list_components(&mut self) -> Result<Vec<WitRuntimePolicyComponent>, RuntimePolicyError> {
        self.require_runtime_policy_read()?;
        let components = self
            .host_access
            .runtime_policy
            .list_components()
            .map_err(map_runtime_policy_access_error)?;
        self.bound_runtime_policy_components(components)
    }

    fn list_components_in_scope(
        &mut self,
        scope_id: String,
    ) -> Result<Vec<WitRuntimePolicyComponent>, RuntimePolicyError> {
        self.require_runtime_policy_read()?;
        if scope_id.len() > self.max_host_message_bytes {
            return Err(RuntimePolicyError::MessageTooLarge);
        }
        let components = self
            .host_access
            .runtime_policy
            .list_components_in_scope(&scope_id)
            .map_err(map_runtime_policy_access_error)?;
        self.bound_runtime_policy_components(components)
    }

    fn grant(
        &mut self,
        scope_id: String,
        instance_id: String,
        component_id: String,
        permission: String,
    ) -> Result<(), RuntimePolicyError> {
        self.require_runtime_policy_write()?;
        if scope_id
            .len()
            .saturating_add(instance_id.len())
            .saturating_add(component_id.len())
            .saturating_add(permission.len())
            > self.max_host_message_bytes
        {
            return Err(RuntimePolicyError::MessageTooLarge);
        }
        self.host_access
            .runtime_policy
            .grant(&scope_id, &instance_id, &component_id, &permission)
            .map_err(map_runtime_policy_access_error)
    }

    fn revoke(
        &mut self,
        scope_id: String,
        instance_id: String,
        component_id: String,
        permission: String,
    ) -> Result<(), RuntimePolicyError> {
        self.require_runtime_policy_write()?;
        if scope_id
            .len()
            .saturating_add(instance_id.len())
            .saturating_add(component_id.len())
            .saturating_add(permission.len())
            > self.max_host_message_bytes
        {
            return Err(RuntimePolicyError::MessageTooLarge);
        }
        self.host_access
            .runtime_policy
            .revoke(&scope_id, &instance_id, &component_id, &permission)
            .map_err(map_runtime_policy_access_error)
    }
}

impl WasmHostState {
    fn require_runtime_policy_read(&self) -> Result<(), RuntimePolicyError> {
        if !self.host_access_active {
            return Err(RuntimePolicyError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::RuntimePolicyRead)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => RuntimePolicyError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => RuntimePolicyError::Unavailable,
            })
    }

    fn require_runtime_policy_write(&self) -> Result<(), RuntimePolicyError> {
        if !self.host_access_active {
            return Err(RuntimePolicyError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::RuntimePolicyWrite)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => RuntimePolicyError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => RuntimePolicyError::Unavailable,
            })
    }

    fn bound_runtime_policy_components(
        &self,
        components: Vec<RuntimePolicyComponent>,
    ) -> Result<Vec<WitRuntimePolicyComponent>, RuntimePolicyError> {
        let message_bytes = components.iter().fold(0_usize, |total, component| {
            total
                .saturating_add(component.scope_id.len())
                .saturating_add(component.instance_id.len())
                .saturating_add(component.component_id.len())
                .saturating_add(component.requested.iter().map(String::len).sum::<usize>())
                .saturating_add(component.granted.iter().map(String::len).sum::<usize>())
        });
        if message_bytes > self.max_host_message_bytes {
            return Err(RuntimePolicyError::MessageTooLarge);
        }

        Ok(components
            .into_iter()
            .map(|component| WitRuntimePolicyComponent {
                scope_id: component.scope_id,
                instance_id: component.instance_id,
                component_id: component.component_id,
                requested: component.requested,
                granted: component.granted,
            })
            .collect())
    }
}

impl CompositionHost for WasmHostState {
    fn list_activations(&mut self) -> Result<Vec<WitCompositionActivation>, CompositionError> {
        self.require_composition_read()?;
        let activations = self
            .host_access
            .composition
            .list_activations()
            .map_err(map_composition_access_error)?;
        self.bound_composition_activations(activations)
    }

    fn select_artifact(
        &mut self,
        digest: String,
        enabled: Option<bool>,
    ) -> Result<WitCompositionActivation, CompositionError> {
        self.require_composition_write()?;
        if digest.len() > self.max_host_message_bytes {
            return Err(CompositionError::InvalidDigest);
        }

        self.host_access
            .composition
            .select_artifact(&digest, enabled)
            .map(to_wit_composition_activation)
            .map_err(map_composition_access_error)
    }

    fn set_enabled(&mut self, subject: String, enabled: bool) -> Result<(), CompositionError> {
        self.require_composition_write()?;
        if subject.len() > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .set_enabled(&subject, enabled)
            .map_err(map_composition_access_error)
    }

    fn remove_activation(&mut self, subject: String) -> Result<(), CompositionError> {
        self.require_composition_write()?;
        if subject.len() > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .remove_activation(&subject)
            .map_err(map_composition_access_error)
    }

    fn list_activations_in_scope(
        &mut self,
        scope_id: String,
    ) -> Result<Vec<WitCompositionActivation>, CompositionError> {
        self.require_composition_read()?;
        if scope_id.len() > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        let activations = self
            .host_access
            .composition
            .list_activations_in_scope(&scope_id)
            .map_err(map_composition_access_error)?;
        self.bound_composition_activations(activations)
    }

    fn select_artifact_in_scope(
        &mut self,
        scope_id: String,
        digest: String,
        enabled: Option<bool>,
    ) -> Result<WitCompositionActivation, CompositionError> {
        self.require_composition_write()?;
        if scope_id.len().saturating_add(digest.len()) > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .select_artifact_in_scope(&scope_id, &digest, enabled)
            .map(to_wit_composition_activation)
            .map_err(map_composition_access_error)
    }

    fn set_enabled_in_scope(
        &mut self,
        scope_id: String,
        subject: String,
        enabled: bool,
    ) -> Result<(), CompositionError> {
        self.require_composition_write()?;
        if scope_id.len().saturating_add(subject.len()) > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .set_enabled_in_scope(&scope_id, &subject, enabled)
            .map_err(map_composition_access_error)
    }

    fn remove_activation_in_scope(
        &mut self,
        scope_id: String,
        subject: String,
    ) -> Result<(), CompositionError> {
        self.require_composition_write()?;
        if scope_id.len().saturating_add(subject.len()) > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .remove_activation_in_scope(&scope_id, &subject)
            .map_err(map_composition_access_error)
    }
}

impl WasmHostState {
    fn require_composition_read(&self) -> Result<(), CompositionError> {
        if !self.host_access_active {
            return Err(CompositionError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::CompositionRead)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => CompositionError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => CompositionError::Unavailable,
            })
    }

    fn require_composition_write(&self) -> Result<(), CompositionError> {
        if !self.host_access_active {
            return Err(CompositionError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::CompositionWrite)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => CompositionError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => CompositionError::Unavailable,
            })
    }

    fn bound_composition_activations(
        &self,
        activations: Vec<CompositionActivation>,
    ) -> Result<Vec<WitCompositionActivation>, CompositionError> {
        let message_bytes = activations.iter().fold(0_usize, |total, activation| {
            total
                .saturating_add(activation.subject.len())
                .saturating_add(activation.content.len())
                .saturating_add(activation.name.len())
                .saturating_add(activation.version.as_ref().map_or(0, String::len))
                .saturating_add(activation.digest.len())
                .saturating_add(activation.instance_id.len())
                .saturating_add(activation.scope_id.len())
                .saturating_add(1)
        });
        if message_bytes > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }

        Ok(activations
            .into_iter()
            .map(to_wit_composition_activation)
            .collect())
    }
}

fn to_wit_composition_activation(activation: CompositionActivation) -> WitCompositionActivation {
    WitCompositionActivation {
        subject: activation.subject,
        content: activation.content,
        name: activation.name,
        version: activation.version,
        digest: activation.digest,
        instance_id: activation.instance_id,
        scope_id: activation.scope_id,
        enabled: activation.enabled,
    }
}

fn map_artifact_store_access_error(error: HostAccessError) -> ArtifactStoreError {
    match error {
        HostAccessError::InvalidArtifact | HostAccessError::InvalidDigest => {
            ArtifactStoreError::InvalidArtifact
        }
        HostAccessError::Unavailable => ArtifactStoreError::Unavailable,
        HostAccessError::NotFound
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => ArtifactStoreError::Rejected,
    }
}

fn map_preference_access_error(error: HostAccessError) -> PreferenceError {
    match error {
        HostAccessError::InvalidPreference => PreferenceError::InvalidPreference,
        HostAccessError::PreferenceQuotaExceeded => PreferenceError::QuotaExceeded,
        HostAccessError::Unavailable => PreferenceError::Unavailable,
        HostAccessError::InvalidArtifact
        | HostAccessError::InvalidDigest
        | HostAccessError::NotFound
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::Rejected => PreferenceError::Rejected,
    }
}

fn map_runtime_policy_access_error(error: HostAccessError) -> RuntimePolicyError {
    match error {
        HostAccessError::InvalidPermission => RuntimePolicyError::InvalidPermission,
        HostAccessError::NotFound => RuntimePolicyError::NotFound,
        HostAccessError::Unavailable => RuntimePolicyError::Unavailable,
        HostAccessError::InvalidArtifact
        | HostAccessError::InvalidDigest
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => RuntimePolicyError::Rejected,
    }
}

fn map_composition_access_error(error: HostAccessError) -> CompositionError {
    match error {
        HostAccessError::InvalidDigest | HostAccessError::InvalidArtifact => {
            CompositionError::InvalidDigest
        }
        HostAccessError::NotFound => CompositionError::NotFound,
        HostAccessError::UnsupportedContent => CompositionError::UnsupportedContent,
        HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => CompositionError::Rejected,
        HostAccessError::Unavailable => CompositionError::Unavailable,
    }
}
