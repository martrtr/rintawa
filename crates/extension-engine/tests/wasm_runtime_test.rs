use taverna_extension_engine::{EngineResult, WasmRuntimeEngine};
use taverna_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contributions::ContributionDescriptor,
    errors::ExtensionResult,
    traits::Component,
    types::{ComponentId, ExtensionId},
};

struct TestLogger;

impl LoggerApi for TestLogger {
    fn log(&self, _level: LogLevel, _message: &str) {}
}

struct TestComponentContext {
    extension_id: ExtensionId,
    component_id: ComponentId,
    logger: TestLogger,
}

impl TestComponentContext {
    fn new() -> Self {
        Self {
            extension_id: ExtensionId::new("test-extension"),
            component_id: ComponentId::new("stateful-component"),
            logger: TestLogger,
        }
    }
}

impl ComponentContext for TestComponentContext {
    fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }
}

impl RegistrationContext for TestComponentContext {
    fn register(&mut self, _contribution: ContributionDescriptor) -> ExtensionResult<()> {
        Ok(())
    }
}

const STATEFUL_WASM_COMPONENT: &str = r#"
    (component
        (core module $module
            (memory (export "memory") 1)
            (global $event-count (mut i32) (i32.const 0))

            (func (export "cabi_realloc")
                (param i32 i32 i32 i32)
                (result i32)
                (i32.const 8))

            (func (export "register")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))

            (func (export "start")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))

            (func (export "stop"))

            (func (export "on-event") (param i32 i32 i32 i32)
                (if (i32.eqz
                    (i32.or
                        (i32.eq (global.get $event-count) (i32.const 2))
                        (i32.eq (global.get $event-count) (i32.const 3))))
                    (then unreachable)))
        )

        (core instance $instance (instantiate $module))
        (alias core export $instance "memory" (core memory $memory))
        (alias core export $instance "cabi_realloc" (core func $realloc))
        (alias core export $instance "register" (core func $register))
        (alias core export $instance "start" (core func $start))
        (alias core export $instance "stop" (core func $stop))
        (alias core export $instance "on-event" (core func $on-event))

        (type $lifecycle (func))
        (type $on-event-type (func (param "topic" string) (param "payload" (list u8))))

        (func $register-lifted (type $lifecycle) (canon lift (core func $register)))
        (func $start-lifted (type $lifecycle) (canon lift (core func $start)))
        (func $stop-lifted (type $lifecycle) (canon lift (core func $stop)))
        (func $on-event-lifted (type $on-event-type)
            (canon lift (core func $on-event) (memory $memory) (realloc $realloc) string-encoding=utf8))

        (type $guest (instance
            (export "register" (func (type $lifecycle)))
            (export "start" (func (type $lifecycle)))
            (export "stop" (func (type $lifecycle)))
            (export "on-event" (func (type $on-event-type)))
        ))
        (component $guest-shim
            (type $lifecycle (func))
            (type $on-event-type (func (param "topic" string) (param "payload" (list u8))))
            (import "register" (func $register (type $lifecycle)))
            (import "start" (func $start (type $lifecycle)))
            (import "stop" (func $stop (type $lifecycle)))
            (import "on-event" (func $on-event (type $on-event-type)))
            (export "register" (func $register))
            (export "start" (func $start))
            (export "stop" (func $stop))
            (export "on-event" (func $on-event))
        )
        (instance $guest-instance (instantiate $guest-shim
            (with "register" (func $register-lifted))
            (with "start" (func $start-lifted))
            (with "stop" (func $stop-lifted))
            (with "on-event" (func $on-event-lifted))
        ))
        (export "taverna:engine/guest@0.0.1" (instance $guest-instance))
    )
"#;

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

#[test]
fn test_should_preserve_guest_state_across_component_lifecycle() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new();
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    component.dispatch_event("chat.message", b"first")?;
    component.stop(&mut context)?;
    component.start(&mut context)?;
    component.dispatch_event("chat.message", b"second")?;

    Ok(())
}
