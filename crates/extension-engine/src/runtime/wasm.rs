//! WASM Runtime execution driver built on Wasmtime.
//!
//! Provides sandboxed Component Model lifecycle execution for Taverna WASM extensions.

use std::sync::Arc;

use taverna_sdk::{
    context::{ComponentContext, RegistrationContext},
    errors::{ExtensionError, ExtensionResult},
    traits::Component,
    types::ComponentId,
};
use wasmtime::{
    Engine, Store,
    component::{Component as WasmtimeComponent, Linker},
};

use crate::errors::EngineResult;

/// Internal host state stored inside the Wasmtime Store context.
pub struct WasmHostState {
    component_id: ComponentId,
}

impl WasmHostState {
    /// Creates a new host state instance.
    pub fn new(component_id: ComponentId) -> Self {
        Self { component_id }
    }

    /// Returns the active component ID.
    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
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
    /// if compilation fails.
    pub fn load_component_from_bytes(
        &self,
        id: ComponentId,
        bytes: &[u8],
    ) -> EngineResult<WasmComponent> {
        let component = WasmtimeComponent::new(&self.engine, bytes)?;
        let linker = Linker::new(&self.engine);

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

impl Component for WasmComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, _ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let instance = self
            .linker
            .instantiate(&mut store, &self.component)
            .map_err(|err| ExtensionError::Message(format!("failed to instantiate WASM: {err}")))?;

        if let Some(func) = instance.get_func(&mut store, "register") {
            func.call(&mut store, &[], &mut [])
                .map_err(|err| ExtensionError::Message(format!("WASM register failed: {err}")))?;
        }

        Ok(())
    }

    fn start(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        let instance = self
            .linker
            .instantiate(&mut store, &self.component)
            .map_err(|err| ExtensionError::Message(format!("failed to instantiate WASM: {err}")))?;

        if let Some(func) = instance.get_func(&mut store, "start") {
            func.call(&mut store, &[], &mut [])
                .map_err(|err| ExtensionError::Message(format!("WASM start failed: {err}")))?;
        }

        Ok(())
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let host_state = WasmHostState::new(self.id.clone());
        let mut store = Store::new(&self.engine, host_state);

        if let Ok(instance) = self.linker.instantiate(&mut store, &self.component) {
            let func = instance.get_func(&mut store, "stop");
            if let Some(func) = func {
                let _ = func.call(&mut store, &[], &mut []);
            }
        }

        Ok(())
    }
}
