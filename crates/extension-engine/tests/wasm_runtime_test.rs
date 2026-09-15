use rintawa_extension_engine::{EngineResult, WasmExecutionBudget, WasmRuntimeEngine};
use rintawa_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contracts::{ContractKey, ContractVersion},
    contributions::ContributionDescriptor,
    errors::{ExtensionError, ExtensionResult},
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
    ui::{UiActionEvent, UiActionId, UiActionPayload, UiNodeId, UiSurfaceId},
};
use std::fs;

struct TestLogger;

impl LoggerApi for TestLogger {
    fn log(&self, _level: LogLevel, _message: &str) {}
}

struct TestComponentContext {
    extension_id: ExtensionId,
    instance_id: ExtensionInstanceId,
    scope_id: RuntimeScopeId,
    component_id: ComponentId,
    logger: TestLogger,
}

impl TestComponentContext {
    fn new() -> Self {
        Self {
            extension_id: ExtensionId::new("test-extension"),
            instance_id: ExtensionInstanceId::new("test-extension"),
            scope_id: RuntimeScopeId::new("default"),
            component_id: ComponentId::new("stateful-component"),
            logger: TestLogger,
        }
    }
}

impl ComponentContext for TestComponentContext {
    fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    fn extension_instance_id(&self) -> &ExtensionInstanceId {
        &self.instance_id
    }

    fn runtime_scope_id(&self) -> &RuntimeScopeId {
        &self.scope_id
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

const STATEFUL_WASM_COMPONENT: &str = include_str!("fixtures/stateful_component.wat");

#[test]
fn test_wasm_runtime_engine_initialization() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
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
fn test_should_dispatch_service_request_to_wasm_guest() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    let response = component.handle_service(
        &mut context,
        &ContractKey::new("example.service", ContractVersion::new(1)),
        b"payload",
    )?;

    assert!(response.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_oversized_wasm_service_request() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        max_host_message_bytes: 32,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    let error = component
        .handle_service(
            &mut context,
            &ContractKey::new("x", ContractVersion::new(1)),
            &[0; 33],
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ExtensionError::HostMessageTooLarge {
            operation: "service request",
            actual_bytes: 33,
            maximum_bytes: 32,
        }
    ));
    Ok(())
}

#[test]
fn test_should_report_wasm_trap_from_service_handler() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    let error = component
        .handle_service(
            &mut context,
            &ContractKey::new("example.service", ContractVersion::new(99)),
            b"payload",
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ExtensionError::Message(message) if message.contains("service request failed")
    ));
    Ok(())
}

#[test]
fn test_should_dispatch_validated_ui_action_to_wasm_guest() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    component.handle_ui_action(
        &mut context,
        &UiActionEvent {
            owner_instance_id: ExtensionInstanceId::new("test-extension"),
            surface_id: UiSurfaceId::new("example.main"),
            node_id: UiNodeId::new("send"),
            action_id: UiActionId::new("example.send"),
            surface_revision: 1,
            payload: UiActionPayload::None,
        },
    )?;

    Ok(())
}

#[test]
fn test_should_preserve_guest_state_across_component_lifecycle() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    component.dispatch_event(&mut context, "chat.message", b"first")?;
    component.stop(&mut context)?;
    component.start(&mut context)?;
    component.dispatch_event(&mut context, "chat.message", b"second")?;

    Ok(())
}

#[test]
fn test_should_reject_component_artifacts_above_the_host_budget() {
    let budget = WasmExecutionBudget {
        max_component_bytes: 8,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget).unwrap();

    let error = match runtime.load_component_from_bytes(ComponentId::new("oversized"), &[0; 9]) {
        Ok(_) => panic!("oversized component should be rejected"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        rintawa_extension_engine::EngineError::WasmArtifactTooLarge {
            observed_bytes: 9,
            maximum_bytes: 8,
            ..
        }
    ));
}

#[test]
fn test_should_bound_file_reads_before_compiling_an_oversized_artifact() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        max_component_bytes: 8,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let directory = tempfile::tempdir()?;
    let artifact_path = directory.path().join("oversized.wasm");
    fs::write(&artifact_path, [0; 9])?;

    let error =
        match runtime.load_component_from_file(ComponentId::new("oversized"), &artifact_path) {
            Ok(_) => panic!("oversized component should be rejected"),
            Err(error) => error,
        };

    assert!(matches!(
        error,
        rintawa_extension_engine::EngineError::WasmArtifactTooLarge {
            observed_bytes: 9,
            maximum_bytes: 8,
            ..
        }
    ));

    Ok(())
}

