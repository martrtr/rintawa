//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Rintawa WASM extensions.

use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};

use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contributions::{ContributionDescriptor, ContributionKind},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    secrets::{SecretAccessError, SecretPath},
    traits::Component,
    types::{ComponentId, ExtensionId, RuntimeEffectId},
};

use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Engine, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Component as WasmtimeComponent, Linker},
};

use crate::{
    errors::{EngineError, EngineResult},
    secrets::SecretManager,
};

/// Host-owned resource limits for one WASM component instance.
///
/// Each lifecycle callback receives a fresh `fuel_per_callback` allowance;
/// unused fuel is discarded before the next callback. Memory and table limits
/// are enforced by Wasmtime for the lifetime of the component store. The
/// defaults are a deliberately conservative local-host baseline, not a public
/// package ABI: a production supervisor may choose a stricter policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmExecutionBudget {
    /// Largest accepted compiled component artifact in bytes.
    pub max_component_bytes: usize,
    /// Maximum size of each guest linear memory in bytes.
    pub max_memory_bytes: usize,
    /// Maximum number of elements in each guest table.
    pub max_table_elements: usize,
    /// Maximum core instances allocated by one component store.
    pub max_instances: usize,
    /// Maximum tables allocated by one component store.
    pub max_tables: usize,
    /// Maximum linear memories allocated by one component store.
    pub max_memories: usize,
    /// Fuel made available before each guest callback and instantiation.
    pub fuel_per_callback: u64,
    /// Maximum byte length of an inbound event topic or payload.
    pub max_host_message_bytes: usize,
}

impl Default for WasmExecutionBudget {
    fn default() -> Self {
        Self {
            max_component_bytes: 32 * 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            max_table_elements: 100_000,
            max_instances: 32,
            max_tables: 16,
            max_memories: 8,
            fuel_per_callback: 10_000_000,
            max_host_message_bytes: 1024 * 1024,
        }
    }
}

impl WasmExecutionBudget {
    fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_memory_bytes)
            .table_elements(self.max_table_elements)
            .instances(self.max_instances)
            .tables(self.max_tables)
            .memories(self.max_memories)
            .build()
    }
}

#[allow(missing_docs)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/engine.wit",
        world: "plugin",
        async: false,
    });
}

use bindings::Plugin;
use bindings::rintawa::engine::{
    host::{Host as HostOperations, LogLevel},
    registration::{Error as RegistrationError, Host as RegistrationHost},
    runtime_effects::{Error as RuntimeEffectError, Host as RuntimeEffectsHost},
    secrets::{Error as SecretError, Host as SecretsHost},
};

/// Internal host state stored inside the Wasmtime Store context.
pub struct WasmHostState {
    component_id: ComponentId,
    extension_id: Option<ExtensionId>,
    registration_scope: Option<WasmRegistrationScope>,
    runtime_effects_active: bool,
    next_effect_handle: u64,
    effect_handles: HashMap<String, ActiveWasmRuntimeEffect>,
    pending_effects: Vec<WasmRuntimeEffectOperation>,
    pending_revocations: HashSet<String>,
    secrets: SecretManager,
    secret_access_active: bool,
    resource_limits: StoreLimits,
}

/// Registrations produced by one guest `register` invocation before the host
/// commits them to the Extension Engine.
struct WasmRegistrationScope {
    contributions: Vec<ContributionDescriptor>,
    capabilities: HashSet<String>,
}

/// A guest request that is committed through the Engine-owned effect registry.
enum WasmRuntimeEffectOperation {
    Subscribe {
        handle: String,
        topic: String,
    },
    Unsubscribe {
        handle: String,
        effect: ActiveWasmRuntimeEffect,
    },
}

/// The Engine effect currently associated with one opaque guest handle.
#[derive(Clone)]
struct ActiveWasmRuntimeEffect {
    effect_id: RuntimeEffectId,
    effect: RuntimeEffect,
}

impl WasmHostState {
    /// Creates a new host state instance.
    pub fn new(component_id: ComponentId) -> Self {
        Self::with_secret_manager(component_id, SecretManager::system())
    }

