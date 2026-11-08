//! Implementation of SDK contexts provided by the Extension Engine.

use std::collections::HashSet;

use rintawa_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contracts::{ContractConsumer, ContractDefinition, ContractProvider},
    contributions::{ContributionDescriptor, WorldSchemaContribution},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    secrets::{SecretPath, SecretValue},
    types::{
        ComponentId, ContributionId, ExtensionId, ExtensionInstanceId, RuntimeEffectId,
        RuntimeScopeId,
    },
};
use tracing::{debug, error, info, trace, warn};

use crate::{
    composition::{OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider},
    runtime_effects::RuntimeEffectRegistry,
    secrets::SecretManager,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedWorldSchemaContribution {
    pub(crate) extension_id: ExtensionId,
    pub(crate) component_id: ComponentId,
    pub(crate) contribution: WorldSchemaContribution,
}

pub(crate) struct RegistrationBuffers<'a> {
    pub(crate) contributions: &'a mut Vec<ContributionDescriptor>,
    pub(crate) world_schemas: &'a mut Vec<OwnedWorldSchemaContribution>,
    pub(crate) contract_definitions: &'a mut Vec<OwnedContractDefinition>,
    pub(crate) contract_providers: &'a mut Vec<OwnedContractProvider>,
    pub(crate) contract_consumers: &'a mut Vec<OwnedContractConsumer>,
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

    fn register_world_schema(&mut self, schema: WorldSchemaContribution) -> ExtensionResult<()> {
        if self
            .buffers
            .world_schemas
            .iter()
            .any(|registered| registered.contribution.key() == schema.key())
        {
            return Err(ExtensionError::DuplicateWorldSchema(
                schema.key().to_string(),
            ));
        }
        self.buffers
            .world_schemas
            .push(OwnedWorldSchemaContribution {
                extension_id: self.identity.extension_id.clone(),
                component_id: self.identity.component_id.clone(),
                contribution: schema,
            });
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
}

/// Context provided to components during start and stop execution phases.
pub struct EngineComponentContext<'a> {
    identity: ComponentIdentity,
    logger: EngineLogger,
    runtime_effects: &'a mut RuntimeEffectRegistry,
    secrets: &'a SecretManager,
    execution_active: bool,
}

impl<'a> EngineComponentContext<'a> {
    /// Creates a new execution context.
    pub(crate) fn new(
        identity: ComponentIdentity,
        runtime_effects: &'a mut RuntimeEffectRegistry,
        secrets: &'a SecretManager,
        execution_active: bool,
    ) -> Self {
        let logger = EngineLogger::new(identity.clone());
        Self {
            identity,
            logger,
            runtime_effects,
            secrets,
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
}
