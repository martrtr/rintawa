//! Main Extension Engine implementation managing lifecycle and contributions.

use std::collections::{HashMap, HashSet};
use taverna_sdk::{
    contributions::ContributionDescriptor,
    manifest::ExtensionManifest,
    runtime_effects::RuntimeEffect,
    traits::Component,
    types::{ComponentId, ContributionId, ExtensionId, RuntimeEffectId},
};

use crate::{
    context::{EngineComponentContext, EngineRegistrationContext},
    errors::{EngineError, EngineResult},
    runtime_effects::RuntimeEffectRegistry,
};

/// Represents the active lifecycle state of an extension in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionState {
    /// Extension is registered and contributions are indexed.
    Registered,
    /// Extension components are active.
    Active,
    /// Extension has been stopped and contributions deactivated.
    Stopped,
}

struct ManagedExtension {
    manifest: ExtensionManifest,
    state: ExtensionState,
    components: Vec<Box<dyn Component>>,
    contributions: Vec<OwnedContribution>,
}

/// A contribution registered by a specific component within an extension.
struct OwnedContribution {
    component_id: ComponentId,
    descriptor: ContributionDescriptor,
}

/// The core engine managing extensions, native components, and contributions.
#[derive(Default)]
pub struct ExtensionEngine {
    extensions: HashMap<ExtensionId, ManagedExtension>,
    active_contributions: HashSet<ContributionId>,
    runtime_effects: RuntimeEffectRegistry,
}

impl ExtensionEngine {
    /// Creates a new, empty [`ExtensionEngine`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses an extension manifest from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ManifestParse`] if the manifest structure is invalid TOML.
    pub fn parse_manifest(&self, raw_toml: &str) -> EngineResult<ExtensionManifest> {
        let manifest: ExtensionManifest = toml::from_str(raw_toml)?;
        Ok(manifest)
    }

    /// Registers an extension and its native components into the engine.
    ///
    /// Runs the `register` phase on all supplied components and tracks contributions.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionAlreadyExists`] if the ID is taken,
    /// or [`EngineError::LifecycleFailed`] if duplicate contributions or component errors occur.
    pub fn register_extension(
        &mut self,
        manifest: ExtensionManifest,
        mut components: Vec<Box<dyn Component>>,
    ) -> EngineResult<()> {
        if self.extensions.contains_key(&manifest.id) {
            return Err(EngineError::ExtensionAlreadyExists(
                manifest.id.as_str().to_string(),
            ));
        }

        let mut registered_descriptors = Vec::new();
        let mut extension_contributions = Vec::new();

        for comp in &mut components {
            let first_contribution = registered_descriptors.len();
            let mut ctx = EngineRegistrationContext::new(
                manifest.id.clone(),
                comp.id().clone(),
                &mut registered_descriptors,
                &self.active_contributions,
            );

            if let Err(err) = comp.register(&mut ctx) {
                return Err(EngineError::LifecycleFailed {
                    extension_id: manifest.id.as_str().to_string(),
                    component_id: comp.id().as_str().to_string(),
                    reason: err.to_string(),
                });
            }

            extension_contributions.extend(
                registered_descriptors[first_contribution..]
                    .iter()
                    .cloned()
                    .map(|descriptor| OwnedContribution {
                        component_id: comp.id().clone(),
                        descriptor,
                    }),
            );
        }

        for contrib in &extension_contributions {
            self.active_contributions
                .insert(contrib.descriptor.id.clone());
        }

        let managed = ManagedExtension {
            manifest: manifest.clone(),
            state: ExtensionState::Registered,
            components,
            contributions: extension_contributions,
        };

        self.extensions.insert(manifest.id, managed);
        Ok(())
    }

    /// Activates an extension by executing `start` on all its components.
    ///
    /// Re-activates registered contributions if resuming from `Stopped` state.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension isn't registered,
    /// or [`EngineError::LifecycleFailed`] if a component fails to start or contributions conflict.
    pub fn start_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let (extensions, runtime_effects) = (&mut self.extensions, &mut self.runtime_effects);
        let ext = extensions
            .get_mut(extension_id)
            .ok_or_else(|| EngineError::ExtensionNotFound(extension_id.as_str().to_string()))?;

        if ext.state == ExtensionState::Active {
            return Ok(());
        }

        // Re-check and re-activate contributions if coming from Stopped state
        if ext.state == ExtensionState::Stopped {
            for contrib in &ext.contributions {
                if self.active_contributions.contains(&contrib.descriptor.id) {
                    return Err(EngineError::LifecycleFailed {
                        extension_id: extension_id.as_str().to_string(),
                        component_id: "engine".to_string(),
                        reason: format!(
                            "contribution conflict on restart: `{}`",
                            contrib.descriptor.id
                        ),
                    });
                }
            }
            for contrib in &ext.contributions {
                self.active_contributions
                    .insert(contrib.descriptor.id.clone());
            }
        }