    /// Creates host state with the Rintawa secret manager shared by the runtime.
    pub fn with_secret_manager(component_id: ComponentId, secrets: SecretManager) -> Self {
        Self::with_secret_manager_and_budget(component_id, secrets, &WasmExecutionBudget::default())
    }

    fn with_secret_manager_and_budget(
        component_id: ComponentId,
        secrets: SecretManager,
        budget: &WasmExecutionBudget,
    ) -> Self {
        Self {
            component_id,
            extension_id: None,
            registration_scope: None,
            runtime_effects_active: false,
            next_effect_handle: 0,
            effect_handles: HashMap::new(),
            pending_effects: Vec::new(),
            pending_revocations: HashSet::new(),
            secrets,
            secret_access_active: false,
            resource_limits: budget.store_limits(),
        }
    }

    /// Returns the active component ID.
    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn begin_registration(&mut self, extension_id: ExtensionId) -> ExtensionResult<()> {
        if self.registration_scope.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM component registration is already in progress",
            )));
        }

        if self.extension_id.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM component has already completed registration",
            )));
        }

        self.registration_scope = Some(WasmRegistrationScope {
            contributions: Vec::new(),
            capabilities: HashSet::new(),
        });
        self.extension_id = Some(extension_id);
        Ok(())
    }

    fn finish_registration(&mut self) -> ExtensionResult<Vec<ContributionDescriptor>> {
        let scope = self.registration_scope.take().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM component registration is not active"))
        })?;

        Ok(scope.contributions)
    }

    fn cancel_registration(&mut self) {
        self.registration_scope = None;
        self.extension_id = None;
    }

    fn validate_execution_owner(&self, ctx: &dyn ComponentContext) -> ExtensionResult<()> {
        let Some(extension_id) = &self.extension_id else {
            return Err(ExtensionError::Message(String::from(
                "WASM component has not completed registration",
            )));
        };

        if ctx.extension_id() != extension_id || ctx.component_id() != &self.component_id {
            return Err(ExtensionError::Message(String::from(
                "WASM component received a context for a different owner",
            )));
        }

        Ok(())
    }

    fn discard_guest_execution(&mut self) {
        self.registration_scope = None;
        self.runtime_effects_active = false;
        self.pending_effects.clear();
        self.pending_revocations.clear();
        self.secret_access_active = false;
    }

    fn queue_capability(&mut self, name: String) -> Result<(), RegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Rejecting WASM capability registration outside guest register callback"
            );
            return Err(RegistrationError::RegistrationNotActive);
        };

        if !scope.capabilities.insert(name.clone()) {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Ignoring duplicate WASM capability registration"
            );
            return Ok(());
        }

        scope.contributions.push(ContributionDescriptor::new(
            name,
            ContributionKind::capability(),
        ));
        Ok(())
    }

    fn remove_queued_capability(&mut self, name: &str) -> Result<(), RegistrationError> {
        let Some(scope) = self.registration_scope.as_mut() else {
            warn!(
                plugin = %self.component_id,
                capability = %name,
                "Rejecting WASM capability removal outside guest register callback"
            );
            return Err(RegistrationError::RegistrationNotActive);
        };

        if scope.capabilities.remove(name) {
            scope.contributions.retain(|contribution| {
                contribution.kind != ContributionKind::capability()
                    || contribution.id.as_str() != name
            });
        }
        Ok(())
    }

    fn subscribe_event(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }
        if topic.trim().is_empty() {
            return Err(RuntimeEffectError::InvalidTopic);
        }

        let handle = format!("effect-{}", self.next_effect_handle);
        self.next_effect_handle = self
            .next_effect_handle
            .checked_add(1)
            .ok_or(RuntimeEffectError::RuntimeNotActive)?;
        self.pending_effects
            .push(WasmRuntimeEffectOperation::Subscribe {
                handle: handle.clone(),
                topic,
            });
        Ok(handle)
    }

    fn unsubscribe_event(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        if !self.runtime_effects_active {
            return Err(RuntimeEffectError::RuntimeNotActive);
        }

        if let Some(index) = self.pending_effects.iter().position(|operation| {
            matches!(operation, WasmRuntimeEffectOperation::Subscribe { handle: pending, .. } if pending == &handle)
        }) {
            self.pending_effects.remove(index);
            return Ok(());
        }

        if self.pending_revocations.contains(&handle) {
            return Err(RuntimeEffectError::UnknownEffect);
        }

        let Some(effect) = self.effect_handles.get(&handle).cloned() else {
            return Err(RuntimeEffectError::UnknownEffect);
        };
        self.pending_revocations.insert(handle.clone());
        self.pending_effects
            .push(WasmRuntimeEffectOperation::Unsubscribe { handle, effect });
        Ok(())
    }

    /// Opens the only guest execution scope that can create effects or read secrets.
    fn begin_guest_execution(&mut self) {
        self.runtime_effects_active = true;
        self.secret_access_active = true;
    }

    /// Closes guest access before committing its queued runtime effects.
    fn finish_guest_execution(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.runtime_effects_active = false;
        self.secret_access_active = false;

        self.commit_runtime_effects(ctx)
    }

    /// Commits reversible effects requested by the just-completed guest callback.
    fn commit_runtime_effects(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let mut registered_handles = Vec::new();
        let mut revoked_effects = Vec::new();
        for operation in std::mem::take(&mut self.pending_effects) {
            let operation_result = match operation {
                WasmRuntimeEffectOperation::Subscribe { handle, topic } => {
                    let effect = RuntimeEffect::event_subscription(topic);
                    ctx.register_runtime_effect(effect.clone())
                        .map(|effect_id| {
                            self.effect_handles.insert(
                                handle.clone(),
                                ActiveWasmRuntimeEffect { effect_id, effect },
                            );
                            registered_handles.push(handle);
                        })
                }
                WasmRuntimeEffectOperation::Unsubscribe { handle, effect } => {
                    ctx.revoke_runtime_effect(&effect.effect_id).map(|()| {
                        self.effect_handles.remove(&handle);
                        self.pending_revocations.remove(&handle);
                        revoked_effects.push((handle, effect));
                    })
                }
            };

            if let Err(error) = operation_result {
                if let Err(rollback_error) =
                    self.rollback_runtime_effects(ctx, registered_handles, revoked_effects)
                {
                    return Err(ExtensionError::RuntimeEffectRollbackFailed {
                        operation: error.to_string(),
                        rollback: rollback_error.to_string(),
                    });
                }
                return Err(error);
            }
        }

        Ok(())
    }

    fn rollback_runtime_effects(
        &mut self,
        ctx: &mut dyn ComponentContext,
        registered_handles: Vec<String>,
        revoked_effects: Vec<(String, ActiveWasmRuntimeEffect)>,
    ) -> ExtensionResult<()> {
        let mut rollback_errors = Vec::new();

        for handle in registered_handles.iter().rev() {
            if let Some(effect) = self.effect_handles.remove(handle)
                && let Err(error) = ctx.revoke_runtime_effect(&effect.effect_id)
            {
                self.effect_handles.insert(handle.clone(), effect);
                rollback_errors.push(error.to_string());
            }
        }

        for (handle, effect) in revoked_effects.into_iter().rev() {
            match ctx.register_runtime_effect(effect.effect.clone()) {
                Ok(effect_id) => {
                    self.effect_handles.insert(
                        handle.clone(),
                        ActiveWasmRuntimeEffect {
                            effect_id,
                            effect: effect.effect,
                        },
                    );
                }
                Err(error) => rollback_errors.push(error.to_string()),
            }
        }

        self.pending_revocations.clear();

        if rollback_errors.is_empty() {
            Ok(())
        } else {
            Err(ExtensionError::Message(rollback_errors.join("; ")))
        }
    }

    fn read_secret(&self, path: String) -> Result<String, SecretError> {
        if !self.secret_access_active {
            return Err(SecretError::AccessNotActive);
        }

        let extension_id = self
            .extension_id
            .as_ref()
            .ok_or(SecretError::AccessNotActive)?;
        let path = SecretPath::parse(path).map_err(|_| SecretError::InvalidPath)?;

        self.secrets
            .read_for_component(extension_id, &self.component_id, &path)
            .map(|value| value.expose_secret().to_string())
            .map_err(|error| match error {
                SecretAccessError::InvalidPath => SecretError::InvalidPath,
                SecretAccessError::AccessDenied => SecretError::AccessDenied,
                SecretAccessError::NotFound => SecretError::NotFound,
                SecretAccessError::Unavailable => SecretError::Unavailable,
            })
    }
}

