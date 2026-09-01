//! Implementation of SDK contexts provided by the Extension Engine.

use std::collections::HashSet;
use taverna_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contributions::ContributionDescriptor,
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    types::{ComponentId, ContributionId, ExtensionId, RuntimeEffectId},
};
use tracing::{debug, error, info, trace, warn};

use crate::runtime_effects::RuntimeEffectRegistry;

/// Engine logger redirecting SDK logs to the `tracing` ecosystem.
#[derive(Debug, Clone)]
pub struct EngineLogger {
    extension_id: ExtensionId,
    component_id: ComponentId,
}

impl EngineLogger {
    /// Creates a new logger scoped to an extension and component.
    pub fn new(extension_id: ExtensionId, component_id: ComponentId) -> Self {
        Self {
            extension_id,
            component_id,
        }
    }
}

impl LoggerApi for EngineLogger {
    fn log(&self, level: LogLevel, message: &str) {
        let ext = self.extension_id.as_str();
        let comp = self.component_id.as_str();

        match level {
            LogLevel::Trace => trace!(target: "extension", ext, comp, "{message}"),
            LogLevel::Debug => debug!(target: "extension", ext, comp, "{message}"),
            LogLevel::Info => info!(target: "extension", ext, comp, "{message}"),
            LogLevel::Warn => warn!(target: "extension", ext, comp, "{message}"),
            LogLevel::Error => error!(target: "extension", ext, comp, "{message}"),
        }
    }
}

/// Registration context provided to components during initialization.
pub struct EngineRegistrationContext<'a> {
    extension_id: ExtensionId,
    component_id: ComponentId,
    logger: EngineLogger,
    registered_contributions: &'a mut Vec<ContributionDescriptor>,
    active_contribution_ids: &'a HashSet<ContributionId>,
}

impl<'a> EngineRegistrationContext<'a> {
    /// Creates a new registration context instance.
    pub fn new(
        extension_id: ExtensionId,
        component_id: ComponentId,
        registered_contributions: &'a mut Vec<ContributionDescriptor>,
        active_contribution_ids: &'a HashSet<ContributionId>,
    ) -> Self {
        let logger = EngineLogger::new(extension_id.clone(), component_id.clone());
        Self {
            extension_id,
            component_id,
            logger,
            registered_contributions,
            active_contribution_ids,
        }
    }
}

impl<'a> ComponentContext for EngineRegistrationContext<'a> {
    fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }
}

impl<'a> RegistrationContext for EngineRegistrationContext<'a> {
    fn register(&mut self, contribution: ContributionDescriptor) -> ExtensionResult<()> {
        if self.active_contribution_ids.contains(&contribution.id)
            || self
                .registered_contributions
                .iter()
                .any(|c| c.id == contribution.id)
        {
            return Err(ExtensionError::DuplicateContribution(
                contribution.id.as_str().to_string(),
            ));
        }

        self.registered_contributions.push(contribution);
        Ok(())
    }
}

/// Context provided to components during start and stop execution phases.
pub struct EngineComponentContext<'a> {
    extension_id: ExtensionId,
    component_id: ComponentId,
    logger: EngineLogger,
    runtime_effects: &'a mut RuntimeEffectRegistry,
}

impl<'a> EngineComponentContext<'a> {
    /// Creates a new execution context.
    pub(crate) fn new(
        extension_id: ExtensionId,
        component_id: ComponentId,
        runtime_effects: &'a mut RuntimeEffectRegistry,
    ) -> Self {
        let logger = EngineLogger::new(extension_id.clone(), component_id.clone());
        Self {
            extension_id,
            component_id,
            logger,
            runtime_effects,
        }
    }
}

impl ComponentContext for EngineComponentContext<'_> {
    fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }

    fn register_runtime_effect(
        &mut self,
        effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        self.runtime_effects
            .register(self.extension_id.clone(), self.component_id.clone(), effect)
    }

    fn revoke_runtime_effect(&mut self, effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
        self.runtime_effects
            .revoke(effect_id, &self.extension_id, &self.component_id)
    }

    fn revoke_all_runtime_effects(&mut self) -> ExtensionResult<()> {
        self.runtime_effects
            .revoke_component(&self.extension_id, &self.component_id);
        Ok(())
    }
}
