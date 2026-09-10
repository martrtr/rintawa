use rintawa_extension_engine::{EngineResult, WasmRuntimeEngine};
use rintawa_sdk::{
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

struct TestContext {
    extension_id: ExtensionId,
    component_id: ComponentId,
    logger: TestLogger,
}

impl TestContext {
    fn new() -> Self {
        Self {
            extension_id: ExtensionId::new("test-extension"),
            component_id: ComponentId::new("publish-test"),
            logger: TestLogger,
        }
    }
}
impl ComponentContext for TestContext {
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

impl RegistrationContext for TestContext {
    fn register(&mut self, _contribution: ContributionDescriptor) -> ExtensionResult<()> {
        Ok(())
    }
}

const PUBLISH_EVENT_UNAVAILABLE_WASM_COMPONENT: &str = r#"
(component
    (type $host-type
        (instance
            (type (;0;) (enum "trace" "debug" "info" "warn" "error"))
            (export (;1;) "log-level" (type (eq 0)))
            (type (;2;) (variant (case "unavailable")))
            (export (;3;) "publish-error" (type (eq 2)))
            (type (;4;) (func (param "level" 1) (param "message" string)))
            (export (;0;) "log" (func (type 4)))
            (type (;5;) (list u8))
            (type (;6;) (result (error 3)))
            (type (;7;) (func (param "topic" string) (param "payload" 5) (result 6)))
            (export (;1;) "publish-event" (func (type 7)))
        ))
    (import "rintawa:engine/host@0.0.1" (instance $host (type $host-type)))
    (alias export $host "publish-event" (func $publish-event))

    (core module $memory-module
        (memory (export "memory") 1)
    )
    (core instance $memory-instance (instantiate $memory-module))
    (alias core export $memory-instance "memory" (core memory $memory))

    (core func $publish-event-lowered
        (canon lower
            (func $publish-event)
            (memory $memory)
            string-encoding=utf8))
    (core instance $host-core
        (export "publish-event" (func $publish-event-lowered))
    )
    (core instance $memory-core
        (export "memory" (memory $memory))
    )

    (core module $module
        (import "memory" "memory" (memory 1))
        (type $publish-type (func (param i32 i32 i32 i32 i32)))
        (import "host" "publish-event" (func $publish (type $publish-type)))

        (func (export "cabi_realloc")            (param i32 i32 i32 i32)
            (result i32)
            (i32.const 128))

        (data (i32.const 0) "dialogue.message")
        (data (i32.const 32) "payload")

        (func (export "register"))
        (func (export "start")
            (call $publish
                (i32.const 0)
                (i32.const 16)
                (i32.const 32)
                (i32.const 7)
                (i32.const 96))
            ;; result discriminant: 0 = ok, 1 = error
            (if (i32.ne
                (i32.load8_u (i32.const 96))
                (i32.const 1))
                (then unreachable))
            ;; publish-error discriminant: 0 = unavailable
            (if (i32.ne
                (i32.load8_u (i32.const 97))
                (i32.const 0))
                (then unreachable)))
        (func (export "stop"))
        (func (export "on-event") (param i32 i32 i32 i32))
    )
    (core instance $instance (instantiate $module
        (with "memory" (instance $memory-core))
        (with "host" (instance $host-core))
    ))
    (alias core export $instance "cabi_realloc" (core func $realloc))
    (alias core export $instance "register" (core func $register))
    (alias core export $instance "start" (core func $start))
    (alias core export $instance "stop" (core func $stop))
    (alias core export $instance "on-event" (core func $on-event))

    (type $lifecycle (func))
    (type $on-event-type
        (func (param "topic" string) (param "payload" (list u8))))
    (func $register-lifted (type $lifecycle)
        (canon lift (core func $register)))
    (func $start-lifted (type $lifecycle)
        (canon lift (core func $start)))
    (func $stop-lifted (type $lifecycle)
        (canon lift (core func $stop)))
    (func $on-event-lifted (type $on-event-type)
        (canon lift
            (core func $on-event)
            (memory $memory)
            (realloc $realloc)
            string-encoding=utf8))
    (type $guest (instance
        (export "register" (func (type $lifecycle)))
        (export "start" (func (type $lifecycle)))
        (export "stop" (func (type $lifecycle)))
        (export "on-event" (func (type $on-event-type)))
    ))
    (component $guest-shim
        (type $lifecycle (func))
        (type $on-event-type
            (func (param "topic" string) (param "payload" (list u8))))
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
    (export "rintawa:engine/guest@0.0.1" (instance $guest-instance))
)
"#;

#[test]
fn test_guest_observes_publish_event_unavailable() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("publish-test"),
        PUBLISH_EVENT_UNAVAILABLE_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;

    Ok(())
}