impl HostOperations for WasmHostState {
    fn log(&mut self, level: LogLevel, message: String) {
        let id = &self.component_id;
        match level {
            LogLevel::Trace => trace!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Debug => debug!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Info => info!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Warn => warn!(target: "wasm_plugin", plugin = %id, "{message}"),
            LogLevel::Error => error!(target: "wasm_plugin", plugin = %id, "{message}"),
        }
    }

    fn publish_event(&mut self, topic: String, _payload: Vec<u8>) {
        debug!(
            plugin = %self.component_id,
            topic = %topic,
            "WASM plugin published event"
        );
    }
}

impl RuntimeEffectsHost for WasmHostState {
    fn subscribe_event(&mut self, topic: String) -> Result<String, RuntimeEffectError> {
        debug!(
            plugin = %self.component_id,
            topic = %topic,
            "WASM plugin subscribed to topic"
        );
        self.subscribe_event(topic)
    }

    fn unsubscribe_event(&mut self, handle: String) -> Result<(), RuntimeEffectError> {
        self.unsubscribe_event(handle)
    }
}

impl SecretsHost for WasmHostState {
    fn read(&mut self, path: String) -> Result<String, SecretError> {
        self.read_secret(path)
    }
}

impl RegistrationHost for WasmHostState {
    fn register_capability(
        &mut self,
        name: String,
        _schema: String,
    ) -> Result<(), RegistrationError> {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin registered capability"
        );
        self.queue_capability(name)
    }

    fn unregister_capability(&mut self, name: String) -> Result<(), RegistrationError> {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin unregistered capability"
        );
        self.remove_queued_capability(&name)
    }
}

