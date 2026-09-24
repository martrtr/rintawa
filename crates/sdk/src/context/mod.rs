//! Context traits supplied by the Extension Engine to a component.
//!
//! Contexts expose SDK interfaces only. Engine-owned runtime, storage, and UI
//! implementation details must never escape through this public boundary.

use crate::{
    api::LoggerApi,
    contracts::{ContractConsumer, ContractDefinition, ContractProvider},
    contributions::ContributionDescriptor,
    contributions::WorldSchemaContribution,
    errors::ExtensionResult,
    runtime_effects::RuntimeEffect,
    secrets::{SecretPath, SecretValue},
    services::{ServiceCallError, ServiceCallResult},
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeEffectId, RuntimeScopeId},
    ui::{
        UiLayerDescriptor, UiPatchBatch, UiResult, UiSurfaceContribution, UiSurfaceId,
        UiSurfaceSnapshot,
    },
};

/// The context available to a component during execution.
///
/// This trait provides access to the component's identity and the APIs
/// it can use during execution.
pub trait ComponentContext {
    /// Returns the logical ID of the extension software that owns this component.
    fn extension_id(&self) -> &ExtensionId;

    /// Returns the opaque ID of this concrete extension runtime instance.
    fn extension_instance_id(&self) -> &ExtensionInstanceId;

    /// Returns the runtime composition scope containing this instance.
    fn runtime_scope_id(&self) -> &RuntimeScopeId;

    /// Returns the ID of this component.
    fn component_id(&self) -> &ComponentId;

    /// Returns the logger API for this component.
    fn logger(&self) -> &dyn LoggerApi;

    /// Installs a reversible runtime effect owned by this component.
    ///
    /// Hosts that do not provide an effect registry return
    /// [`crate::errors::ExtensionError::RuntimeEffectsUnavailable`]. Effects
    /// are never a substitute for registering durable schemas or service
    /// contracts during the registration phase.
    fn register_runtime_effect(
        &mut self,
        _effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        Err(crate::errors::ExtensionError::RuntimeEffectsUnavailable)
    }

    /// Revokes a runtime effect previously installed by this component.
    ///
    /// The host rejects attempts to revoke an effect owned by another
    /// component. Hosts remove any remaining owner effects on stop or crash.
    fn revoke_runtime_effect(&mut self, _effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::RuntimeEffectsUnavailable)
    }

    /// Revokes every runtime effect owned by this component.
    ///
    /// Hosts use this during component failure handling. Extension code normally
    /// revokes one known effect with [`Self::revoke_runtime_effect`] instead.
    fn revoke_all_runtime_effects(&mut self) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::RuntimeEffectsUnavailable)
    }

    /// Reads a secret through this component's active host-granted capability.
    ///
    /// The caller identity comes from the host context, not from extension
    /// input. Contexts supplied during `stop` and registration do not permit
    /// secret reads. Extensions cannot enumerate, store, or delete secrets.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::ExtensionError::SecretAccess`] when the path is
    /// not granted, absent, or unavailable from the host credential store.
    fn read_secret(&self, _path: &SecretPath) -> ExtensionResult<SecretValue> {
        Err(crate::errors::ExtensionError::SecretAccess(
            crate::secrets::SecretAccessError::AccessDenied,
        ))
    }

    /// Calls one provider selected for a versioned contract.
    ///
    /// # Errors
    ///
    /// Returns a transport-level [`ServiceCallError`] when no usable provider
    /// exists, the caller is not a declared consumer, or provider execution fails.
    fn call_service(
        &mut self,
        _contract: &crate::contracts::ContractKey,
        _request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        Err(ServiceCallError::Unavailable)
    }

    /// Mounts the initial presentation snapshot for one statically registered surface.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::ui::UiError`] when the surface cannot be mounted.
    fn mount_ui_surface(&mut self, _snapshot: UiSurfaceSnapshot) -> UiResult<()> {
        Err(crate::ui::UiError::RuntimeUnavailable)
    }

    /// Applies one incremental patch batch to an owned mounted surface.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::ui::UiError`] when the patch batch is invalid or cannot be applied.
    fn patch_ui_surface(&mut self, _batch: UiPatchBatch) -> UiResult<()> {
        Err(crate::ui::UiError::RuntimeUnavailable)
    }

    /// Removes the current presentation snapshot for an owned surface.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::ui::UiError`] when the surface is unavailable or not owned by the caller.
    fn unmount_ui_surface(&mut self, _surface_id: &UiSurfaceId) -> UiResult<()> {
        Err(crate::ui::UiError::RuntimeUnavailable)
    }
}

/// The context available during component registration.
///
/// This trait extends [`ComponentContext`] with registration capabilities,
/// allowing components to register their contributions.
pub trait RegistrationContext: ComponentContext {
    /// Registers a contribution with the system.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::ExtensionError::DuplicateContribution`] if another active
    /// registration already owns the contribution identifier.
    fn register(&mut self, contribution: ContributionDescriptor) -> ExtensionResult<()>;

    /// Defines one versioned contract.
    ///
    /// # Errors
    ///
    /// Returns an error when this context cannot register contract metadata or
    /// the component already defined the same contract.
    fn define_contract(&mut self, _definition: ContractDefinition) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::ContractRegistrationUnavailable)
    }

    /// Registers this component as a provider of one contract.
    ///
    /// # Errors
    ///
    /// Returns an error when this context cannot register contract metadata or
    /// the component already provides the same contract.
    fn provide_contract(&mut self, _provider: ContractProvider) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::ContractRegistrationUnavailable)
    }

    /// Registers this component as a consumer of one contract.
    ///
    /// # Errors
    ///
    /// Returns an error when this context cannot register contract metadata or
    /// the component already consumes the same contract.
    fn consume_contract(&mut self, _consumer: ContractConsumer) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::ContractRegistrationUnavailable)
    }

    /// Declares one immutable versioned world schema owned by this extension.
    ///
    /// The declaration carries no owner field; the host assigns the current
    /// extension identity when it commits registration metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when world-schema registration is unavailable or the same
    /// schema key was already declared by this extension instance.
    fn register_world_schema(&mut self, _schema: WorldSchemaContribution) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::WorldSchemaRegistrationUnavailable)
    }

    /// Registers one static portable UI surface owned by this component.
    ///
    /// # Errors
    ///
    /// Returns an error when the current host context cannot register UI metadata.
    fn register_ui_surface(&mut self, _surface: UiSurfaceContribution) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::UiRegistrationUnavailable)
    }

    /// Registers this component as a renderer for the portable UI protocol.
    ///
    /// The component must separately provide the platform `rintawa.ui.layer@1`
    /// binding. The host selects one provider through normal composition policy
    /// before attaching its descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when the current host context cannot register UI metadata.
    fn register_ui_layer(&mut self, _descriptor: UiLayerDescriptor) -> ExtensionResult<()> {
        Err(crate::errors::ExtensionError::UiRegistrationUnavailable)
    }
}
