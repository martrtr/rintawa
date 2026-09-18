//! Implementation of SDK contexts provided by the Extension Engine.

use rintawa_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contracts::{ContractConsumer, ContractDefinition, ContractProvider},
    contributions::ContributionDescriptor,
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    secrets::{SecretPath, SecretValue},
    services::{ServiceCallError, ServiceCallResult},
    types::{
        ComponentId, ContributionId, ExtensionId, ExtensionInstanceId, RuntimeEffectId,
        RuntimeScopeId,
    },
    ui::{
        UiLayerDescriptor, UiPatchBatch, UiResult, UiSurfaceContribution, UiSurfaceId,
        UiSurfaceSnapshot,
    },
};
use std::collections::HashSet;
use tracing::{debug, error, info, trace, warn};

use rintawa_ui_runtime::{OwnedUiLayerDescriptor, OwnedUiSurfaceContribution, UiRuntime};

use crate::{
    composition::{OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider},
    runtime_effects::RuntimeEffectRegistry,
    secrets::SecretManager,
    services::ServiceRuntime,
};

/// Runtime identity supplied by the host for one concrete component activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComponentIdentity {
    pub(crate) extension_id: ExtensionId,
    pub(crate) instance_id: ExtensionInstanceId,
    pub(crate) scope_id: RuntimeScopeId,
    pub(crate) component_id: ComponentId,
}

impl ComponentIdentity {
    pub(crate) fn new(
        extension_id: ExtensionId,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
        component_id: ComponentId,
    ) -> Self {
        Self {
            extension_id,
            instance_id,
            scope_id,
            component_id,
        }
    }

    pub(crate) fn owner(&self) -> rintawa_sdk::contracts::ComponentRef {
        rintawa_sdk::contracts::ComponentRef::new(
            self.instance_id.clone(),
            self.component_id.clone(),
        )
    }
}

/// Engine logger redirecting SDK logs to the `tracing` ecosystem.
#[derive(Debug, Clone)]
pub struct EngineLogger {
    identity: ComponentIdentity,
}

impl EngineLogger {
    /// Creates a logger scoped to one concrete component activation.
    pub(crate) fn new(identity: ComponentIdentity) -> Self {
        Self { identity }
    }
}

impl LoggerApi for EngineLogger {
    fn log(&self, level: LogLevel, message: &str) {
        let ext = self.identity.extension_id.as_str();
        let instance = self.identity.instance_id.as_str();
        let scope = self.identity.scope_id.as_str();
        let comp = self.identity.component_id.as_str();

        match level {
            LogLevel::Trace => trace!(target: "extension", ext, instance, scope, comp, "{message}"),
            LogLevel::Debug => debug!(target: "extension", ext, instance, scope, comp, "{message}"),
            LogLevel::Info => info!(target: "extension", ext, instance, scope, comp, "{message}"),
            LogLevel::Warn => warn!(target: "extension", ext, instance, scope, comp, "{message}"),
            LogLevel::Error => error!(target: "extension", ext, instance, scope, comp, "{message}"),
        }
    }
}

/// Mutable registration buffers populated by one component register callback.
pub(crate) struct RegistrationBuffers<'a> {
    pub(crate) contributions: &'a mut Vec<ContributionDescriptor>,
    pub(crate) contract_definitions: &'a mut Vec<OwnedContractDefinition>,
    pub(crate) contract_providers: &'a mut Vec<OwnedContractProvider>,
    pub(crate) contract_consumers: &'a mut Vec<OwnedContractConsumer>,
    pub(crate) ui_surfaces: &'a mut Vec<OwnedUiSurfaceContribution>,
    pub(crate) ui_layers: &'a mut Vec<OwnedUiLayerDescriptor>,
}

/// Registration context provided to components during initialization.
pub struct EngineRegistrationContext<'a> {
    identity: ComponentIdentity,
    logger: EngineLogger,
    buffers: RegistrationBuffers<'a>,
    active_contribution_ids: &'a HashSet<(RuntimeScopeId, ContributionId)>,
}

impl<'a> EngineRegistrationContext<'a> {
    /// Creates a new registration context instance.
    pub(crate) fn new(
        identity: ComponentIdentity,
        buffers: RegistrationBuffers<'a>,
        active_contribution_ids: &'a HashSet<(RuntimeScopeId, ContributionId)>,
    ) -> Self {
        let logger = EngineLogger::new(identity.clone());
        Self {
            identity,
            logger,
            buffers,
            active_contribution_ids,
        }
    }
}

impl<'a> ComponentContext for EngineRegistrationContext<'a> {
    fn extension_id(&self) -> &ExtensionId {
        &self.identity.extension_id
    }

    fn extension_instance_id(&self) -> &ExtensionInstanceId {
        &self.identity.instance_id
    }

    fn runtime_scope_id(&self) -> &RuntimeScopeId {
        &self.identity.scope_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.identity.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }
}

impl<'a> RegistrationContext for EngineRegistrationContext<'a> {
    fn register(&mut self, contribution: ContributionDescriptor) -> ExtensionResult<()> {
        if self
            .active_contribution_ids
            .contains(&(self.identity.scope_id.clone(), contribution.id.clone()))
            || self
                .buffers
                .contributions
                .iter()
                .any(|c| c.id == contribution.id)
        {
            return Err(ExtensionError::DuplicateContribution(
                contribution.id.as_str().to_string(),
            ));
        }

        self.buffers.contributions.push(contribution);
        Ok(())
    }

