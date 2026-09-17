//! Main Extension Engine implementation managing lifecycle and contributions.

use rintawa_sdk::{
    contracts::{ComponentRef, ContractDefinition, ContractKey, ContractResolutionPolicy},
    contributions::ContributionDescriptor,
    manifest::ExtensionManifest,
    runtime_effects::RuntimeEffect,
    runtime_permissions::RuntimePermission,
    secrets::SecretPathPattern,
    traits::Component,
    types::{
        ComponentId, ContributionId, ExtensionId, ExtensionInstanceId, RuntimeEffectId,
        RuntimeScopeId,
    },
    ui::{UiActionEvent, UiLayerDescriptor},
};
use rintawa_ui_runtime::{UiPresentationSurface, UiRuntime};

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    activation::{ActivationPlan, build_activation_plan},
    artifact_host::RtwComponentHost,
    composition::{
        CompositionSnapshot, OwnedContractConsumer, OwnedContractDefinition, OwnedContractProvider,
        UnresolvedContractReason, resolve_contract_providers, resolve_contracts,
    },
    context::{
        ComponentIdentity, EngineComponentContext, EngineRegistrationContext, RegistrationBuffers,
    },
    errors::{ComponentStopFailure, EngineError, EngineResult},
    execution_targets::{ExecutionTargetDependency, ExecutionTargetRegistry},
    runtime::WasmRuntimeEngine,
    runtime_effects::RuntimeEffectRegistry,
    runtime_permissions::RuntimePermissionManager,
    secrets::SecretManager,
    services::{ComponentHandle, ServiceInstanceRegistration, ServiceRuntime},
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
    instance_id: ExtensionInstanceId,
    scope_id: RuntimeScopeId,
    manifest: ExtensionManifest,
    state: ExtensionState,
    components: Vec<ManagedComponent>,
    contributions: Vec<OwnedContribution>,
    contract_definitions: Vec<OwnedContractDefinition>,
    contract_providers: Vec<OwnedContractProvider>,
    contract_consumers: Vec<OwnedContractConsumer>,
    execution_target_dependencies: Vec<ExecutionTargetDependency>,
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

    fn handle_ui_action(
        &self,
        context: &mut dyn rintawa_sdk::context::ComponentContext,
        event: &UiActionEvent,
    ) -> rintawa_sdk::errors::ExtensionResult<()> {
        let mut component = self.handle.lock().map_err(|_| {
            rintawa_sdk::errors::ExtensionError::Message(String::from(
                "component lock was poisoned",
            ))
        })?;
        component.handle_ui_action(context, event)
    }
}

/// A contribution registered by a specific component within an extension.
struct OwnedContribution {
    component_id: ComponentId,
    descriptor: ContributionDescriptor,
}

/// The core engine managing extensions, native components, and contributions.
pub struct ExtensionEngine {
    extensions: HashMap<ExtensionInstanceId, ManagedExtension>,
    active_contributions: HashSet<(RuntimeScopeId, ContributionId)>,
    runtime_effects: RuntimeEffectRegistry,
    runtime_permissions: RuntimePermissionManager,
    secrets: SecretManager,
    services: ServiceRuntime,
    ui: UiRuntime,
    preferred_contract_providers: HashMap<RuntimeScopeId, HashMap<ContractKey, ComponentRef>>,
    platform_contract_definitions:
        HashMap<RuntimeScopeId, HashMap<ContractKey, ContractDefinition>>,
    execution_targets: ExecutionTargetRegistry,
}

/// Runtime scope used by legacy convenience APIs that do not specify one.
pub const DEFAULT_RUNTIME_SCOPE: &str = "default";

fn default_scope_id() -> RuntimeScopeId {
    RuntimeScopeId::new(DEFAULT_RUNTIME_SCOPE)
}

fn default_instance_id(extension_id: &ExtensionId) -> ExtensionInstanceId {
    ExtensionInstanceId::new(extension_id.as_str())
}

impl Default for ExtensionEngine {
    fn default() -> Self {
        let secrets = SecretManager::default();
        let services = ServiceRuntime::new(secrets.clone());
        Self {
            extensions: HashMap::new(),
            active_contributions: HashSet::new(),
            runtime_effects: RuntimeEffectRegistry::default(),
            runtime_permissions: RuntimePermissionManager::default(),
            secrets,
            services,
            ui: UiRuntime::new(),
            preferred_contract_providers: HashMap::new(),
            platform_contract_definitions: HashMap::new(),
            execution_targets: ExecutionTargetRegistry::default(),
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
            runtime_permissions: RuntimePermissionManager::default(),
            secrets,
            services,
            ui: UiRuntime::new(),
            preferred_contract_providers: HashMap::new(),
            platform_contract_definitions: HashMap::new(),
            execution_targets: ExecutionTargetRegistry::default(),
        }
    }

