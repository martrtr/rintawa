//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Taverna WASM extensions.

use std::collections::HashSet;
use std::sync::Arc;

use taverna_sdk::{
    context::{ComponentContext, RegistrationContext},
    errors::{ExtensionError, ExtensionResult},
    traits::Component,
    types::ComponentId,
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
use bindings::taverna::engine::host::{Host, LogLevel};

/// Internal host state stored inside the Wasmtime Store context.
pub struct WasmHostState {
    component_id: ComponentId,
    subscriptions: HashSet<String>,
    capabilities: HashSet<String>,
}

impl WasmHostState {
    /// Creates a new host state instance.
    pub fn new(component_id: ComponentId) -> Self {
        Self {
            component_id,
            subscriptions: HashSet::new(),
            capabilities: HashSet::new(),
        }
    }

    /// Returns the active component ID.
    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    /// Returns the active set of subscribed event topics.
    pub fn subscriptions(&self) -> &HashSet<String> {
        &self.subscriptions
    }

    /// Returns the active registered capabilities.
    pub fn capabilities(&self) -> &HashSet<String> {
        &self.capabilities
    }
}

impl Host for WasmHostState {
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

    fn subscribe_event(&mut self, topic: String) {
        debug!(
            plugin = %self.component_id,
            topic = %topic,
            "WASM plugin subscribed to topic"
        );
        self.subscriptions.insert(topic);
    }

    fn register_capability(&mut self, name: String, _schema: String) {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin registered capability"
        );
        self.capabilities.insert(name);
    }

    fn unregister_capability(&mut self, name: String) {
        info!(
            plugin = %self.component_id,
            capability = %name,
            "WASM plugin unregistered capability"
        );
        self.capabilities.remove(&name);
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
        })
    }
}

/// A WASM component implementing the public SDK [`Component`] trait.
pub struct WasmComponent {
    id: ComponentId,
    engine: Engine,
    component: WasmtimeComponent,
    linker: Arc<Linker<WasmHostState>>,
}

impl WasmComponent {
    /// Triggers an incoming event dispatch into the WASM guest instance.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::Message`] if WASM instantiation or execution fails.
    pub fn dispatch_event(&self, topic: &str, payload: &[u8]) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|err| ExtensionError::Message(format!("instantiation error: {err}")))?;

        plugin
            .taverna_engine_guest()
            .call_on_event(&mut store, topic, payload)
            .map_err(|err| ExtensionError::Message(format!("event dispatch error: {err}")))?;

        Ok(())
    }
}

impl Component for WasmComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, _ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|err| ExtensionError::Message(format!("instantiation error: {err}")))?;

        plugin
            .taverna_engine_guest()
            .call_register(&mut store)
            .map_err(|err| ExtensionError::Message(format!("register failed: {err}")))?;

        Ok(())
    }

    fn start(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|err| ExtensionError::Message(format!("instantiation error: {err}")))?;

        plugin
            .taverna_engine_guest()
            .call_start(&mut store)
            .map_err(|err| ExtensionError::Message(format!("start failed: {err}")))?;

        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let plugin = Plugin::instantiate(&mut store, &self.component, &self.linker);

        if let Ok(plugin) = plugin {
            let _ = plugin.taverna_engine_guest().call_stop(&mut store);
        }

        Ok(())
    }
}
