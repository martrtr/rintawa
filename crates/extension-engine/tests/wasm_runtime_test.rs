use taverna_extension_engine::{EngineResult, WasmRuntimeEngine};
use taverna_sdk::types::ComponentId;

#[test]
fn test_wasm_runtime_engine_initialization() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new();
    let id = ComponentId::new("test-wasm-component");

    // Minimal valid WebAssembly Component binary representation (empty component header)
    let minimal_wasm_component_bytes: [u8; 8] = [
        0x00, 0x61, 0x73, 0x6d, // \0asm
        0x0d, 0x00, 0x01, 0x00, // Version 1 (Component Model)
    ];

    let result = runtime.load_component_from_bytes(id, &minimal_wasm_component_bytes);
    assert!(result.is_ok());

    Ok(())
}
