//! Main Extension Engine implementation managing lifecycle and contributions.

use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey},
    contributions::ContributionDescriptor,
    manifest::ExtensionManifest,
    runtime_effects::RuntimeEffect,
    secrets::SecretPathPattern,
    traits::Component,
    types::{ComponentId, ContributionId, ExtensionId, RuntimeEffectId},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use crate::{
    composition::{
        CompositionSnapshot, OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider,
        resolve_contracts,
    },
    context::{EngineComponentContext, EngineRegistrationContext},
    errors::{ComponentStopFailure, EngineError, EngineResult},
    runtime::WasmRuntimeEngine,
    runtime_effects::RuntimeEffectRegistry,
    secrets::SecretManager,
    services::{ComponentHandle, ServiceRuntime},
};

/// Represents the active lifecycle state of an extension in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionState {
    /// Extension is registered and contributions are indexed.
    Registered,
    /// Extension components are active.
    Active,
    /// Host-owned contributions and runtime effects are deactivated.
    ///
    /// A transition can still return [`EngineError::StopFailed`] when one or
    /// more component callbacks could not clean up their own external state.
    Stopped,
}

struct ManagedExtension {
    manifest: ExtensionManifest,
    state: ExtensionState,
    components: Vec<ManagedComponent>,
    contributions: Vec<OwnedContribution>,
    contract_definitions: Vec<OwnedContractDefinition>,
    contract_providers: Vec<OwnedContractProvider>,
    contract_consumers: Vec<OwnedContractConsumer>,
}

struct ManagedComponent {
    id: ComponentId,
    handle: ComponentHandle,
}

impl ManagedComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn start(
        &self,
        context: &mut dyn rintawa_sdk::context::ComponentContext,
    ) -> rintawa_sdk::errors::ExtensionResult<()> {
        let mut component = self.handle.lock().map_err(|_| {
            rintawa_sdk::errors::ExtensionError::Message(String::from(
                "component lock was poisoned",
            ))
        })?;
        component.start(context)
    }

    fn stop(
        &self,
        context: &mut dyn rintawa_sdk::context::ComponentContext,
    ) -> rintawa_sdk::errors::ExtensionResult<()> {
        let mut component = self.handle.lock().map_err(|_| {
            rintawa_sdk::errors::ExtensionError::Message(String::from(
                "component lock was poisoned",
            ))
        })?;
        component.stop(context)
    }
}

/// A contribution registered by a specific component within an extension.
struct OwnedContribution {
    component_id: ComponentId,
    descriptor: ContributionDescriptor,
}

/// The core engine managing extensions, native components, and contributions.
pub struct ExtensionEngine {
    extensions: HashMap<ExtensionId, ManagedExtension>,
    active_contributions: HashSet<ContributionId>,
    runtime_effects: RuntimeEffectRegistry,
    secrets: SecretManager,
    services: ServiceRuntime,
    preferred_contract_providers: HashMap<ContractKey, ComponentRef>,
}

impl Default for ExtensionEngine {
    fn default() -> Self {
        let secrets = SecretManager::default();
        let services = ServiceRuntime::new(secrets.clone());
        Self {
            extensions: HashMap::new(),
            active_contributions: HashSet::new(),
            runtime_effects: RuntimeEffectRegistry::default(),
            secrets,
            services,
            preferred_contract_providers: HashMap::new(),
        }
    }
}

impl ExtensionEngine {
    /// Creates a new, empty [`ExtensionEngine`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an engine using a host-configured secret manager.
    ///
    /// Rintawa should supply its production credential-store manager here.
    /// The engine's internal tests use an in-memory vault separately.
    pub fn with_secret_manager(secrets: SecretManager) -> Self {
        let services = ServiceRuntime::new(secrets.clone());
        Self {
            extensions: HashMap::new(),
            active_contributions: HashSet::new(),
            runtime_effects: RuntimeEffectRegistry::default(),
            secrets,
            services,
            preferred_contract_providers: HashMap::new(),
        }
    }

    /// Returns the trusted host secret manager used by this engine.
    ///
    /// This handle is for Rintawa configuration and policy code, not for an
    /// extension component. Components receive only `ComponentContext`.
    pub fn secret_manager(&self) -> &SecretManager {
        &self.secrets
    }

