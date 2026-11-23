//! Integration tests for WASM component execution, budgets, and host boundaries.

use rintawa_extension_engine::{
    EngineResult, ExtensionEngine, WasmExecutionBudget, WasmRuntimeEngine,
};
use rintawa_sdk::{
    api::{LogLevel, LoggerApi},
    context::{ComponentContext, RegistrationContext},
    contributions::ContributionDescriptor,
    errors::{ExtensionError, ExtensionResult},
    manifest::{
        ComponentDescriptor, ComponentKind, ComponentPermissions, ExtensionManifest,
        WASM_COMPONENT_TARGET_V1,
    },
    runtime_permissions::RuntimePermission,
    traits::Component,
    types::{ComponentId, ComponentTarget, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};
use std::{fs, time::Duration};

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
const TASK_WASM_COMPONENT: &[u8] = include_bytes!("fixtures/task-runtime/component.wasm");

#[test]
fn test_should_initialize_wasm_runtime_engine() -> EngineResult<()> {
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
fn test_should_execute_granted_background_task_through_engine_runtime_pump() -> EngineResult<()> {
    let mut engine = ExtensionEngine::new();
    let component_id = ComponentId::new("runtime");
    let component = engine
        .wasm_runtime_engine()?
        .load_component_from_bytes(component_id.clone(), TASK_WASM_COMPONENT)?;
    let manifest = ExtensionManifest {
        id: ExtensionId::new("task-runtime"),
        name: String::from("Task Runtime"),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: vec![ComponentDescriptor {
            id: component_id.clone(),
            kind: ComponentKind::Runtime,
            target: ComponentTarget::new(WASM_COMPONENT_TARGET_V1),
            entry: None,
            required: true,
            published_preference: None,
            permissions: ComponentPermissions {
                secret_read: Vec::new(),
                runtime: vec![RuntimePermission::BackgroundTask],
            },
        }],
    };
    let extension_id = manifest.id.clone();
    engine.register_extension(manifest, vec![Box::new(component)])?;
    engine.grant_requested_runtime_permission(
        &extension_id,
        &component_id,
        RuntimePermission::BackgroundTask,
    )?;
    engine.start_extension(&extension_id)?;

    let delay = engine
        .poll_runtime()?
        .expect("task fixture should request a cooperative wake-up");
    std::thread::sleep(delay + Duration::from_millis(2));
    assert_eq!(engine.poll_runtime()?, None);

    engine.stop_extension(&extension_id)?;
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
