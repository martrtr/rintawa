//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Rintawa WASM extensions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contributions::{ContributionDescriptor, ContributionKind},
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    traits::Component,
    types::{ComponentId, ExtensionId, RuntimeEffectId},
};

use tracing::{debug, error, info, trace, warn};
use wasmtime::{
    Engine, Store,
    component::{Component as WasmtimeComponent, Linker},
};

use crate::errors::EngineResult;

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
        Self {
            component_id,
            extension_id: None,
            registration_scope: None,
            runtime_effects_active: false,
            next_effect_handle: 0,
            effect_handles: HashMap::new(),
            pending_effects: Vec::new(),
            pending_revocations: HashSet::new(),
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

    fn discard_pending_runtime_effects(&mut self) {
        self.registration_scope = None;
        self.runtime_effects_active = false;
        self.pending_effects.clear();
        self.pending_revocations.clear();
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

    fn begin_runtime_effects(&mut self) {
        self.runtime_effects_active = true;
    }

    fn finish_runtime_effects(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.runtime_effects_active = false;

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
}

impl Default for WasmRuntimeEngine {
    fn default() -> Self {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(false);

        let engine = Engine::new(&config).unwrap_or_else(|_| Engine::default());

        Self { engine }
    }
}

impl WasmRuntimeEngine {
    /// Creates a new [`WasmRuntimeEngine`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads and compiles a WASM component from binary bytes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::WasmRuntime`](crate::errors::EngineError::WasmRuntime)
    /// if compilation or linker binding fails.
    pub fn load_component_from_bytes(
        &self,
        id: ComponentId,
        bytes: &[u8],
    ) -> EngineResult<WasmComponent> {
        let component = WasmtimeComponent::new(&self.engine, bytes)?;
        let mut linker = Linker::new(&self.engine);

        Plugin::add_to_linker(&mut linker, |state: &mut WasmHostState| state)?;

        Ok(WasmComponent {
            id,
            engine: self.engine.clone(),
            component,
            linker: Arc::new(linker),
            instance: None,
        })
    }
}

/// A WASM component implementing the public SDK [`Component`] trait.
pub struct WasmComponent {
    id: ComponentId,
    engine: Engine,
    component: WasmtimeComponent,
    linker: Arc<Linker<WasmHostState>>,
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
            let host_state = WasmHostState::new(self.id.clone());
            let mut store = Store::new(&self.engine, host_state);
            let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker)
                .map_err(|err| ExtensionError::Message(format!("instantiation error: {err}")))?;

            self.instance = Some(WasmInstance { store, plugin });
        }

        self.instance.as_mut().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM component instance was not initialized"))
        })
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
        let instance = self.instance_mut()?;

        instance.store.data().validate_execution_owner(ctx)?;

        instance.store.data_mut().begin_runtime_effects();

        let dispatch_result = instance
            .plugin
            .rintawa_engine_guest()
            .call_on_event(&mut instance.store, topic, payload)
            .map_err(|err| ExtensionError::Message(format!("event dispatch error: {err}")));

        if let Err(error) = dispatch_result {
            instance.store.data_mut().discard_pending_runtime_effects();
            instance.store.data_mut().effect_handles.clear();
            if let Err(cleanup_error) = ctx.revoke_all_runtime_effects() {
                return Err(ExtensionError::RuntimeEffectCleanupFailed {
                    operation: error.to_string(),
                    cleanup: cleanup_error.to_string(),
                });
            }
            return Err(error);
        }

        if let Err(error) = instance.store.data_mut().finish_runtime_effects(ctx) {
            instance.store.data_mut().discard_pending_runtime_effects();
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
        let contributions = {
            let instance = self.instance_mut()?;
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
                return Err(ExtensionError::Message(format!("register failed: {err}")));
            }

            instance.store.data_mut().finish_registration()?
        };

        for contribution in contributions {
            ctx.register(contribution)?;
        }

        Ok(())
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let instance = self.instance_mut()?;

        instance.store.data().validate_execution_owner(ctx)?;

        instance.store.data_mut().begin_runtime_effects();

        let start_result = instance
            .plugin
            .rintawa_engine_guest()
            .call_start(&mut instance.store)
            .map_err(|err| ExtensionError::Message(format!("start failed: {err}")));

        if let Err(error) = start_result {
            instance.store.data_mut().discard_pending_runtime_effects();
            return Err(error);
        }

        if let Err(error) = instance.store.data_mut().finish_runtime_effects(ctx) {
            instance.store.data_mut().discard_pending_runtime_effects();
            return Err(error);
        }

        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        if let Some(instance) = self.instance.as_mut() {
            let stop_result = instance
                .plugin
                .rintawa_engine_guest()
                .call_stop(&mut instance.store)
                .map_err(|err| ExtensionError::Message(format!("stop failed: {err}")));
            instance.store.data_mut().discard_pending_runtime_effects();
            instance.store.data_mut().effect_handles.clear();
            stop_result?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::{
        api::{LogLevel, LoggerApi},
        types::ExtensionId,
    };

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

        state.begin_runtime_effects();
        let handle =
            RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message"))
                .unwrap();
        state.finish_runtime_effects(&mut context).unwrap();
        assert_eq!(context.effects.len(), 1);

        state.begin_runtime_effects();
        RuntimeEffectsHost::unsubscribe_event(&mut state, handle).unwrap();
        state.finish_runtime_effects(&mut context).unwrap();
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
    fn test_should_roll_back_effects_when_a_callback_batch_fails() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();
        context.fail_registration_on(2);

        state.begin_runtime_effects();
        state
            .subscribe_event(String::from("dialogue.first"))
            .unwrap();
        state
            .subscribe_event(String::from("dialogue.second"))
            .unwrap();

        assert!(state.finish_runtime_effects(&mut context).is_err());
        assert!(context.effects.is_empty());
    }

    #[test]
    fn test_should_report_a_failed_rollback_without_retaining_a_stale_handle() {
        let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
        let mut context = TestRuntimeContext::new();

        state.begin_runtime_effects();
        let active_handle = state
            .subscribe_event(String::from("dialogue.active"))
            .unwrap();
        state.finish_runtime_effects(&mut context).unwrap();

        context.fail_registration_on(2);
        context.fail_registration_on(3);
        state.begin_runtime_effects();
        state.unsubscribe_event(active_handle.clone()).unwrap();
        state.subscribe_event(String::from("dialogue.new")).unwrap();

        assert!(matches!(
            state.finish_runtime_effects(&mut context),
            Err(ExtensionError::RuntimeEffectRollbackFailed { .. })
        ));
        assert!(context.effects.is_empty());

        state.begin_runtime_effects();
        assert!(matches!(
            state.unsubscribe_event(active_handle),
            Err(RuntimeEffectError::UnknownEffect)
        ));
    }
}