    /// Registers one non-built-in component execution target owned by an active component.
    ///
    /// Execution-target identifiers are process-global ABI identities rather than
    /// runtime-scope composition roles. The built-in WASM target is reserved and
    /// cannot be replaced. The registration is automatically revoked when the
    /// owning extension instance stops or unregisters.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] when the owner instance
    /// is unknown, [`EngineError::ExecutionTargetOwnerInactive`] when the owner
    /// component is absent or inactive, or a target validation/conflict error.
    pub fn register_execution_target_host(
        &self,
        owner: &ComponentRef,
        host: Arc<dyn RtwComponentHost>,
    ) -> EngineResult<()> {
        let extension = self
            .extensions
            .get(&owner.instance_id)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(owner.instance_id.to_string()))?;
        let owner_is_active = extension.state == ExtensionState::Active
            && extension
                .components
                .iter()
                .any(|component| component.id() == &owner.component_id);
        if !owner_is_active {
            return Err(EngineError::ExecutionTargetOwnerInactive {
                instance_id: owner.instance_id.to_string(),
                component_id: owner.component_id.to_string(),
            });
        }
        self.execution_targets.register(owner.clone(), host)
    }

    /// Returns the active component that owns a registered execution target.
    pub fn execution_target_owner(&self, target: &str) -> Option<ComponentRef> {
        self.execution_targets.owner(target)
    }

    pub(crate) fn resolve_component_host(
        &self,
        target: &str,
    ) -> Option<crate::execution_targets::RegisteredExecutionTargetHost> {
        self.execution_targets.resolve(target)
    }

    fn execution_target_dependents_of(
        &self,
        provider_instance_id: &ExtensionInstanceId,
    ) -> Vec<ExtensionInstanceId> {
        let mut dependents: Vec<_> = self
            .extensions
            .values()
            .filter(|extension| &extension.instance_id != provider_instance_id)
            .filter(|extension| {
                extension
                    .execution_target_dependencies
                    .iter()
                    .any(|dependency| &dependency.provider.instance_id == provider_instance_id)
            })
            .map(|extension| extension.instance_id.clone())
            .collect();
        dependents.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        dependents.dedup();
        dependents
    }

    fn ensure_execution_target_provider_not_in_use(
        &self,
        provider_instance_id: &ExtensionInstanceId,
    ) -> EngineResult<()> {
        let dependents = self.execution_target_dependents_of(provider_instance_id);
        if dependents.is_empty() {
            return Ok(());
        }
        Err(EngineError::ExecutionTargetProviderInUse {
            provider_instance_id: provider_instance_id.to_string(),
            dependents: dependents
                .into_iter()
                .map(|instance| instance.to_string())
                .collect(),
        })
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
        WasmRuntimeEngine::with_host_services(
            self.secrets.clone(),
            self.services.clone(),
            self.ui.clone(),
            self.execution_targets.clone(),
            self.runtime_permissions.clone(),
        )
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
        scope_id: &RuntimeScopeId,
        incoming: &[OwnedContractDefinition],
    ) -> EngineResult<()> {
        let mut definitions = HashMap::new();
        for extension in self
            .extensions
            .values()
            .filter(|extension| &extension.scope_id == scope_id)
        {
            for owned in &extension.contract_definitions {
                definitions.insert(
                    owned.definition.contract.clone(),
                    (owned.definition.resolution, owned.definition.protocol),
                );
            }
        }

        for owned in incoming {
            let definition = &owned.definition;
            if self
                .platform_contract_definitions
                .get(scope_id)
                .is_some_and(|platform| platform.contains_key(&definition.contract))
            {
                return Err(EngineError::PlatformContractDefinitionReserved {
                    contract: definition.contract.to_string(),
                    scope_id: scope_id.to_string(),
                });
            }
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

    /// Defines one platform-owned composition binding inside an exact runtime scope.
    ///
    /// Platform-owned definitions let extensions provide stable host roles without
    /// letting a package redefine the role's resolution policy. Repeating the same
    /// definition is idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::PlatformContractReservationConflict`] if an extension
    /// already defined the contract in this scope, or
    /// [`EngineError::ContractDefinitionConflict`] if the platform already defined
    /// the same contract with another resolution policy.
    pub fn define_platform_binding_contract_in_scope(
        &mut self,
        scope_id: RuntimeScopeId,
        contract: ContractKey,
        resolution: ContractResolutionPolicy,
    ) -> EngineResult<()> {
        let has_extension_definition = self.extensions.values().any(|extension| {
            extension.scope_id == scope_id
                && extension
                    .contract_definitions
                    .iter()
                    .any(|owned| owned.definition.contract == contract)
        });
        if has_extension_definition {
            return Err(EngineError::PlatformContractReservationConflict {
                contract: contract.to_string(),
                scope_id: scope_id.to_string(),
            });
        }

        let definition = ContractDefinition::new(contract.clone(), resolution);
        let definitions = self
            .platform_contract_definitions
            .entry(scope_id)
            .or_default();
        if let Some(existing) = definitions.get(&contract) {
            if existing != &definition {
                return Err(EngineError::ContractDefinitionConflict {
                    contract: contract.to_string(),
                    existing: format!("{}/{}", existing.protocol, existing.resolution),
                    incoming: format!("{}/{}", definition.protocol, definition.resolution),
                });
            }
            return Ok(());
        }
        definitions.insert(contract, definition);
        Ok(())
    }

    /// Registers an extension in the default runtime scope.
    ///
    /// This convenience API preserves the early single-instance workflow. New
    /// supervisors and package managers should call [`Self::register_extension_instance`]
    /// with an explicit opaque instance ID and scope.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::register_extension_instance`].
    pub fn register_extension(
        &mut self,
        manifest: ExtensionManifest,
        components: Vec<Box<dyn Component>>,
    ) -> EngineResult<()> {
        let instance_id = default_instance_id(&manifest.id);
        if self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionAlreadyExists(manifest.id.to_string()));
        }
        self.register_extension_instance(instance_id, default_scope_id(), manifest, components)
    }

    /// Registers one concrete extension runtime instance.
    ///
    /// The logical [`ExtensionId`] comes from the manifest, while `instance_id`
    /// identifies this exact activation and `scope_id` defines its current
    /// composition boundary. Multiple instances of the same logical extension
    /// may coexist when they use distinct instance IDs.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid manifest, duplicate runtime instance,
    /// conflicting registrations inside the same scope, or a component register
    /// callback failure.
    pub fn register_extension_instance(
        &mut self,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
        manifest: ExtensionManifest,
        components: Vec<Box<dyn Component>>,
    ) -> EngineResult<()> {
        self.register_extension_instance_with_target_dependencies(
            instance_id,
            scope_id,
            manifest,
            components,
            Vec::new(),
        )
    }

    pub(crate) fn register_extension_instance_with_target_dependencies(
        &mut self,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
        manifest: ExtensionManifest,
        mut components: Vec<Box<dyn Component>>,
        execution_target_dependencies: Vec<ExecutionTargetDependency>,
    ) -> EngineResult<()> {
        manifest.validate()?;

        if self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionInstanceAlreadyExists(
                instance_id.to_string(),
            ));
        }

        let mut registered_descriptors = Vec::new();
        let mut extension_contributions = Vec::new();
        let mut contract_definitions = Vec::new();
        let mut contract_providers = Vec::new();
        let mut contract_consumers = Vec::new();
        let mut ui_surfaces = Vec::new();

        for comp in &mut components {
            let first_contribution = registered_descriptors.len();
            let buffers = RegistrationBuffers {
                contributions: &mut registered_descriptors,
                contract_definitions: &mut contract_definitions,
                contract_providers: &mut contract_providers,
                contract_consumers: &mut contract_consumers,
                ui_surfaces: &mut ui_surfaces,
            };
            let identity = ComponentIdentity::new(
                manifest.id.clone(),
                instance_id.clone(),
                scope_id.clone(),
                comp.id().clone(),
            );
            let mut ctx =
                EngineRegistrationContext::new(identity, buffers, &self.active_contributions);

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

        self.validate_contract_definitions(&scope_id, &contract_definitions)?;

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
            .register_instance(ServiceInstanceRegistration {
                instance_id: instance_id.clone(),
                extension_id: manifest.id.clone(),
                scope_id: scope_id.clone(),
                definitions: contract_definitions.clone(),
                providers: contract_providers.clone(),
                consumers: contract_consumers.clone(),
                components: components
                    .iter()
                    .map(|component| (component.id.clone(), component.handle.clone()))
                    .collect(),
            })
            .map_err(|()| EngineError::ServiceRuntimeUnavailable)?;

        if let Err(error) =
            self.ui
                .register_instance(instance_id.clone(), scope_id.clone(), ui_surfaces.clone())
        {
            self.services.unregister_instance(&instance_id);
            return Err(error.into());
        }

        for contrib in &extension_contributions {
            self.active_contributions
                .insert((scope_id.clone(), contrib.descriptor.id.clone()));
        }

        let managed = ManagedExtension {
            instance_id: instance_id.clone(),
            scope_id,
            manifest,
            state: ExtensionState::Registered,
            components,
            contributions: extension_contributions,
            contract_definitions,
            contract_providers,
            contract_consumers,
            execution_target_dependencies,
        };

        self.extensions.insert(instance_id, managed);
        Ok(())
    }

    /// Approves a manifest-requested secret grant for the default instance.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::grant_requested_secret_read_for_instance`].
    pub fn grant_requested_secret_read(
        &self,
        extension_id: &ExtensionId,
        component_id: &ComponentId,
        pattern: SecretPathPattern,
    ) -> EngineResult<()> {
        let instance_id = default_instance_id(extension_id);
        if !self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionNotFound(extension_id.to_string()));
        }
        self.grant_requested_secret_read_for_instance(&instance_id, component_id, pattern)
    }

    /// Approves a manifest-requested read grant for one concrete component principal.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] for an unknown instance,
    /// [`EngineError::SecretComponentNotFound`] for an undeclared component, or
    /// [`EngineError::SecretPermissionNotRequested`] if policy tries to expand
    /// beyond the manifest request.
    pub fn grant_requested_secret_read_for_instance(
        &self,
        instance_id: &ExtensionInstanceId,
        component_id: &ComponentId,
        pattern: SecretPathPattern,
    ) -> EngineResult<()> {
        let extension = self
            .extensions
            .get(instance_id)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;

        let Some(component) = extension
            .manifest
            .components
            .iter()
            .find(|component| &component.id == component_id)
        else {
            return Err(EngineError::SecretComponentNotFound {
                extension_id: extension.manifest.id.to_string(),
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
                extension_id: extension.manifest.id.to_string(),
                component_id: component_id.as_str().to_string(),
                pattern: pattern.to_string(),
            });
        }

        self.secrets.grant_read(
            ComponentRef::new(instance_id.clone(), component_id.clone()),
            pattern,
        )?;
        Ok(())
    }

    /// Approves one manifest-requested runtime permission for the default instance.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::grant_requested_runtime_permission_for_instance`].
    pub fn grant_requested_runtime_permission(
        &self,
        extension_id: &ExtensionId,
        component_id: &ComponentId,
        permission: RuntimePermission,
    ) -> EngineResult<()> {
        let instance_id = default_instance_id(extension_id);
        if !self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionNotFound(extension_id.to_string()));
        }
        self.grant_requested_runtime_permission_for_instance(&instance_id, component_id, permission)
    }

    /// Approves one exact runtime permission for a concrete component principal.
    ///
    /// A host cannot grant a capability that the component did not request in its
    /// manifest. The grant survives stop/restart but is removed on unregister.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] for an unknown instance,
    /// [`EngineError::RuntimePermissionComponentNotFound`] for an undeclared component,
    /// [`EngineError::RuntimePermissionNotRequested`] when policy would expand the
    /// manifest request, or [`EngineError::RuntimePermissionUnavailable`] when the
    /// internal policy store cannot be accessed.
    pub fn grant_requested_runtime_permission_for_instance(
        &self,
        instance_id: &ExtensionInstanceId,
        component_id: &ComponentId,
        permission: RuntimePermission,
    ) -> EngineResult<()> {
        let extension = self
            .extensions
            .get(instance_id)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;
        let Some(component) = extension
            .manifest
            .components
            .iter()
            .find(|component| &component.id == component_id)
        else {
            return Err(EngineError::RuntimePermissionComponentNotFound {
                extension_id: extension.manifest.id.to_string(),
                component_id: component_id.to_string(),
            });
        };
        if !component.permissions.runtime.contains(&permission) {
            return Err(EngineError::RuntimePermissionNotRequested {
                extension_id: extension.manifest.id.to_string(),
                component_id: component_id.to_string(),
                permission: permission.to_string(),
            });
        }
        self.runtime_permissions
            .grant(
                ComponentRef::new(instance_id.clone(), component_id.clone()),
                permission,
            )
            .map_err(|()| EngineError::RuntimePermissionUnavailable)
    }

    /// Activates the default runtime instance of one logical extension.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::start_extension_instance`].
    pub fn start_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let instance_id = default_instance_id(extension_id);
        if !self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionNotFound(extension_id.to_string()));
        }
        self.start_extension_instance(&instance_id)
    }

    /// Activates one concrete extension runtime instance.
    ///
    /// Re-activates scope-local contributions if resuming from `Stopped` state.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] if the instance is not
    /// registered, [`EngineError::ActivationPlan`] when its required composition
    /// cannot be satisfied by already active providers, [`EngineError::LifecycleFailed`]
    /// if a component fails to start or a contribution conflicts inside the same
    /// runtime scope, or [`EngineError::StartupRollbackFailed`] when rollback
    /// callbacks fail.
    pub fn start_extension_instance(
        &mut self,
        instance_id: &ExtensionInstanceId,
    ) -> EngineResult<()> {
        let state = self
            .extensions
            .get(instance_id)
            .map(|extension| extension.state)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;
        if state == ExtensionState::Active {
            return Ok(());
        }

        self.plan_extension_activation(std::slice::from_ref(instance_id))?;

        let (extensions, runtime_effects) = (&mut self.extensions, &mut self.runtime_effects);
        let ext = extensions
            .get_mut(instance_id)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;

        if ext.state == ExtensionState::Stopped {
            for contrib in &ext.contributions {
                let key = (ext.scope_id.clone(), contrib.descriptor.id.clone());
                if self.active_contributions.contains(&key) {
                    return Err(EngineError::LifecycleFailed {
                        extension_id: ext.manifest.id.to_string(),
                        component_id: String::from("engine"),
                        reason: format!(
                            "contribution conflict on restart in scope `{}`: `{}`",
                            ext.scope_id, contrib.descriptor.id
                        ),
                    });
                }
            }
            for contrib in &ext.contributions {
                self.active_contributions
                    .insert((ext.scope_id.clone(), contrib.descriptor.id.clone()));
            }
        }

        let logical_id = ext.manifest.id.clone();
        let scope_id = ext.scope_id.clone();
        let concrete_instance_id = ext.instance_id.clone();
        let total_components = ext.components.len();

        for index in 0..total_components {
            let (started, remaining) = ext.components.split_at_mut(index);
            let comp = &mut remaining[0];
            let identity = ComponentIdentity::new(
                logical_id.clone(),
                concrete_instance_id.clone(),
                scope_id.clone(),
                comp.id().clone(),
            );
            let mut ctx = EngineComponentContext::new(
                identity,
                runtime_effects,
                &self.secrets,
                &self.services,
                &self.ui,
                true,
            );

            if let Err(err) = comp.start(&mut ctx) {
                let failed_component_id = comp.id().as_str().to_string();
                let start_reason = err.to_string();
                let mut rollback_failures = Vec::new();

                let identity = ComponentIdentity::new(
                    logical_id.clone(),
                    concrete_instance_id.clone(),
                    scope_id.clone(),
                    comp.id().clone(),
                );
                let mut failed_stop_context = EngineComponentContext::new(
                    identity,
                    runtime_effects,
                    &self.secrets,
                    &self.services,
                    &self.ui,
                    false,
                );
                if let Err(stop_error) = comp.stop(&mut failed_stop_context) {
                    rollback_failures.push(ComponentStopFailure {
                        component_id: failed_component_id.clone(),
                        reason: stop_error.to_string(),
                    });
                }

                for comp_to_stop in started.iter_mut().rev() {
                    let component_id = comp_to_stop.id().as_str().to_string();
                    let identity = ComponentIdentity::new(
                        logical_id.clone(),
                        concrete_instance_id.clone(),
                        scope_id.clone(),
                        comp_to_stop.id().clone(),
                    );
                    let mut stop_ctx = EngineComponentContext::new(
                        identity,
                        runtime_effects,
                        &self.secrets,
                        &self.services,
                        &self.ui,
                        false,
                    );
                    if let Err(stop_error) = comp_to_stop.stop(&mut stop_ctx) {
                        rollback_failures.push(ComponentStopFailure {
                            component_id,
                            reason: stop_error.to_string(),
                        });
                    }
                }

                runtime_effects.revoke_instance(&concrete_instance_id);
                self.execution_targets
                    .revoke_instance(&concrete_instance_id);
                self.ui.set_instance_active(&concrete_instance_id, false)?;
                self.services.set_active(&concrete_instance_id, false);

                if ext.state == ExtensionState::Stopped {
                    for contrib in &ext.contributions {
                        self.active_contributions
                            .remove(&(scope_id.clone(), contrib.descriptor.id.clone()));
                    }
                }

                if rollback_failures.is_empty() {
                    return Err(EngineError::LifecycleFailed {
                        extension_id: logical_id.to_string(),
                        component_id: failed_component_id,
                        reason: start_reason,
                    });
                }

                return Err(EngineError::StartupRollbackFailed {
                    extension_id: logical_id.to_string(),
                    component_id: failed_component_id,
                    start_reason,
                    rollback_failures,
                });
            }
        }

        self.ui.set_instance_active(&concrete_instance_id, true)?;
        ext.state = ExtensionState::Active;
        self.services.set_active(&concrete_instance_id, true);
        Ok(())
    }

    /// Stops the default runtime instance of one logical extension.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::stop_extension_instance`].
    pub fn stop_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let instance_id = default_instance_id(extension_id);
        if !self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionNotFound(extension_id.to_string()));
        }
        self.stop_extension_instance(&instance_id)
    }

    /// Stops one concrete extension runtime instance.
    ///
    /// Host-owned services, UI presentation, contributions, and runtime effects
    /// are revoked for this instance without affecting another instance of the
    /// same logical extension.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] for an unknown instance,
    /// or [`EngineError::StopFailed`] after cleanup if one or more callbacks fail.
    pub fn stop_extension_instance(
        &mut self,
        instance_id: &ExtensionInstanceId,
    ) -> EngineResult<()> {
        let state = self
            .extensions
            .get(instance_id)
            .map(|extension| extension.state)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;
        if state == ExtensionState::Stopped {
            return Ok(());
        }
        self.ensure_execution_target_provider_not_in_use(instance_id)?;

        let (extensions, runtime_effects) = (&mut self.extensions, &mut self.runtime_effects);
        let ext = extensions
            .get_mut(instance_id)
            .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;

        let logical_id = ext.manifest.id.clone();
        let scope_id = ext.scope_id.clone();
        let concrete_instance_id = ext.instance_id.clone();

        self.ui.set_instance_active(&concrete_instance_id, false)?;
        self.services.set_active(&concrete_instance_id, false);

        let mut stop_failures = Vec::new();
        for comp in ext.components.iter_mut().rev() {
            let component_id = comp.id().as_str().to_string();
            let identity = ComponentIdentity::new(
                logical_id.clone(),
                concrete_instance_id.clone(),
                scope_id.clone(),
                comp.id().clone(),
            );
            let mut ctx = EngineComponentContext::new(
                identity,
                runtime_effects,
                &self.secrets,
                &self.services,
                &self.ui,
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
            self.active_contributions
                .remove(&(scope_id.clone(), contrib.descriptor.id.clone()));
        }

        runtime_effects.revoke_instance(&concrete_instance_id);
        self.execution_targets
            .revoke_instance(&concrete_instance_id);
        ext.state = ExtensionState::Stopped;

        if stop_failures.is_empty() {
            Ok(())
        } else {
            Err(EngineError::StopFailed {
                extension_id: logical_id.to_string(),
                failures: stop_failures,
            })
        }
    }

    /// Unregisters the default runtime instance of one logical extension.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::unregister_extension_instance`].
    pub fn unregister_extension(&mut self, extension_id: &ExtensionId) -> EngineResult<()> {
        let instance_id = default_instance_id(extension_id);
        if !self.extensions.contains_key(&instance_id) {
            return Err(EngineError::ExtensionNotFound(extension_id.to_string()));
        }
        self.unregister_extension_instance(&instance_id)
    }

    /// Completely unregisters and unloads one concrete extension runtime instance.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] for an unknown instance
    /// or propagates [`EngineError::StopFailed`] after host cleanup.
    pub fn unregister_extension_instance(
        &mut self,
        instance_id: &ExtensionInstanceId,
    ) -> EngineResult<()> {
        self.ensure_execution_target_provider_not_in_use(instance_id)?;
        if let Some(ext) = self.extensions.get(instance_id) {
            if ext.state == ExtensionState::Active {
                self.stop_extension_instance(instance_id)?;
            }
        } else {
            return Err(EngineError::ExtensionInstanceNotFound(
                instance_id.to_string(),
            ));
        }

        let removed = self.extensions.remove(instance_id);
        if let Some(extension) = removed {
            for contribution in &extension.contributions {
                self.active_contributions.remove(&(
                    extension.scope_id.clone(),
                    contribution.descriptor.id.clone(),
                ));
            }
            for component in &extension.manifest.components {
                self.secrets.revoke_component(&ComponentRef::new(
                    instance_id.clone(),
                    component.id.clone(),
                ));
            }
            for component in &extension.components {
                self.secrets.revoke_component(&ComponentRef::new(
                    instance_id.clone(),
                    component.id.clone(),
                ));
            }
            for component in &extension.manifest.components {
                self.runtime_permissions
                    .revoke_component(&ComponentRef::new(
                        instance_id.clone(),
                        component.id.clone(),
                    ));
            }
            self.services.unregister_instance(instance_id);
            self.ui.unregister_instance(instance_id);
            self.execution_targets.revoke_instance(instance_id);
        }
        Ok(())
    }

    /// Selects the preferred provider for a single-provider contract in the default scope.
    pub fn set_preferred_contract_provider(
        &mut self,
        contract: ContractKey,
        provider: ComponentRef,
    ) {
        self.set_preferred_contract_provider_policy_in_scope(
            default_scope_id(),
            contract,
            provider,
        );
    }

    /// Stores preferred-provider policy for one exact runtime scope.
    ///
    /// Unlike [`Self::set_preferred_contract_provider_in_scope`], this method does
    /// not require the selected provider to be registered yet. Persistent profile
    /// policy can therefore be restored before or independently of provider
    /// availability; resolution reports `PreferredProviderUnavailable` while the
    /// selected component is absent or ineligible.
    pub fn set_preferred_contract_provider_policy_in_scope(
        &mut self,
        scope_id: RuntimeScopeId,
        contract: ContractKey,
        provider: ComponentRef,
    ) {
        self.preferred_contract_providers
            .entry(scope_id.clone())
            .or_default()
            .insert(contract.clone(), provider.clone());
        self.services
            .set_preferred_provider(scope_id, contract, provider);
    }

    /// Selects a currently registered preferred provider inside one exact runtime scope.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] when the provider
    /// instance is unknown, or [`EngineError::InvalidState`] when it belongs to
    /// a different runtime scope.
    pub fn set_preferred_contract_provider_in_scope(
        &mut self,
        scope_id: &RuntimeScopeId,
        contract: ContractKey,
        provider: ComponentRef,
    ) -> EngineResult<()> {
        let extension = self.extensions.get(&provider.instance_id).ok_or_else(|| {
            EngineError::ExtensionInstanceNotFound(provider.instance_id.to_string())
        })?;
        if &extension.scope_id != scope_id {
            return Err(EngineError::InvalidState(format!(
                "provider instance `{}` does not belong to runtime scope `{scope_id}`",
                provider.instance_id
            )));
        }
        self.set_preferred_contract_provider_policy_in_scope(scope_id.clone(), contract, provider);
        Ok(())
    }

    /// Removes a preferred-provider selection in the default runtime scope.
    pub fn clear_preferred_contract_provider(&mut self, contract: &ContractKey) {
        self.clear_preferred_contract_provider_in_scope(&default_scope_id(), contract);
    }

    /// Removes a preferred-provider selection in one exact runtime scope.
    pub fn clear_preferred_contract_provider_in_scope(
        &mut self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) {
        if let Some(providers) = self.preferred_contract_providers.get_mut(scope_id) {
            providers.remove(contract);
            if providers.is_empty() {
                self.preferred_contract_providers.remove(scope_id);
            }
        }
        self.services.clear_preferred_provider(scope_id, contract);
    }

    /// Executes one cooperative runtime pump across every active component.
    ///
    /// Components are visited in stable runtime-instance/component order. The
    /// returned duration is the earliest requested next wake-up; `None` means no
    /// active component currently has scheduled cooperative work.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::RuntimePollFailed`] when an active component cannot
    /// execute its due runtime work.
    pub fn poll_runtime(&mut self) -> EngineResult<Option<Duration>> {
        let mut components = Vec::new();
        for extension in self
            .extensions
            .values()
            .filter(|extension| extension.state == ExtensionState::Active)
        {
            for component in &extension.components {
                components.push((
                    extension.manifest.id.clone(),
                    extension.instance_id.clone(),
                    extension.scope_id.clone(),
                    component.id.clone(),
                    component.handle.clone(),
                ));
            }
        }
        components.sort_by(|left, right| {
            left.1
                .as_str()
                .cmp(right.1.as_str())
                .then_with(|| left.3.as_str().cmp(right.3.as_str()))
        });

        let mut next_wake = None;
        for (extension_id, instance_id, scope_id, component_id, handle) in components {
            let identity = ComponentIdentity::new(
                extension_id.clone(),
                instance_id,
                scope_id,
                component_id.clone(),
            );
            let mut context = EngineComponentContext::new(
                identity,
                &mut self.runtime_effects,
                &self.secrets,
                &self.services,
                &self.ui,
                true,
            );
            let delay = {
                let mut component = handle.lock().map_err(|_| EngineError::RuntimePollFailed {
                    extension_id: extension_id.to_string(),
                    component_id: component_id.to_string(),
                    reason: String::from("component lock was poisoned"),
                })?;
                component.poll_runtime(&mut context).map_err(|error| {
                    EngineError::RuntimePollFailed {
                        extension_id: extension_id.to_string(),
                        component_id: component_id.to_string(),
                        reason: error.to_string(),
                    }
                })?
            };
            if let Some(delay) = delay {
                next_wake = Some(next_wake.map_or(delay, |current: Duration| current.min(delay)));
            }
        }
        Ok(next_wake)
    }

    /// Attaches the selected portable UI Layer for an active extension component.
    ///
    /// # Errors
    ///
    /// Returns a portable UI error when the owner is inactive, another layer is
    /// attached, the protocol is incompatible, or mounted surfaces require unsupported capabilities.
    pub fn attach_ui_layer(
        &self,
        owner: ComponentRef,
        descriptor: UiLayerDescriptor,
    ) -> EngineResult<()> {
        self.ui.attach_layer(owner, descriptor)?;
        Ok(())
    }

    /// Detaches the selected portable UI Layer without deleting feature snapshots.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Ui`] when another component owns the active layer.
    pub fn detach_ui_layer(&self, owner: &ComponentRef) -> EngineResult<()> {
        self.ui.detach_layer(owner)?;
        Ok(())
    }

    /// Returns a host snapshot of every active portable UI surface across scopes.
    pub fn portable_ui_surfaces(&self) -> Vec<UiPresentationSurface> {
        self.ui.presentation_surfaces()
    }

    /// Returns portable surfaces visible to one active UI Layer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Ui`] when the caller is not the active layer in
    /// its runtime scope.
    pub fn portable_ui_surfaces_for_layer(
        &self,
        layer_owner: &ComponentRef,
    ) -> EngineResult<Vec<UiPresentationSurface>> {
        Ok(self.ui.presentation_surfaces_for_layer(layer_owner)?)
    }

    /// Validates and dispatches one semantic input event from the active UI Layer.
    ///
    /// # Errors
    ///
    /// Returns a portable UI validation error for spoofed, cross-scope, or stale
    /// input, [`EngineError::ExtensionInstanceNotFound`] if its runtime owner
    /// disappeared, or [`EngineError::UiActionFailed`] when the owner component
    /// rejects the action.
    pub fn dispatch_ui_action(
        &mut self,
        layer_owner: &ComponentRef,
        event: UiActionEvent,
    ) -> EngineResult<()> {
        let dispatch = self.ui.route_action(layer_owner, event)?;
        let (logical_id, scope_id, component_handle) = {
            let extension = self
                .extensions
                .get(&dispatch.owner.instance_id)
                .ok_or_else(|| {
                    EngineError::ExtensionInstanceNotFound(dispatch.owner.instance_id.to_string())
                })?;
            if extension.state != ExtensionState::Active {
                return Err(EngineError::UiActionFailed {
                    extension_id: extension.manifest.id.to_string(),
                    component_id: dispatch.owner.component_id.to_string(),
                    action_id: dispatch.event.action_id.to_string(),
                    reason: String::from("surface owner extension instance is not active"),
                });
            }
            let component = extension
                .components
                .iter()
                .find(|component| component.id() == &dispatch.owner.component_id)
                .ok_or_else(|| EngineError::UiActionFailed {
                    extension_id: extension.manifest.id.to_string(),
                    component_id: dispatch.owner.component_id.to_string(),
                    action_id: dispatch.event.action_id.to_string(),
                    reason: String::from("surface owner component is not loaded"),
                })?;
            (
                extension.manifest.id.clone(),
                extension.scope_id.clone(),
                component.handle.clone(),
            )
        };

        let owner = dispatch.owner.clone();
        let action_id = dispatch.event.action_id.to_string();
        let identity = ComponentIdentity::new(
            logical_id.clone(),
            owner.instance_id.clone(),
            scope_id,
            owner.component_id.clone(),
        );
        let mut context = EngineComponentContext::new(
            identity,
            &mut self.runtime_effects,
            &self.secrets,
            &self.services,
            &self.ui,
            true,
        );
        let managed = ManagedComponent {
            id: owner.component_id.clone(),
            handle: component_handle,
        };
        managed
            .handle_ui_action(&mut context, &dispatch.event)
            .map_err(|error| EngineError::UiActionFailed {
                extension_id: logical_id.to_string(),
                component_id: owner.component_id.to_string(),
                action_id,
                reason: error.to_string(),
            })
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

    /// Builds a deterministic provider-before-consumer plan for registered instances.
    ///
    /// The input order is used only as a stable tie-breaker between instances that
    /// have no required dependency ordering. Contract resolution remains isolated by
    /// runtime scope, while already active providers may satisfy dependencies without
    /// being included in the requested batch. Registered or stopped instances outside
    /// the requested batch do not participate in provider resolution.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::ExtensionInstanceNotFound`] for an unknown instance,
    /// or [`EngineError::ActivationPlan`] when required composition is unresolved,
    /// an instance is duplicated, or required dependencies contain a cycle.
    pub fn plan_extension_activation(
        &self,
        ordered_instances: &[ExtensionInstanceId],
    ) -> EngineResult<ActivationPlan> {
        let mut scopes = Vec::new();
        let mut seen_scopes = HashSet::new();
        for instance_id in ordered_instances {
            let extension = self
                .extensions
                .get(instance_id)
                .ok_or_else(|| EngineError::ExtensionInstanceNotFound(instance_id.to_string()))?;
            if seen_scopes.insert(extension.scope_id.clone()) {
                scopes.push(extension.scope_id.clone());
            }
        }

        let active_instances: HashSet<_> = self
            .extensions
            .values()
            .filter(|extension| extension.state == ExtensionState::Active)
            .map(|extension| extension.instance_id.clone())
            .collect();
        let mut eligible_instances = active_instances.clone();
        eligible_instances.extend(ordered_instances.iter().cloned());

        let mut composition = CompositionSnapshot::default();
        let mut definition_dependencies = Vec::new();
        for scope_id in scopes {
            let scoped = self.resolve_composition_for_scope(&scope_id, |extension| {
                eligible_instances.contains(&extension.instance_id)
            });
            for binding in scoped.bindings.iter().filter(|binding| binding.required) {
                let is_platform_owned = self
                    .platform_contract_definitions
                    .get(&scope_id)
                    .is_some_and(|definitions| definitions.contains_key(&binding.contract));
                if is_platform_owned {
                    continue;
                }
                if let Some(owner) = self
                    .extensions
                    .values()
                    .filter(|extension| {
                        eligible_instances.contains(&extension.instance_id)
                            && extension.scope_id == scope_id
                    })
                    .flat_map(|extension| extension.contract_definitions.iter())
                    .find(|owned| owned.definition.contract == binding.contract)
                    .map(|owned| owned.owner.clone())
                {
                    definition_dependencies
                        .push((owner.instance_id, binding.consumer.instance_id.clone()));
                }
            }
            composition.bindings.extend(scoped.bindings);
            composition.unresolved.extend(scoped.unresolved);
        }

        Ok(build_activation_plan(
            &composition,
            ordered_instances,
            &active_instances,
            &definition_dependencies,
        )?)
    }

    /// Resolves active contract composition in the default runtime scope.
    pub fn composition_snapshot(&self) -> CompositionSnapshot {
        self.composition_snapshot_for_scope(&default_scope_id())
    }

    /// Resolves active contract composition in one exact runtime scope.
    pub fn composition_snapshot_for_scope(&self, scope_id: &RuntimeScopeId) -> CompositionSnapshot {
        self.resolve_composition_for_scope(scope_id, |extension| {
            extension.state == ExtensionState::Active
        })
    }

    /// Resolves active providers for one contract inside an exact runtime scope.
    ///
    /// Platform-owned definitions participate even though they are not owned by an
    /// extension instance. Extension-owned definitions and providers participate only
    /// while their instances are active. Preferred-provider policy is applied before
    /// returning the selected provider set.
    ///
    /// # Errors
    ///
    /// Returns an [`UnresolvedContractReason`] when the contract is undefined or no
    /// eligible provider can satisfy the current policy.
    pub fn resolve_active_contract_providers_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) -> Result<Vec<ComponentRef>, UnresolvedContractReason> {
        let platform_definition = self
            .platform_contract_definitions
            .get(scope_id)
            .and_then(|definitions| definitions.get(contract));
        let extension_definition = self
            .extensions
            .values()
            .filter(|extension| {
                extension.state == ExtensionState::Active && &extension.scope_id == scope_id
            })
            .flat_map(|extension| extension.contract_definitions.iter())
            .find(|owned| owned.definition.contract == *contract)
            .map(|owned| &owned.definition);
        let definition = platform_definition
            .or(extension_definition)
            .ok_or(UnresolvedContractReason::UndefinedContract)?;

        let providers: Vec<_> = self
            .extensions
            .values()
            .filter(|extension| {
                extension.state == ExtensionState::Active && &extension.scope_id == scope_id
            })
            .flat_map(|extension| extension.contract_providers.iter().cloned())
            .collect();
        let preferred = self
            .preferred_contract_providers
            .get(scope_id)
            .and_then(|providers| providers.get(contract));
        resolve_contract_providers(
            contract,
            definition.resolution,
            &providers,
            preferred,
            &self.secrets,
        )
    }

    /// Returns the extension-owned definition endpoint for one loaded scoped contract.
    ///
    /// Platform-owned definitions intentionally return `None` because they have no
    /// extension lifecycle dependency. The lookup is lifecycle-agnostic and is meant
    /// for bootstrap topology planning rather than runtime availability.
    pub fn contract_definition_owner_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
        contract: &ContractKey,
    ) -> Option<ComponentRef> {
        self.extensions
            .values()
            .filter(|extension| &extension.scope_id == scope_id)
            .flat_map(|extension| extension.contract_definitions.iter())
            .find(|owned| owned.definition.contract == *contract)
            .map(|owned| owned.owner.clone())
    }

    /// Resolves the registered contract topology in the default runtime scope.
    ///
    /// Unlike [`Self::composition_snapshot`], this includes declarations from
    /// registered and stopped instances as well as active ones. It is useful for
    /// diagnostics and topology inspection, but it does not describe currently
    /// callable services or the provider set used by an activation batch.
    /// [`Self::plan_extension_activation`] resolves that batch from active plus
    /// explicitly scheduled instances only.
    pub fn composition_topology_snapshot(&self) -> CompositionSnapshot {
        self.composition_topology_snapshot_for_scope(&default_scope_id())
    }

    /// Resolves the registered contract topology in one exact runtime scope.
    ///
    /// The topology contains every loaded instance in the scope regardless of
    /// lifecycle state. Runtime routing must continue to use
    /// [`Self::composition_snapshot_for_scope`], which is active-only, while
    /// activation planning uses a separately filtered eligible instance set.
    pub fn composition_topology_snapshot_for_scope(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> CompositionSnapshot {
        self.resolve_composition_for_scope(scope_id, |_| true)
    }

    fn resolve_composition_for_scope(
        &self,
        scope_id: &RuntimeScopeId,
        include: impl Fn(&ManagedExtension) -> bool,
    ) -> CompositionSnapshot {
        let mut definitions: Vec<_> = self
            .platform_contract_definitions
            .get(scope_id)
            .into_iter()
            .flat_map(|definitions| definitions.values().cloned())
            .collect();
        let mut providers = Vec::new();
        let mut consumers = Vec::new();

        for extension in self
            .extensions
            .values()
            .filter(|extension| include(extension) && &extension.scope_id == scope_id)
        {
            definitions.extend(
                extension
                    .contract_definitions
                    .iter()
                    .map(|owned| owned.definition.clone()),
            );
            providers.extend(extension.contract_providers.iter().cloned());
            consumers.extend(extension.contract_consumers.iter().cloned());
        }

        let empty_preferences = HashMap::new();
        let preferred = self
            .preferred_contract_providers
            .get(scope_id)
            .unwrap_or(&empty_preferences);
        resolve_contracts(
            &definitions,
            &providers,
            &consumers,
            preferred,
            &self.secrets,
        )
    }

    /// Returns all contribution descriptors owned by registered non-stopped instances.
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

    /// Returns the owner of a contribution in the default runtime scope.
    pub fn active_contribution_owner(
        &self,
        contribution_id: &ContributionId,
    ) -> Option<(&ExtensionId, &ComponentId)> {
        self.active_contribution_owner_in_scope(&default_scope_id(), contribution_id)
    }

    /// Returns the logical extension and component owning a contribution in one scope.
    pub fn active_contribution_owner_in_scope(
        &self,
        scope_id: &RuntimeScopeId,
        contribution_id: &ContributionId,
    ) -> Option<(&ExtensionId, &ComponentId)> {
        self.extensions
            .values()
            .filter(|ext| ext.state != ExtensionState::Stopped && &ext.scope_id == scope_id)
            .find_map(|ext| {
                ext.contributions
                    .iter()
                    .find(|contrib| contrib.descriptor.id == *contribution_id)
                    .map(|contrib| (&ext.manifest.id, &contrib.component_id))
            })
    }

    /// Returns all active runtime effects using legacy logical-extension ownership.
    pub fn active_runtime_effects(
        &self,
    ) -> Vec<(&RuntimeEffectId, &ExtensionId, &ComponentId, &RuntimeEffect)> {
        self.runtime_effects
            .active_effects()
            .into_iter()
            .filter_map(|(effect_id, owner, effect)| {
                self.extensions.get(&owner.instance_id).map(|extension| {
                    (
                        effect_id,
                        &extension.manifest.id,
                        &owner.component_id,
                        effect,
                    )
                })
            })
            .collect()
    }

    /// Returns all active runtime effects with their exact runtime principal.
    pub fn active_runtime_effect_principals(
        &self,
    ) -> Vec<(&RuntimeEffectId, &ComponentRef, &RuntimeEffect)> {
        self.runtime_effects.active_effects()
    }

    /// Returns lifecycle state of the default instance of one logical extension.
    pub fn extension_state(&self, extension_id: &ExtensionId) -> Option<ExtensionState> {
        self.extension_instance_state(&default_instance_id(extension_id))
    }

    /// Returns lifecycle state of one concrete extension runtime instance.
    pub fn extension_instance_state(
        &self,
        instance_id: &ExtensionInstanceId,
    ) -> Option<ExtensionState> {
        self.extensions.get(instance_id).map(|ext| ext.state)
    }
}
