//! WASM adapters for host-owned artifact, user-content, preference, policy, and composition access.

use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, types::RuntimeScopeId,
};

use crate::{
    host_access::{
        CompositionActivation, HostAccessError, RuntimePolicyComponent, UserContentDocument,
        UserContentSummary, UserContentWriteStatus, WorldCommandAccessError,
        WorldCommandActor as HostWorldCommandActor, WorldCommandRequest,
        WorldProjectionAccessError, WorldProjectionReadStatus, WorldProjectionRequest,
        WorldSessionSummary,
    },
    runtime::wasm::{
        ArtifactStoreError, ArtifactStoreHost, AssetStoreError, AssetStoreHost, CompositionError,
        CompositionHost, PreferenceError, PreferencesHost, RuntimePermissionCheck,
        RuntimePolicyError, RuntimePolicyHost, ScopedCompositionHost, ScopedRuntimePolicyHost,
        UserContentError, UserContentHost, WasmHostState, WitAcceptedUserContentWrite,
        WitAcceptedWorldCommand, WitAcceptedWorldProjectionRead, WitAssetRef,
        WitCompositionActivation, WitImportedArtifact, WitRuntimeArtifactPolicy,
        WitRuntimePolicyComponent, WitRuntimePolicyRequest, WitUserContentDocument,
        WitUserContentEntry, WitUserContentWriteState, WitWorldCommandActor,
        WitWorldCommandRequest, WitWorldProjectionReadState, WitWorldProjectionRequest,
        WitWorldProjectionView, WitWorldSummary, WorldCommandError, WorldCommandsHost,
        WorldProjectionError, WorldProjectionsHost, WorldSessionError, WorldSessionsHost,
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

impl AssetStoreHost for WasmHostState {
    fn import_asset(
        &mut self,
        bytes: Vec<u8>,
        media_type: String,
    ) -> Result<WitAssetRef, AssetStoreError> {
        if !self.host_access_active {
            return Err(AssetStoreError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::AssetImport)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => AssetStoreError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => AssetStoreError::Unavailable,
            })?;
        if bytes.len() > self.max_asset_import_bytes
            || media_type.len() > self.max_host_message_bytes
        {
            return Err(AssetStoreError::MessageTooLarge);
        }

        let imported = self
            .host_access
            .asset_store
            .import_asset(&bytes, &media_type)
            .map_err(map_asset_store_access_error)?;
        Ok(WitAssetRef {
            digest: imported.digest,
            size: imported.size,
            media_type: imported.media_type,
        })
    }
}

impl UserContentHost for WasmHostState {
    fn list_items(
        &mut self,
        content: Option<String>,
    ) -> Result<Vec<WitUserContentEntry>, UserContentError> {
        self.require_user_content_read()?;
        if content
            .as_ref()
            .is_some_and(|content| content.len() > self.max_host_message_bytes)
        {
            return Err(UserContentError::MessageTooLarge);
        }
        let entries = self
            .host_access
            .user_content
            .list_user_content(content.as_deref())
            .map_err(map_user_content_access_error)?;
        self.bound_user_content_entries(entries)
    }

    fn read(&mut self, id: String) -> Result<WitUserContentDocument, UserContentError> {
        self.require_user_content_read()?;
        if id.len() > self.max_host_message_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        let document = self
            .host_access
            .user_content
            .read_user_content(&id)
            .map_err(map_user_content_access_error)?;
        self.bound_user_content_document(document)
    }

    fn request_import(
        &mut self,
        rtw: Vec<u8>,
    ) -> Result<WitAcceptedUserContentWrite, UserContentError> {
        let owner = self.require_user_content_write()?;
        if rtw.len() > self.max_artifact_import_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        self.consume_user_content_write_budget()?;
        let accepted = self
            .host_access
            .user_content_write
            .request_import(&owner, &rtw)
            .map_err(map_user_content_access_error)?;
        Ok(WitAcceptedUserContentWrite {
            operation_id: accepted.operation_id,
        })
    }

    fn request_replace(
        &mut self,
        id: String,
        rtw: Vec<u8>,
    ) -> Result<WitAcceptedUserContentWrite, UserContentError> {
        let owner = self.require_user_content_write()?;
        if id.len() > self.max_host_message_bytes || rtw.len() > self.max_artifact_import_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        self.consume_user_content_write_budget()?;
        let accepted = self
            .host_access
            .user_content_write
            .request_replace(&owner, &id, &rtw)
            .map_err(map_user_content_access_error)?;
        Ok(WitAcceptedUserContentWrite {
            operation_id: accepted.operation_id,
        })
    }

    fn write_status(
        &mut self,
        operation_id: String,
    ) -> Result<WitUserContentWriteState, UserContentError> {
        let owner = self.require_user_content_write()?;
        if operation_id.len() > self.max_host_message_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        let status = self
            .host_access
            .user_content_write
            .write_status(&owner, &operation_id)
            .map_err(map_user_content_access_error)?;
        self.bound_user_content_write_status(status)
    }
}

impl WorldSessionsHost for WasmHostState {
    fn list_worlds(&mut self) -> Result<Vec<WitWorldSummary>, WorldSessionError> {
        self.require_world_session_permission(RuntimePermission::WorldSessionRead)?;
        let worlds = self
            .host_access
            .world_sessions
            .list_worlds()
            .map_err(map_world_session_access_error)?;
        self.bound_world_session_summaries(worlds)
    }

    fn create(&mut self) -> Result<WitWorldSummary, WorldSessionError> {
        self.require_world_session_permission(RuntimePermission::WorldSessionWrite)?;
        self.consume_world_session_mutation_budget()?;
        let world = self
            .host_access
            .world_sessions
            .create_world()
            .map_err(map_world_session_access_error)?;
        self.bound_world_session_summary(world)
    }

    fn set_active(&mut self, world_id: String, active: bool) -> Result<(), WorldSessionError> {
        self.require_world_session_permission(RuntimePermission::WorldSessionWrite)?;
        self.consume_world_session_mutation_budget()?;
        if world_id.len() > self.max_host_message_bytes {
            return Err(WorldSessionError::MessageTooLarge);
        }
        self.host_access
            .world_sessions
            .set_active(&world_id, active)
            .map_err(map_world_session_access_error)
    }
}

impl WorldCommandsHost for WasmHostState {
    fn submit(
        &mut self,
        request: WitWorldCommandRequest,
    ) -> Result<WitAcceptedWorldCommand, WorldCommandError> {
        self.require_world_command_submit()?;
        self.consume_world_command_submission_budget()?;
        let actor_bytes = match &request.actor {
            WitWorldCommandActor::Principal => 0,
            WitWorldCommandActor::Entity(entity_id) => entity_id.len(),
        };
        let message_bytes = request
            .world_id
            .len()
            .saturating_add(request.schema.len())
            .saturating_add(actor_bytes)
            .saturating_add(request.payload_json.len());
        if message_bytes > self.max_host_message_bytes {
            return Err(WorldCommandError::MessageTooLarge);
        }

        let actor = match request.actor {
            WitWorldCommandActor::Principal => HostWorldCommandActor::Principal,
            WitWorldCommandActor::Entity(entity_id) => HostWorldCommandActor::Entity(entity_id),
        };
        let accepted = self
            .host_access
            .world_commands
            .submit_world_command(WorldCommandRequest {
                world_id: request.world_id,
                schema: request.schema,
                actor,
                expected_position: request.expected_position,
                payload_json: request.payload_json,
            })
            .map_err(map_world_command_access_error)?;
        Ok(WitAcceptedWorldCommand {
            command_id: accepted.command_id,
            correlation_id: accepted.correlation_id,
        })
    }
}

impl WorldProjectionsHost for WasmHostState {
    fn request_read(
        &mut self,
        request: WitWorldProjectionRequest,
    ) -> Result<WitAcceptedWorldProjectionRead, WorldProjectionError> {
        let owner = self.require_world_projection_read()?;
        let message_bytes = request
            .world_id
            .len()
            .saturating_add(request.schema.len())
            .saturating_add(request.input_json.len());
        if message_bytes > self.max_host_message_bytes {
            return Err(WorldProjectionError::MessageTooLarge);
        }
        self.consume_world_projection_read_budget()?;
        let accepted = self
            .host_access
            .world_projections
            .request_projection(
                &owner,
                WorldProjectionRequest {
                    world_id: request.world_id,
                    schema: request.schema,
                    input_json: request.input_json,
                },
            )
            .map_err(map_world_projection_access_error)?;
        Ok(WitAcceptedWorldProjectionRead {
            operation_id: accepted.operation_id,
        })
    }

    fn read_status(
        &mut self,
        operation_id: String,
    ) -> Result<WitWorldProjectionReadState, WorldProjectionError> {
        let owner = self.require_world_projection_read()?;
        if operation_id.len() > self.max_host_message_bytes {
            return Err(WorldProjectionError::MessageTooLarge);
        }
        let status = self
            .host_access
            .world_projections
            .projection_status(&owner, &operation_id)
            .map_err(map_world_projection_access_error)?;
        match status {
            WorldProjectionReadStatus::Pending => Ok(WitWorldProjectionReadState::Pending),
            WorldProjectionReadStatus::Succeeded(view) => {
                let message_bytes = view
                    .world_id
                    .len()
                    .saturating_add(view.schema.len())
                    .saturating_add(view.value_json.len());
                if message_bytes > self.max_host_message_bytes {
                    return Err(WorldProjectionError::MessageTooLarge);
                }
                Ok(WitWorldProjectionReadState::Succeeded(
                    WitWorldProjectionView {
                        world_id: view.world_id,
                        snapshot_position: view.snapshot_position,
                        schema: view.schema,
                        value_json: view.value_json,
                    },
                ))
            }
            WorldProjectionReadStatus::Failed(reason) => {
                if reason.len() > self.max_host_message_bytes {
                    return Err(WorldProjectionError::MessageTooLarge);
                }
                Ok(WitWorldProjectionReadState::Failed(reason))
            }
        }
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

impl ScopedRuntimePolicyHost for WasmHostState {
    fn list_components(
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

    fn set_world_default(
        &mut self,
        subject: String,
        world_default: bool,
    ) -> Result<(), CompositionError> {
        self.require_composition_write()?;
        if subject.len() > self.max_host_message_bytes {
            return Err(CompositionError::Rejected);
        }
        self.host_access
            .composition
            .set_world_default(&subject, world_default)
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
}

impl ScopedCompositionHost for WasmHostState {
    fn list_activations(
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

    fn select_artifact(
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

    fn set_enabled(
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

    fn remove_activation(
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
    fn require_user_content_read(&self) -> Result<(), UserContentError> {
        if !self.host_access_active {
            return Err(UserContentError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::UserContentRead)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => UserContentError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => UserContentError::Unavailable,
            })
    }

    fn require_user_content_write(&self) -> Result<ComponentRef, UserContentError> {
        if !self.host_access_active {
            return Err(UserContentError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::UserContentWrite)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => UserContentError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => UserContentError::Unavailable,
            })
    }

    fn consume_user_content_write_budget(&mut self) -> Result<(), UserContentError> {
        if self.user_content_writes_this_execution >= self.max_user_content_writes_per_execution {
            return Err(UserContentError::LimitExceeded);
        }
        self.user_content_writes_this_execution =
            self.user_content_writes_this_execution.saturating_add(1);
        Ok(())
    }

    fn bound_user_content_entries(
        &self,
        entries: Vec<UserContentSummary>,
    ) -> Result<Vec<WitUserContentEntry>, UserContentError> {
        let message_bytes = entries.iter().fold(0_usize, |total, entry| {
            total
                .saturating_add(entry.id.len())
                .saturating_add(entry.content.len())
                .saturating_add(entry.revision.len())
        });
        if message_bytes > self.max_host_message_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        Ok(entries.into_iter().map(to_wit_user_content_entry).collect())
    }

    fn bound_user_content_document(
        &self,
        document: UserContentDocument,
    ) -> Result<WitUserContentDocument, UserContentError> {
        let message_bytes = document
            .metadata
            .id
            .len()
            .saturating_add(document.metadata.content.len())
            .saturating_add(document.metadata.revision.len())
            .saturating_add(document.descriptor.len());
        if message_bytes > self.max_host_message_bytes {
            return Err(UserContentError::MessageTooLarge);
        }
        Ok(WitUserContentDocument {
            metadata: to_wit_user_content_entry(document.metadata),
            descriptor: document.descriptor,
        })
    }

    fn bound_user_content_write_status(
        &self,
        status: UserContentWriteStatus,
    ) -> Result<WitUserContentWriteState, UserContentError> {
        match status {
            UserContentWriteStatus::Pending => Ok(WitUserContentWriteState::Pending),
            UserContentWriteStatus::Succeeded(entry) => {
                let entry = self.bound_user_content_entries(vec![entry])?;
                let entry = entry.into_iter().next().ok_or(UserContentError::Rejected)?;
                Ok(WitUserContentWriteState::Succeeded(entry))
            }
            UserContentWriteStatus::Failed(diagnostic) => {
                if diagnostic.len() > self.max_host_message_bytes {
                    return Err(UserContentError::MessageTooLarge);
                }
                Ok(WitUserContentWriteState::Failed(diagnostic))
            }
        }
    }

    fn require_world_projection_read(&self) -> Result<ComponentRef, WorldProjectionError> {
        if !self.host_access_active {
            return Err(WorldProjectionError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::WorldProjectionRead)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => WorldProjectionError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => WorldProjectionError::Unavailable,
            })
    }

    fn consume_world_projection_read_budget(&mut self) -> Result<(), WorldProjectionError> {
        if self.world_projection_reads_this_execution
            >= self.max_world_projection_reads_per_execution
        {
            return Err(WorldProjectionError::LimitExceeded);
        }
        self.world_projection_reads_this_execution =
            self.world_projection_reads_this_execution.saturating_add(1);
        Ok(())
    }

    fn require_world_command_submit(&self) -> Result<(), WorldCommandError> {
        if !self.host_access_active {
            return Err(WorldCommandError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::WorldCommandSubmit)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => WorldCommandError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => WorldCommandError::Unavailable,
            })
    }

    fn consume_world_command_submission_budget(&mut self) -> Result<(), WorldCommandError> {
        if self.world_command_submissions_this_execution
            >= self.max_world_command_submissions_per_execution
        {
            return Err(WorldCommandError::LimitExceeded);
        }
        self.world_command_submissions_this_execution = self
            .world_command_submissions_this_execution
            .saturating_add(1);
        Ok(())
    }

    fn require_world_session_permission(
        &self,
        permission: RuntimePermission,
    ) -> Result<(), WorldSessionError> {
        if !self.host_access_active {
            return Err(WorldSessionError::AccessNotActive);
        }
        self.runtime_permission_owner(permission)
            .map(|_| ())
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => WorldSessionError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => WorldSessionError::Unavailable,
            })
    }

    fn consume_world_session_mutation_budget(&mut self) -> Result<(), WorldSessionError> {
        if self.world_session_mutations_this_execution
            >= self.max_world_session_mutations_per_execution
        {
            return Err(WorldSessionError::LimitExceeded);
        }
        self.world_session_mutations_this_execution = self
            .world_session_mutations_this_execution
            .saturating_add(1);
        Ok(())
    }

    fn bound_world_session_summaries(
        &self,
        worlds: Vec<WorldSessionSummary>,
    ) -> Result<Vec<WitWorldSummary>, WorldSessionError> {
        let message_bytes = worlds.iter().fold(0_usize, |total, world| {
            total
                .saturating_add(world.world_id.len())
                .saturating_add(world.last_error.as_ref().map_or(0, String::len))
                .saturating_add(24)
        });
        if message_bytes > self.max_host_message_bytes {
            return Err(WorldSessionError::MessageTooLarge);
        }
        Ok(worlds.into_iter().map(to_wit_world_summary).collect())
    }

    fn bound_world_session_summary(
        &self,
        world: WorldSessionSummary,
    ) -> Result<WitWorldSummary, WorldSessionError> {
        if world
            .world_id
            .len()
            .saturating_add(world.last_error.as_ref().map_or(0, String::len))
            .saturating_add(24)
            > self.max_host_message_bytes
        {
            return Err(WorldSessionError::MessageTooLarge);
        }
        Ok(to_wit_world_summary(world))
    }

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

fn to_wit_world_summary(world: WorldSessionSummary) -> WitWorldSummary {
    WitWorldSummary {
        world_id: world.world_id,
        commit_position: world.commit_position,
        active: world.active,
        pending_active: world.pending_active,
        last_error: world.last_error,
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
        world_default: activation.world_default,
    }
}

fn to_wit_user_content_entry(entry: UserContentSummary) -> WitUserContentEntry {
    WitUserContentEntry {
        id: entry.id,
        content: entry.content,
        revision: entry.revision,
    }
}

fn map_world_projection_access_error(error: WorldProjectionAccessError) -> WorldProjectionError {
    match error {
        WorldProjectionAccessError::InvalidWorldId => WorldProjectionError::InvalidWorldId,
        WorldProjectionAccessError::InvalidSchema => WorldProjectionError::InvalidSchema,
        WorldProjectionAccessError::InvalidInput => WorldProjectionError::InvalidInput,
        WorldProjectionAccessError::NotFound => WorldProjectionError::NotFound,
        WorldProjectionAccessError::WorldNotActive => WorldProjectionError::WorldNotActive,
        WorldProjectionAccessError::QueueFull => WorldProjectionError::QueueFull,
        WorldProjectionAccessError::Rejected => WorldProjectionError::Rejected,
        WorldProjectionAccessError::Unavailable => WorldProjectionError::Unavailable,
    }
}

fn map_world_command_access_error(error: WorldCommandAccessError) -> WorldCommandError {
    match error {
        WorldCommandAccessError::InvalidWorldId => WorldCommandError::InvalidWorldId,
        WorldCommandAccessError::InvalidSchema => WorldCommandError::InvalidSchema,
        WorldCommandAccessError::InvalidActor => WorldCommandError::InvalidActor,
        WorldCommandAccessError::InvalidPayload => WorldCommandError::InvalidPayload,
        WorldCommandAccessError::NotFound => WorldCommandError::NotFound,
        WorldCommandAccessError::WorldNotActive => WorldCommandError::WorldNotActive,
        WorldCommandAccessError::QueueFull => WorldCommandError::QueueFull,
        WorldCommandAccessError::Rejected => WorldCommandError::Rejected,
        WorldCommandAccessError::Unavailable => WorldCommandError::Unavailable,
    }
}

fn map_user_content_access_error(error: HostAccessError) -> UserContentError {
    match error {
        HostAccessError::InvalidUserContentId => UserContentError::InvalidId,
        HostAccessError::InvalidContentType => UserContentError::InvalidContent,
        HostAccessError::NotFound => UserContentError::NotFound,
        HostAccessError::QueueFull => UserContentError::QueueFull,
        HostAccessError::Unavailable => UserContentError::Unavailable,
        HostAccessError::Rejected
        | HostAccessError::InvalidArtifact
        | HostAccessError::InvalidAsset
        | HostAccessError::InvalidDigest
        | HostAccessError::InvalidWorldId
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded => UserContentError::Rejected,
    }
}

fn map_world_session_access_error(error: HostAccessError) -> WorldSessionError {
    match error {
        HostAccessError::InvalidWorldId => WorldSessionError::InvalidWorldId,
        HostAccessError::NotFound => WorldSessionError::NotFound,
        HostAccessError::QueueFull => WorldSessionError::QueueFull,
        HostAccessError::Unavailable => WorldSessionError::Unavailable,
        HostAccessError::InvalidArtifact
        | HostAccessError::InvalidAsset
        | HostAccessError::InvalidDigest
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => WorldSessionError::Rejected,
    }
}

fn map_artifact_store_access_error(error: HostAccessError) -> ArtifactStoreError {
    match error {
        HostAccessError::InvalidArtifact | HostAccessError::InvalidDigest => {
            ArtifactStoreError::InvalidArtifact
        }
        HostAccessError::Unavailable => ArtifactStoreError::Unavailable,
        HostAccessError::NotFound
        | HostAccessError::InvalidWorldId
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::QueueFull
        | HostAccessError::InvalidAsset
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => ArtifactStoreError::Rejected,
    }
}

fn map_asset_store_access_error(error: HostAccessError) -> AssetStoreError {
    match error {
        HostAccessError::InvalidAsset | HostAccessError::InvalidDigest => {
            AssetStoreError::InvalidAsset
        }
        HostAccessError::Unavailable => AssetStoreError::Unavailable,
        HostAccessError::InvalidArtifact
        | HostAccessError::InvalidWorldId
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::QueueFull
        | HostAccessError::NotFound
        | HostAccessError::UnsupportedContent
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => AssetStoreError::Rejected,
    }
}

fn map_preference_access_error(error: HostAccessError) -> PreferenceError {
    match error {
        HostAccessError::InvalidPreference => PreferenceError::InvalidPreference,
        HostAccessError::PreferenceQuotaExceeded => PreferenceError::QuotaExceeded,
        HostAccessError::Unavailable => PreferenceError::Unavailable,
        HostAccessError::InvalidArtifact
        | HostAccessError::InvalidAsset
        | HostAccessError::InvalidDigest
        | HostAccessError::InvalidWorldId
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::QueueFull
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
        | HostAccessError::InvalidAsset
        | HostAccessError::InvalidDigest
        | HostAccessError::InvalidWorldId
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::QueueFull
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
        HostAccessError::InvalidAsset
        | HostAccessError::InvalidWorldId
        | HostAccessError::InvalidUserContentId
        | HostAccessError::InvalidContentType
        | HostAccessError::QueueFull
        | HostAccessError::InvalidPermission
        | HostAccessError::InvalidPreference
        | HostAccessError::PreferenceQuotaExceeded
        | HostAccessError::Rejected => CompositionError::Rejected,
        HostAccessError::Unavailable => CompositionError::Unavailable,
    }
}