    /// Creates a WASM runtime bound to this engine's secret policy.
    ///
    /// Use this factory for components that may request `secret-read`. Creating
    /// an independent [`WasmRuntimeEngine`] also creates an independent policy
    /// and therefore cannot observe grants configured on this engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn wasm_runtime_engine(&self) -> EngineResult<WasmRuntimeEngine> {
        WasmRuntimeEngine::with_host_services(self.secrets.clone(), self.services.clone())
    }

    /// Parses and validates an extension manifest from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ManifestParse`] if the manifest structure is invalid
    /// TOML, or [`EngineError::ManifestValidation`] if it violates a semantic invariant.
    pub fn parse_manifest(&self, raw_toml: &str) -> EngineResult<ExtensionManifest> {
        let manifest: ExtensionManifest = toml::from_str(raw_toml)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate_contract_definitions(
        &self,
        incoming: &[OwnedContractDefinition],
    ) -> EngineResult<()> {
        let mut definitions = HashMap::new();
        for extension in self.extensions.values() {
            for owned in &extension.contract_definitions {
                definitions.insert(
                    owned.definition.contract.clone(),
                    (owned.definition.resolution, owned.definition.protocol),
                );
            }
        }

        for owned in incoming {
            let definition = &owned.definition;
            if let Some(existing) = definitions.get(&definition.contract) {
                let incoming = (definition.resolution, definition.protocol);
                if *existing != incoming {
                    return Err(EngineError::ContractDefinitionConflict {
                        contract: definition.contract.to_string(),
                        existing: format!("{}/{}", existing.1, existing.0),
                        incoming: format!("{}/{}", incoming.1, incoming.0),
                    });
                }
            } else {
                definitions.insert(
                    definition.contract.clone(),
                    (definition.resolution, definition.protocol),
                );
            }
        }
        Ok(())
    }

    /// Registers an extension and its native components into the engine.
    ///
    /// Runs the `register` phase on all supplied components and tracks contributions.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ManifestValidation`] if the manifest violates a
    /// semantic invariant, [`EngineError::ExtensionAlreadyExists`] if the extension
    /// ID is taken, or [`EngineError::LifecycleFailed`] if duplicate contributions
    /// or component errors occur.
    pub fn register_extension(
        &mut self,
        manifest: ExtensionManifest,
        mut components: Vec<Box<dyn Component>>,
    ) -> EngineResult<()> {
        manifest.validate()?;

        if self.extensions.contains_key(&manifest.id) {
            return Err(EngineError::ExtensionAlreadyExists(
                manifest.id.as_str().to_string(),
            ));
        }

        let mut registered_descriptors = Vec::new();
        let mut extension_contributions = Vec::new();
        let mut contract_definitions = Vec::new();
        let mut contract_providers = Vec::new();
        let mut contract_consumers = Vec::new();

        for comp in &mut components {
            let first_contribution = registered_descriptors.len();
            let mut ctx = EngineRegistrationContext::new(
                manifest.id.clone(),
                comp.id().clone(),
                &mut registered_descriptors,
                &mut contract_definitions,
                &mut contract_providers,
                &mut contract_consumers,
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

        self.validate_contract_definitions(&contract_definitions)?;

        let components: Vec<_> = components
            .into_iter()
            .map(|component| {
                let id = component.id().clone();
                ManagedComponent {
                    id,
                    handle: Arc::new(Mutex::new(component)),
                }
            })
            .collect();

        self.services
            .register_extension(
                manifest.id.clone(),
                contract_definitions.clone(),
                contract_providers.clone(),
                contract_consumers.clone(),
                components
                    .iter()
                    .map(|component| (component.id.clone(), component.handle.clone())),
            )
            .map_err(|()| EngineError::ServiceRuntimeUnavailable)?;

        for contrib in &extension_contributions {
            self.active_contributions
                .insert(contrib.descriptor.id.clone());
        }

        let managed = ManagedExtension {
            manifest: manifest.clone(),
            state: ExtensionState::Registered,
            components,
            contributions: extension_contributions,
            contract_definitions,
            contract_providers,
            contract_consumers,
        };

        self.extensions.insert(manifest.id, managed);
        Ok(())
    }

    /// Approves a manifest-requested read grant for one component.
    ///
    /// The granted pattern must be equal to, or narrower than, a pattern in
    /// the extension's manifest. The grant takes effect only while an active
    /// execution context exposes the secret-read capability.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] for an unknown extension,
    /// [`EngineError::SecretComponentNotFound`] for an undeclared component, or
    /// [`EngineError::SecretPermissionNotRequested`] if policy tries to expand
    /// beyond the manifest request.
    pub fn grant_requested_secret_read(
        &self,
        extension_id: &ExtensionId,
        component_id: &ComponentId,
        pattern: SecretPathPattern,
    ) -> EngineResult<()> {
        let extension = self
            .extensions
            .get(extension_id)
            .ok_or_else(|| EngineError::ExtensionNotFound(extension_id.as_str().to_string()))?;

        let Some(component) = extension
            .manifest
            .components
            .iter()
            .find(|component| &component.id == component_id)
        else {
            return Err(EngineError::SecretComponentNotFound {
                extension_id: extension_id.as_str().to_string(),
                component_id: component_id.as_str().to_string(),
            });
        };

        if !component
            .permissions
            .secret_read
            .iter()
            .any(|requested| requested.allows_pattern(&pattern))
        {
            return Err(EngineError::SecretPermissionNotRequested {
                extension_id: extension_id.as_str().to_string(),
                component_id: component_id.as_str().to_string(),
                pattern: pattern.to_string(),
            });
        }

        self.secrets
            .grant_read(extension_id.clone(), component_id.clone(), pattern)?;
        Ok(())
    }

    /// Activates an extension by executing `start` on all its components.
    ///
    /// Re-activates registered contributions if resuming from `Stopped` state.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension isn't registered,
    /// [`EngineError::LifecycleFailed`] if a component fails to start or contributions conflict,
    /// or [`EngineError::StartupRollbackFailed`] if startup fails and one or more rollback
    /// `stop` callbacks fail as well. Host-owned runtime effects are revoked before either
    /// startup error is returned.
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
                &self.secrets,
                &self.services,
                true,
            );

            if let Err(err) = comp.start(&mut ctx) {
                let failed_component_id = comp.id().as_str().to_string();
                let start_reason = err.to_string();
                let mut rollback_failures = Vec::new();

                let mut failed_stop_context = EngineComponentContext::new(
                    ext.manifest.id.clone(),
                    comp.id().clone(),
                    runtime_effects,
                    &self.secrets,
                    &self.services,
                    false,
                );
                if let Err(stop_error) = comp.stop(&mut failed_stop_context) {
                    rollback_failures.push(ComponentStopFailure {
                        component_id: failed_component_id.clone(),
                        reason: stop_error.to_string(),
                    });
                }

                // Rollback previously started components in reverse order.
                for comp_to_stop in started.iter_mut().rev() {
                    let component_id = comp_to_stop.id().as_str().to_string();
                    let mut stop_ctx = EngineComponentContext::new(
                        ext.manifest.id.clone(),
                        comp_to_stop.id().clone(),
                        runtime_effects,
                        &self.secrets,
                        &self.services,
                        false,
                    );
                    if let Err(stop_error) = comp_to_stop.stop(&mut stop_ctx) {
                        rollback_failures.push(ComponentStopFailure {
                            component_id,
                            reason: stop_error.to_string(),
                        });
                    }
                }

                // Host-owned effects are revoked even when component rollback fails.
                runtime_effects.revoke_extension(extension_id);

                if ext.state == ExtensionState::Stopped {
                    for contrib in &ext.contributions {
                        self.active_contributions.remove(&contrib.descriptor.id);
                    }
                }

                if rollback_failures.is_empty() {
                    return Err(EngineError::LifecycleFailed {
                        extension_id: extension_id.as_str().to_string(),
                        component_id: failed_component_id,
                        reason: start_reason,
                    });
                }

                return Err(EngineError::StartupRollbackFailed {
                    extension_id: extension_id.as_str().to_string(),
                    component_id: failed_component_id,
                    start_reason,
                    rollback_failures,
                });
            }
        }

        ext.state = ExtensionState::Active;
        self.services.set_active(extension_id, true);
        Ok(())
    }

    /// Stops an extension and deactivates its registered contributions.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension does not exist,
    /// or [`EngineError::StopFailed`] if one or more component callbacks fail.
    /// Host-owned contributions and runtime effects are revoked before that
    /// error is returned, and the extension transitions to `Stopped`.
    pub fn stop_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let (extensions, runtime_effects) = (&mut self.extensions, &mut self.runtime_effects);
        let ext = extensions
            .get_mut(extension_id)
            .ok_or_else(|| EngineError::ExtensionNotFound(extension_id.as_str().to_string()))?;

        if ext.state == ExtensionState::Stopped {
            return Ok(());
        }

        self.services.set_active(extension_id, false);

        let mut stop_failures = Vec::new();
        for comp in ext.components.iter_mut().rev() {
            let component_id = comp.id().as_str().to_string();
            let mut ctx = EngineComponentContext::new(
                ext.manifest.id.clone(),
                comp.id().clone(),
                runtime_effects,
                &self.secrets,
                &self.services,
                false,
            );
            if let Err(err) = comp.stop(&mut ctx) {
                stop_failures.push(ComponentStopFailure {
                    component_id,
                    reason: err.to_string(),
                });
            }
        }

        for contrib in &ext.contributions {
            self.active_contributions.remove(&contrib.descriptor.id);
        }

        runtime_effects.revoke_extension(extension_id);
        ext.state = ExtensionState::Stopped;

        if stop_failures.is_empty() {
            Ok(())
        } else {
            Err(EngineError::StopFailed {
                extension_id: extension_id.as_str().to_string(),
                failures: stop_failures,
            })
        }
    }

    /// Completely unregisters and unloads an extension from the engine.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionNotFound`] if the extension does not exist, or propagates
    /// [`EngineError::StopFailed`] if an active extension cannot stop cleanly. Host-owned cleanup
    /// still completes inside [`Self::stop_extension`] before that error is returned; unloading is
    /// aborted and the extension remains registered in `Stopped` state.
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

        let removed = self.extensions.remove(extension_id);
        if let Some(extension) = removed {
            for component in &extension.manifest.components {
                self.secrets.revoke_component(extension_id, &component.id);
            }
            for component in extension.components {
                self.secrets.revoke_component(extension_id, &component.id);
            }
            self.services.unregister_extension(extension_id);
        }
        Ok(())
    }

    /// Selects the preferred provider for a single-provider contract.
    pub fn set_preferred_contract_provider(
        &mut self,
        contract: ContractKey,
        provider: ComponentRef,
    ) {
        self.preferred_contract_providers
            .insert(contract.clone(), provider.clone());
        self.services.set_preferred_provider(contract, provider);
    }

    /// Removes an explicit preferred-provider selection.
    pub fn clear_preferred_contract_provider(&mut self, contract: &ContractKey) {
        self.preferred_contract_providers.remove(contract);
        self.services.clear_preferred_provider(contract);
    }

    /// Calls a unary service on behalf of an active consumer component.
    ///
    /// # Errors
    ///
    /// Returns a transport-level service error when the consumer has no usable
    /// single-provider binding or provider execution fails.
    pub fn call_service(
        &self,
        consumer: &ComponentRef,
        contract: &ContractKey,
        request: &[u8],
    ) -> rintawa_sdk::services::ServiceCallResult<Vec<u8>> {
        self.services.call(consumer, contract, request)
    }

    /// Resolves the current active contract composition.
    pub fn composition_snapshot(&self) -> CompositionSnapshot {
        let mut definitions = Vec::new();
        let mut providers = Vec::new();
        let mut consumers = Vec::new();

        for extension in self
            .extensions
            .values()
            .filter(|extension| extension.state == ExtensionState::Active)
        {
            definitions.extend(extension.contract_definitions.iter().cloned());
            providers.extend(extension.contract_providers.iter().cloned());
            consumers.extend(extension.contract_consumers.iter().cloned());
        }

        resolve_contracts(
            &definitions,
            &providers,
            &consumers,
            &self.preferred_contract_providers,
            &self.secrets,
        )
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