    fn define_contract(&mut self, definition: ContractDefinition) -> ExtensionResult<()> {
        let owner = self.identity.owner();
        if self.buffers.contract_definitions.iter().any(|registered| {
            registered.owner == owner && registered.definition.contract == definition.contract
        }) {
            return Err(ExtensionError::DuplicateContractDefinition(
                definition.contract.to_string(),
            ));
        }
        self.buffers
            .contract_definitions
            .push(OwnedContractDefinition { owner, definition });
        Ok(())
    }

    fn provide_contract(&mut self, provider: ContractProvider) -> ExtensionResult<()> {
        let owner = self.identity.owner();
        if self.buffers.contract_providers.iter().any(|registered| {
            registered.owner == owner && registered.provider.contract == provider.contract
        }) {
            return Err(ExtensionError::DuplicateContractProvider(
                provider.contract.to_string(),
            ));
        }
        self.buffers
            .contract_providers
            .push(OwnedContractProvider { owner, provider });
        Ok(())
    }

    fn consume_contract(&mut self, consumer: ContractConsumer) -> ExtensionResult<()> {
        let owner = self.identity.owner();
        if self.buffers.contract_consumers.iter().any(|registered| {
            registered.owner == owner && registered.consumer.contract == consumer.contract
        }) {
            return Err(ExtensionError::DuplicateContractConsumer(
                consumer.contract.to_string(),
            ));
        }
        self.buffers
            .contract_consumers
            .push(OwnedContractConsumer { owner, consumer });
        Ok(())
    }

    fn register_ui_surface(&mut self, surface: UiSurfaceContribution) -> ExtensionResult<()> {
        let owner = self.identity.owner();
        if self
            .buffers
            .ui_surfaces
            .iter()
            .any(|registered| registered.contribution.id == surface.id)
        {
            return Err(ExtensionError::DuplicateUiSurface(surface.id.to_string()));
        }
        self.buffers.ui_surfaces.push(OwnedUiSurfaceContribution {
            owner,
            contribution: surface,
        });
        Ok(())
    }

    fn register_ui_layer(&mut self, descriptor: UiLayerDescriptor) -> ExtensionResult<()> {
        let owner = self.identity.owner();
        if self
            .buffers
            .ui_layers
            .iter()
            .any(|registered| registered.owner == owner)
        {
            return Err(ExtensionError::DuplicateUiLayerDescriptor);
        }
        self.buffers
            .ui_layers
            .push(OwnedUiLayerDescriptor { owner, descriptor });
        Ok(())
    }
}

/// Context provided to components during start and stop execution phases.
pub struct EngineComponentContext<'a> {
    identity: ComponentIdentity,
    logger: EngineLogger,
    runtime_effects: &'a mut RuntimeEffectRegistry,
    secrets: &'a SecretManager,
    services: &'a ServiceRuntime,
    ui: &'a UiRuntime,
    execution_active: bool,
}

impl<'a> EngineComponentContext<'a> {
    /// Creates a new execution context.
    pub(crate) fn new(
        identity: ComponentIdentity,
        runtime_effects: &'a mut RuntimeEffectRegistry,
        secrets: &'a SecretManager,
        services: &'a ServiceRuntime,
        ui: &'a UiRuntime,
        execution_active: bool,
    ) -> Self {
        let logger = EngineLogger::new(identity.clone());
        Self {
            identity,
            logger,
            runtime_effects,
            secrets,
            services,
            ui,
            execution_active,
        }
    }
}

impl ComponentContext for EngineComponentContext<'_> {
    fn extension_id(&self) -> &ExtensionId {
        &self.identity.extension_id
    }

    fn extension_instance_id(&self) -> &ExtensionInstanceId {
        &self.identity.instance_id
    }

    fn runtime_scope_id(&self) -> &RuntimeScopeId {
        &self.identity.scope_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.identity.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }

    fn register_runtime_effect(
        &mut self,
        effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        self.runtime_effects.register(self.identity.owner(), effect)
    }

    fn revoke_runtime_effect(&mut self, effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
        self.runtime_effects
            .revoke(effect_id, &self.identity.owner())
    }

    fn revoke_all_runtime_effects(&mut self) -> ExtensionResult<()> {
        self.runtime_effects
            .revoke_component(&self.identity.owner());
        Ok(())
    }

    fn read_secret(&self, path: &SecretPath) -> ExtensionResult<SecretValue> {
        if !self.execution_active {
            return Err(ExtensionError::SecretAccess(
                rintawa_sdk::secrets::SecretAccessError::AccessDenied,
            ));
        }

        self.secrets
            .read_for_component(&self.identity.owner(), path)
            .map_err(ExtensionError::from)
    }

    fn call_service(
        &mut self,
        contract: &rintawa_sdk::contracts::ContractKey,
        request: &[u8],
    ) -> ServiceCallResult<Vec<u8>> {
        if !self.execution_active {
            return Err(ServiceCallError::Unavailable);
        }
        self.services
            .call_from_execution(&self.identity.owner(), contract, request)
    }

    fn mount_ui_surface(&mut self, snapshot: UiSurfaceSnapshot) -> UiResult<()> {
        if !self.execution_active {
            return Err(rintawa_sdk::ui::UiError::OwnerInactive);
        }
        self.ui.mount_surface(&self.identity.owner(), snapshot)
    }

    fn patch_ui_surface(&mut self, batch: UiPatchBatch) -> UiResult<()> {
        if !self.execution_active {
            return Err(rintawa_sdk::ui::UiError::OwnerInactive);
        }
        self.ui.apply_patches(&self.identity.owner(), batch)
    }

    fn unmount_ui_surface(&mut self, surface_id: &UiSurfaceId) -> UiResult<()> {
        if !self.execution_active {
            return Err(rintawa_sdk::ui::UiError::OwnerInactive);
        }
        self.ui.unmount_surface(&self.identity.owner(), surface_id)
    }
}
