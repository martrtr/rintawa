//! Execution runtimes managed by the Extension Engine.
//!
//! Provides execution hosts for running WASM components and guest plugins safely.

mod wasm;

pub use wasm::{WasmComponent, WasmExecutionBudget, WasmRuntimeEngine};