        let total_components = ext.components.len();

        for i in 0..total_components {
            let (started, remaining) = ext.components.split_at_mut(i);
            let comp = &mut remaining[0];
            let mut ctx = EngineComponentContext::new(
                ext.manifest.id.clone(),
                comp.id().clone(),
                runtime_effects,
            );

            if let Err(err) = comp.start(&mut ctx) {
                let mut failed_stop_context = EngineComponentContext::new(
                    ext.manifest.id.clone(),
                    comp.id().clone(),
                    runtime_effects,
                );
                let _ = comp.stop(&mut failed_stop_context);

                // Rollback previously started components in reverse order
                for comp_to_stop in started.iter_mut().rev() {
                    let mut stop_ctx = EngineComponentContext::new(
                        ext.manifest.id.clone(),
                        comp_to_stop.id().clone(),
                        runtime_effects,
                    );
                    let _ = comp_to_stop.stop(&mut stop_ctx);
                }

                runtime_effects.revoke_extension(extension_id);

                if ext.state == ExtensionState::Stopped {
                    for contrib in &ext.contributions {
                        self.active_contributions.remove(&contrib.descriptor.id);
                    }
                }

                return Err(EngineError::LifecycleFailed {
                    extension_id: extension_id.as_str().to_string(),
                    component_id: comp.id().as_str().to_string(),
                    reason: err.to_string(),
                });
            }
        }

        ext.state = ExtensionState::Active;
        Ok(())
    }

    /// Stops an extension and deactivates its registered contributions.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension does not exist.
    pub fn stop_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let (extensions, runtime_effects) = (&mut self.extensions, &mut self.runtime_effects);
        let ext = extensions
            .get_mut(extension_id)
            .ok_or_else(|| EngineError::ExtensionNotFound(extension_id.as_str().to_string()))?;

        if ext.state == ExtensionState::Stopped {
            return Ok(());
        }

        for comp in ext.components.iter_mut().rev() {
            let mut ctx = EngineComponentContext::new(
                ext.manifest.id.clone(),
                comp.id().clone(),
                runtime_effects,
            );
            if let Err(err) = comp.stop(&mut ctx) {
                tracing::warn!(
                    ext = %extension_id,
                    comp = %comp.id(),
                    "Error during component stop: {err}"
                );
            }
        }

        for contrib in &ext.contributions {
            self.active_contributions.remove(&contrib.descriptor.id);
        }

        runtime_effects.revoke_extension(extension_id);

        ext.state = ExtensionState::Stopped;
        Ok(())
    }

    /// Completely unregisters and unloads an extension from the engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension does not exist.
    pub fn unregister_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        if let Some(ext) = self.extensions.get(extension_id) {
            if ext.state == ExtensionState::Active {
                self.stop_extension(extension_id)?;
            }
        } else {
            return Err(EngineError::ExtensionNotFound(
                extension_id.as_str().to_string(),
            ));
        }

        self.extensions.remove(extension_id);
        Ok(())
    }

    /// Returns a list of all currently active contribution descriptors across registered extensions.
    pub fn active_contributions(&self) -> Vec<ContributionDescriptor> {
        self.extensions
            .values()
            .filter(|ext| ext.state != ExtensionState::Stopped)
            .flat_map(|ext| {
                ext.contributions
                    .iter()
                    .map(|contrib| contrib.descriptor.clone())
            })
            .collect()
    }

    /// Returns the owner of an active contribution, if it is registered.
    pub fn active_contribution_owner(
        &self,
        contribution_id: &ContributionId,
    ) -> Option<(&ExtensionId, &ComponentId)> {
        self.extensions
            .iter()
            .filter(|(_, ext)| ext.state != ExtensionState::Stopped)
            .find_map(|(extension_id, ext)| {
                ext.contributions
                    .iter()
                    .find(|contrib| contrib.descriptor.id == *contribution_id)
                    .map(|contrib| (extension_id, &contrib.component_id))
            })
    }

    /// Returns all active runtime effects together with their extension and component owner.
    pub fn active_runtime_effects(
        &self,
    ) -> Vec<(&RuntimeEffectId, &ExtensionId, &ComponentId, &RuntimeEffect)> {
        self.runtime_effects.active_effects()
    }

    /// Returns the current lifecycle state of an extension, if registered.
    pub fn extension_state(&self, extension_id: &ExtensionId) -> Option<ExtensionState> {
        self.extensions.get(extension_id).map(|ext| ext.state)
    }
}