#[test]
fn test_should_reject_messages_above_the_host_budget() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        max_host_message_bytes: 3,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;

    assert!(matches!(
        component.dispatch_event(&mut context, "chat", b"ok"),
        Err(ExtensionError::HostMessageTooLarge {
            operation: "event topic",
            actual_bytes: 4,
            maximum_bytes: 3,
        })
    ));

    Ok(())
}

#[test]
fn test_should_stop_an_infinite_guest_callback_when_its_fuel_is_exhausted() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        fuel_per_callback: 10_000,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let fuel_exhausting_component = STATEFUL_WASM_COMPONENT.replace(
        r#"(func (export "start")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))"#,
        r#"(func (export "start") (loop br 0))"#,
    );
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        fuel_exhausting_component.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;

    assert!(matches!(
        component.start(&mut context),
        Err(ExtensionError::ExecutionBudgetExceeded {
            resource: "fuel",
            operation: "start",
        })
    ));
    assert!(matches!(
        component.start(&mut context),
        Err(ExtensionError::Message(message))
            if message.contains("discarded after failed `start` callback")
    ));

    let mut healthy_component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        STATEFUL_WASM_COMPONENT.as_bytes(),
    )?;
    let mut healthy_context = TestComponentContext::new();
    healthy_component.register(&mut healthy_context)?;
    healthy_component.start(&mut healthy_context)?;

    Ok(())
}

#[test]
fn test_should_discard_wasm_instance_after_failed_stop() -> EngineResult<()> {
    let runtime = WasmRuntimeEngine::new()?;
    let stop_failing_component = STATEFUL_WASM_COMPONENT.replace(
        r#"(func (export "stop"))"#,
        r#"(func (export "stop") unreachable)"#,
    );
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        stop_failing_component.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    assert!(component.stop(&mut context).is_err());
    assert!(matches!(
        component.start(&mut context),
        Err(ExtensionError::Message(message))
            if message.contains("discarded after failed `stop` callback")
    ));

    Ok(())
}

#[test]
fn test_should_reset_fuel_for_each_callback_on_the_same_guest_instance() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        fuel_per_callback: 100_000,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let fuel_consuming_component = STATEFUL_WASM_COMPONENT
        .replace(
            r#"(func (export "start")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))"#,
            r#"(func (export "start")
                (local $remaining i32)
                (loop $work
                    (local.set $remaining
                        (i32.add (local.get $remaining) (i32.const 1)))
                    (br_if $work
                        (i32.lt_u (local.get $remaining) (i32.const 8000)))))"#,
        )
        .replace(
            r#"(func (export "on-event") (param i32 i32 i32 i32)
                (if (i32.eqz
                    (i32.or
                        (i32.eq (global.get $event-count) (i32.const 2))
                        (i32.eq (global.get $event-count) (i32.const 3))))
                    (then unreachable)))"#,
            r#"(func (export "on-event") (param i32 i32 i32 i32)
                (local $remaining i32)
                (loop $work
                    (local.set $remaining
                        (i32.add (local.get $remaining) (i32.const 1)))
                    (br_if $work
                        (i32.lt_u (local.get $remaining) (i32.const 8000)))))"#,
        );
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        fuel_consuming_component.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    component.register(&mut context)?;
    component.start(&mut context)?;
    component.dispatch_event(&mut context, "chat.message", b"message")?;

    Ok(())
}

#[test]
fn test_should_reject_a_component_that_exceeds_the_memory_budget() -> EngineResult<()> {
    let budget = WasmExecutionBudget {
        max_memory_bytes: 64 * 1024,
        ..WasmExecutionBudget::default()
    };
    let runtime = WasmRuntimeEngine::with_execution_budget(budget)?;
    let oversized_memory_component = STATEFUL_WASM_COMPONENT.replace(
        r#"(memory (export "memory") 1)"#,
        r#"(memory (export "memory") 2)"#,
    );
    let mut component = runtime.load_component_from_bytes(
        ComponentId::new("stateful-component"),
        oversized_memory_component.as_bytes(),
    )?;
    let mut context = TestComponentContext::new();

    let error = component.register(&mut context).unwrap_err();
    assert!(error.to_string().contains("memory"));

    Ok(())
}