/// The engine manager for compiled WebAssembly components.
#[derive(Clone)]
pub struct WasmRuntimeEngine {
    engine: Engine,
    secrets: SecretManager,
    budget: WasmExecutionBudget,
}

impl WasmRuntimeEngine {
    /// Creates a runtime using the system secret manager and default resource budget.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn new() -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(
            SecretManager::system(),
            WasmExecutionBudget::default(),
        )
    }

    /// Creates a runtime that shares the supplied Rintawa secret policy.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_secret_manager(secrets: SecretManager) -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(secrets, WasmExecutionBudget::default())
    }

    /// Creates a runtime using the system secret manager and the supplied budget.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_execution_budget(budget: WasmExecutionBudget) -> EngineResult<Self> {
        Self::with_secret_manager_and_budget(SecretManager::system(), budget)
    }

    /// Creates a runtime with host-owned secret and resource policies.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`] when Wasmtime cannot create the
    /// configured Component Model engine.
    pub fn with_secret_manager_and_budget(
        secrets: SecretManager,
        budget: WasmExecutionBudget,
    ) -> EngineResult<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(false);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;

        Ok(Self {
            engine,
            secrets,
            budget,
        })
    }

    /// Loads and compiles a WASM component from binary bytes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmArtifactTooLarge`] when `bytes` exceed the
    /// configured artifact budget, or [`EngineError::WasmRuntime`] if
    /// compilation or linker binding fails.
    pub fn load_component_from_bytes(
        &self,
        id: ComponentId,
        bytes: &[u8],
    ) -> EngineResult<WasmComponent> {
        self.ensure_component_size(&id, bytes.len())?;
        let component = WasmtimeComponent::new(&self.engine, bytes)?;
        let mut linker = Linker::new(&self.engine);

        Plugin::add_to_linker(&mut linker, |state: &mut WasmHostState| state)?;

        Ok(WasmComponent {
            id,
            engine: self.engine.clone(),
            component,
            linker: Arc::new(linker),
            secrets: self.secrets.clone(),
            budget: self.budget.clone(),
            instance: None,
        })
    }

    /// Reads, bounds, and compiles a WASM component artifact from disk.
    ///
    /// The method intentionally reads no more than one byte above the configured
    /// limit, so a package artifact cannot make the loader allocate its full
    /// untrusted file size before the limit is checked.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmArtifactTooLarge`] when the artifact exceeds
    /// the configured byte limit, [`EngineError::Io`] when it cannot be read,
    /// or [`EngineError::WasmRuntime`] when Wasmtime cannot compile it.
    pub fn load_component_from_file(
        &self,
        id: ComponentId,
        path: &Path,
    ) -> EngineResult<WasmComponent> {
        let read_limit = u64::try_from(self.budget.max_component_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::new();
        File::open(path)?.take(read_limit).read_to_end(&mut bytes)?;
        self.load_component_from_bytes(id, &bytes)
    }

    fn ensure_component_size(&self, id: &ComponentId, observed_bytes: usize) -> EngineResult<()> {
        if observed_bytes > self.budget.max_component_bytes {
            return Err(EngineError::WasmArtifactTooLarge {
                component_id: id.as_str().to_string(),
                observed_bytes,
                maximum_bytes: self.budget.max_component_bytes,
            });
        }

        Ok(())
    }
}

