//! Execution runtimes managed by the Extension Engine.
//!
//! Provides execution hosts for running WASM components and guest plugins safely.

mod wasm;

pub(crate) use wasm::AsyncObserverRegistry;
pub use wasm::{WasmComponent, WasmExecutionBudget, WasmRuntimeEngine};