/// A WASM component implementing the public SDK [`Component`] trait.
pub struct WasmComponent {
    id: ComponentId,
    engine: Engine,
    component: WasmtimeComponent,
    linker: Arc<Linker<WasmHostState>>,
    secrets: SecretManager,
    budget: WasmExecutionBudget,
    instance: Option<WasmInstance>,
}

/// A live guest instance and its host state for one component lifecycle.
///
/// The store owns guest linear memory and globals, so it must live as long as
/// the guest instance. Re-instantiating per lifecycle callback would reset
/// guest state and invalidate component-owned runtime resources.
struct WasmInstance {
    store: Store<WasmHostState>,
    plugin: Plugin,
}

impl WasmComponent {
    /// Instantiates the guest component once and returns its live instance.
    ///
    /// # Errors
    ///
    /// Returns an error when Wasmtime cannot instantiate the component or when
    /// it does not satisfy the generated WIT world contract.
    fn instance_mut(&mut self) -> ExtensionResult<&mut WasmInstance> {
        if self.instance.is_none() {
            let host_state = WasmHostState::with_secret_manager_and_budget(
                self.id.clone(),
                self.secrets.clone(),
                &self.budget,
            );
            let mut store = Store::new(&self.engine, host_state);
            store.limiter(|state| &mut state.resource_limits);
            Self::set_callback_fuel(&mut store, &self.budget, "instantiate")?;
            let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker)
                .map_err(|err| Self::execution_error("instantiate", err))?;

            self.instance = Some(WasmInstance { store, plugin });
        }

        self.instance.as_mut().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM component instance was not initialized"))
        })
    }

    fn set_callback_fuel(
        store: &mut Store<WasmHostState>,
        budget: &WasmExecutionBudget,
        operation: &'static str,
    ) -> ExtensionResult<()> {
        store.set_fuel(budget.fuel_per_callback).map_err(|error| {
            ExtensionError::Message(format!("could not set WASM fuel for {operation}: {error}"))
        })
    }

    fn execution_error(operation: &'static str, error: wasmtime::Error) -> ExtensionError {
        if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
            return ExtensionError::ExecutionBudgetExceeded {
                resource: "fuel",
                operation,
            };
        }

        ExtensionError::Message(format!("{operation} failed: {error}"))
    }

    fn validate_inbound_message(
        &self,
        operation: &'static str,
        message_bytes: usize,
    ) -> ExtensionResult<()> {
        if message_bytes > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation,
                actual_bytes: message_bytes,
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }

        Ok(())
    }

    /// Triggers an incoming event dispatch into the WASM guest instance.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::Message`] if WASM instantiation or execution fails.
    pub fn dispatch_event(
        &mut self,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        self.validate_inbound_message("event topic", topic.len())?;
        self.validate_inbound_message("event payload", payload.len())?;
        let budget = self.budget.clone();
        let instance = self.instance_mut()?;

        instance.store.data().validate_execution_owner(ctx)?;
        Self::set_callback_fuel(&mut instance.store, &budget, "event dispatch")?;

        instance.store.data_mut().begin_guest_execution();

        let dispatch_result = instance
            .plugin
            .rintawa_engine_guest()
            .call_on_event(&mut instance.store, topic, payload)
            .map_err(|err| Self::execution_error("event dispatch", err));

        if let Err(error) = dispatch_result {
            instance.store.data_mut().discard_guest_execution();
            instance.store.data_mut().effect_handles.clear();
            if let Err(cleanup_error) = ctx.revoke_all_runtime_effects() {
                return Err(ExtensionError::RuntimeEffectCleanupFailed {
                    operation: error.to_string(),
                    cleanup: cleanup_error.to_string(),
                });
            }
            return Err(error);
        }

        if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
            instance.store.data_mut().discard_guest_execution();
            return Err(error);
        }

        Ok(())
    }
}

impl Component for WasmComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        let contributions = {
            let instance = self.instance_mut()?;
            Self::set_callback_fuel(&mut instance.store, &budget, "register")?;
            instance
                .store
                .data_mut()
                .begin_registration(ctx.extension_id().clone())?;

            let registration_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_register(&mut instance.store);

            if let Err(err) = registration_result {
                instance.store.data_mut().cancel_registration();
                return Err(Self::execution_error("register", err));
            }

            instance.store.data_mut().finish_registration()?
        };

        for contribution in contributions {
            ctx.register(contribution)?;
        }

        Ok(())
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        let instance = self.instance_mut()?;

        instance.store.data().validate_execution_owner(ctx)?;
        Self::set_callback_fuel(&mut instance.store, &budget, "start")?;

        instance.store.data_mut().begin_guest_execution();

        let start_result = instance
            .plugin
            .rintawa_engine_guest()
            .call_start(&mut instance.store)
            .map_err(|err| Self::execution_error("start", err));

        if let Err(error) = start_result {
            instance.store.data_mut().discard_guest_execution();
            return Err(error);
        }

        if let Err(error) = instance.store.data_mut().finish_guest_execution(ctx) {
            instance.store.data_mut().discard_guest_execution();
            return Err(error);
        }

        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let budget = self.budget.clone();
        if let Some(instance) = self.instance.as_mut() {
            Self::set_callback_fuel(&mut instance.store, &budget, "stop")?;
            let stop_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_stop(&mut instance.store)
                .map_err(|err| Self::execution_error("stop", err));
            instance.store.data_mut().discard_guest_execution();
            instance.store.data_mut().effect_handles.clear();
            stop_result?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemorySecretVault;
    use rintawa_sdk::{
        api::{LogLevel, LoggerApi},
        secrets::{SecretPath, SecretPathPattern, SecretValue},
        types::ExtensionId,
    };
    use std::sync::Arc;

    struct TestLogger;

    impl LoggerApi for TestLogger {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    struct TestRuntimeContext {
        extension_id: ExtensionId,
        component_id: ComponentId,
        logger: TestLogger,
        effects: HashMap<RuntimeEffectId, RuntimeEffect>,
        next_effect_sequence: u64,
        registration_attempts: u64,
        failing_registration_attempts: HashSet<u64>,
    }

    impl TestRuntimeContext {
        fn new() -> Self {
            Self {
                extension_id: ExtensionId::new("rintawa.chat"),
                component_id: ComponentId::new("chat-runtime"),
                logger: TestLogger,
                effects: HashMap::new(),
                next_effect_sequence: 0,
                registration_attempts: 0,
                failing_registration_attempts: HashSet::new(),
            }
        }

        fn fail_registration_on(&mut self, attempt: u64) {
            self.failing_registration_attempts.insert(attempt);
        }
    }

    impl ComponentContext for TestRuntimeContext {
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
            self.registration_attempts += 1;
            if self
                .failing_registration_attempts
                .contains(&self.registration_attempts)
            {
                return Err(ExtensionError::Message(String::from(
                    "simulated effect registration failure",
                )));
            }
            let effect_id = RuntimeEffectId::new(format!("effect-{}", self.next_effect_sequence));
            self.next_effect_sequence += 1;
            self.effects.insert(effect_id.clone(), effect);
            Ok(effect_id)
        }

        fn revoke_runtime_effect(&mut self, effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
            self.effects.remove(effect_id);
            Ok(())
        }
    }

    #[test]
    fn test_should_commit_wasm_capability_contributions_only_after_registration_finishes() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        state
            .begin_registration(ExtensionId::new("rintawa.chat"))
            .unwrap();

        RegistrationHost::register_capability(
            &mut state,
            String::from("rintawa.ai"),
            String::from("{}"),
        )
        .unwrap();

        let contributions = state.finish_registration().unwrap();

        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].id.as_str(), "rintawa.ai");
        assert_eq!(contributions[0].kind, ContributionKind::capability());
    }

    #[test]
    fn test_should_install_and_revoke_wasm_runtime_effects_with_handles() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();

        state.begin_guest_execution();
        let handle =
            RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message"))
                .unwrap();
        state.finish_guest_execution(&mut context).unwrap();
        assert_eq!(context.effects.len(), 1);

        state.begin_guest_execution();
        RuntimeEffectsHost::unsubscribe_event(&mut state, handle).unwrap();
        state.finish_guest_execution(&mut context).unwrap();
        assert!(context.effects.is_empty());
    }

    #[test]
    fn test_should_reject_wasm_effect_or_registration_outside_its_lifecycle_scope() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));

        assert!(matches!(
            RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message")),
            Err(RuntimeEffectError::RuntimeNotActive)
        ));
        assert!(matches!(
            RegistrationHost::register_capability(
                &mut state,
                String::from("rintawa.ai"),
                String::from("{}"),
            ),
            Err(RegistrationError::RegistrationNotActive)
        ));
    }

    #[test]
    fn test_should_read_only_host_granted_wasm_secret_during_and_after_execution() {
        let manager = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let extension_id = ExtensionId::new("official_ai");
        let component_id = ComponentId::new("provider");
        let allowed_path = SecretPath::parse("ai.api_keys.openai").unwrap();

        manager
            .store(&allowed_path, &SecretValue::new("test-key"))
            .unwrap();
        manager
            .grant_read(
                extension_id.clone(),
                component_id.clone(),
                SecretPathPattern::parse("ai.api_keys.*").unwrap(),
            )
            .unwrap();

        let mut state = WasmHostState::with_secret_manager(component_id, manager);
        state.begin_registration(extension_id).unwrap();
        state.finish_registration().unwrap();

        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Err(SecretError::AccessNotActive)
        ));

        state.begin_guest_execution();
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Ok(value) if value == "test-key"
        ));
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys_backup.openai")),
            Err(SecretError::AccessDenied)
        ));

        let mut context = TestRuntimeContext::new();
        state.finish_guest_execution(&mut context).unwrap();
        assert!(matches!(
            SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
            Err(SecretError::AccessNotActive)
        ));
    }

    #[test]
    fn test_should_roll_back_effects_when_a_callback_batch_fails() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();
        context.fail_registration_on(2);

        state.begin_guest_execution();
        state
            .subscribe_event(String::from("dialogue.first"))
            .unwrap();
        state
            .subscribe_event(String::from("dialogue.second"))
            .unwrap();

        assert!(state.finish_guest_execution(&mut context).is_err());
        assert!(context.effects.is_empty());
    }

    #[test]
    fn test_should_report_a_failed_rollback_without_retaining_a_stale_handle() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();

        state.begin_guest_execution();
        let active_handle = state
            .subscribe_event(String::from("dialogue.active"))
            .unwrap();
        state.finish_guest_execution(&mut context).unwrap();

        context.fail_registration_on(2);
        context.fail_registration_on(3);
        state.begin_guest_execution();
        state.unsubscribe_event(active_handle.clone()).unwrap();
        state.subscribe_event(String::from("dialogue.new")).unwrap();

        assert!(matches!(
            state.finish_guest_execution(&mut context),
            Err(ExtensionError::RuntimeEffectRollbackFailed { .. })
        ));
        assert!(context.effects.is_empty());

        state.begin_guest_execution();
        assert!(matches!(
            state.unsubscribe_event(active_handle),
            Err(RuntimeEffectError::UnknownEffect)
        ));
    }
}
