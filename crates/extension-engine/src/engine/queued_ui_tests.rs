//! Regression tests for queued Portable UI and runtime fault containment.

use super::*;
use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    errors::{ExtensionError, ExtensionResult},
    types::ComponentTarget,
    ui::{
        UI_CAPABILITY_BUTTON, UiActionId, UiActionPayload, UiButtonAppearance, UiButtonNode,
        UiCapabilityId, UiNode, UiNodeId, UiNodeKind, UiPlacementHint, UiSurfaceContribution,
        UiSurfaceId, UiSurfaceSnapshot,
    },
};

struct FailingUiComponent {
    id: ComponentId,
    invalidates_runtime: bool,
}

impl Component for FailingUiComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        ctx.register_ui_surface(UiSurfaceContribution::new(
            "example.main",
            UiPlacementHint::Primary,
        ))
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        ctx.mount_ui_surface(UiSurfaceSnapshot {
            surface_id: UiSurfaceId::new("example.main"),
            revision: 1,
            root: UiNodeId::new("button"),
            nodes: vec![UiNode::new(
                "button",
                UiNodeKind::Button(UiButtonNode {
                    label: String::from("Run"),
                    action: UiActionId::new("example.run"),
                    is_enabled: true,
                    appearance: UiButtonAppearance::Default,
                }),
            )],
        })
        .map_err(|error| ExtensionError::Message(error.to_string()))
    }

    fn handle_ui_action(
        &mut self,
        _ctx: &mut dyn ComponentContext,
        _event: &UiActionEvent,
    ) -> ExtensionResult<()> {
        if self.invalidates_runtime {
            Err(ExtensionError::ComponentRuntimeInvalidated {
                operation: "UI action",
                reason: String::from("synthetic fatal UI failure"),
            })
        } else {
            Err(ExtensionError::Message(String::from(
                "synthetic UI failure",
            )))
        }
    }
}

struct TestLayer {
    id: ComponentId,
}

impl Component for TestLayer {
    fn id(&self) -> &ComponentId {
        &self.id
    }
}

struct FailingPollComponent {
    id: ComponentId,
    stop_order: Arc<Mutex<Vec<String>>>,
}

impl Component for FailingPollComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.stop_order
            .lock()
            .map_err(|_| ExtensionError::Message(String::from("stop order lock poisoned")))?
            .push(String::from("provider"));
        Ok(())
    }

    fn poll_runtime(
        &mut self,
        _ctx: &mut dyn ComponentContext,
    ) -> ExtensionResult<Option<Duration>> {
        Err(ExtensionError::Message(String::from(
            "synthetic runtime poll failure",
        )))
    }
}

struct RecordingStopComponent {
    id: ComponentId,
    stop_order: Arc<Mutex<Vec<String>>>,
}

impl Component for RecordingStopComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.stop_order
            .lock()
            .map_err(|_| ExtensionError::Message(String::from("stop order lock poisoned")))?
            .push(String::from("dependent"));
        Ok(())
    }
}

fn test_manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_string(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: Vec::new(),
    }
}

fn test_ui_event() -> UiActionEvent {
    UiActionEvent {
        owner_instance_id: ExtensionInstanceId::new("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: UiNodeId::new("button"),
        action_id: UiActionId::new("example.run"),
        surface_revision: 1,
        payload: UiActionPayload::None,
    }
}

fn setup_ui_engine(invalidates_runtime: bool) -> EngineResult<(ExtensionEngine, ComponentRef)> {
    let mut engine = ExtensionEngine::new();
    engine.register_extension(
        test_manifest("feature"),
        vec![Box::new(FailingUiComponent {
            id: ComponentId::new("runtime"),
            invalidates_runtime,
        })],
    )?;
    engine.register_extension(
        test_manifest("layer"),
        vec![Box::new(TestLayer {
            id: ComponentId::new("runtime"),
        })],
    )?;
    engine.start_extension(&ExtensionId::new("feature"))?;
    engine.start_extension(&ExtensionId::new("layer"))?;

    let layer = ComponentRef::new("layer", "runtime");
    engine.attach_ui_layer(
        layer.clone(),
        UiLayerDescriptor::new(vec![UiCapabilityId::new(UI_CAPABILITY_BUTTON)]),
    )?;
    Ok((engine, layer))
}

#[test]
fn test_should_quarantine_failing_runtime_poll_and_active_target_dependents() -> EngineResult<()> {
    let stop_order = Arc::new(Mutex::new(Vec::new()));
    let provider_instance = ExtensionInstanceId::new("a-provider");
    let dependent_instance = ExtensionInstanceId::new("b-dependent");

    let mut engine = ExtensionEngine::new();
    engine.register_extension(
        test_manifest("a-provider"),
        vec![Box::new(FailingPollComponent {
            id: ComponentId::new("runtime"),
            stop_order: Arc::clone(&stop_order),
        })],
    )?;
    engine.register_extension(
        test_manifest("b-dependent"),
        vec![Box::new(RecordingStopComponent {
            id: ComponentId::new("runtime"),
            stop_order: Arc::clone(&stop_order),
        })],
    )?;

    engine
        .extensions
        .get_mut(&dependent_instance)
        .ok_or_else(|| EngineError::ExtensionInstanceNotFound(dependent_instance.to_string()))?
        .execution_target_dependencies
        .push(ExecutionTargetDependency {
            component_id: ComponentId::new("runtime"),
            target: ComponentTarget::new("example.runtime.test@1"),
            provider: ComponentRef::new(provider_instance.clone(), "runtime"),
        });

    engine.start_extension(&ExtensionId::new("a-provider"))?;
    engine.start_extension(&ExtensionId::new("b-dependent"))?;

    assert_eq!(engine.poll_runtime()?, None);
    assert_eq!(
        engine.extension_instance_state(&provider_instance),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(
        engine.extension_instance_state(&dependent_instance),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(
        stop_order
            .lock()
            .expect("test stop order lock should remain healthy")
            .as_slice(),
        ["dependent", "provider"]
    );
    Ok(())
}

#[test]
fn test_should_contain_failing_queued_ui_action_inside_owning_extension() -> EngineResult<()> {
    let (mut engine, layer) = setup_ui_engine(false)?;
    engine.ui.queue_action(&layer, test_ui_event())?;

    assert_eq!(engine.poll_runtime()?, None);
    assert_eq!(
        engine.extension_state(&ExtensionId::new("feature")),
        Some(ExtensionState::Active)
    );
    assert_eq!(engine.ui.presentation_surfaces().len(), 1);
    Ok(())
}

#[test]
fn test_should_quarantine_invalidated_component_runtime_without_stopping_host() -> EngineResult<()>
{
    let (mut engine, layer) = setup_ui_engine(true)?;
    assert_eq!(engine.ui.presentation_surfaces().len(), 1);
    engine.ui.queue_action(&layer, test_ui_event())?;

    assert_eq!(engine.poll_runtime()?, None);
    assert_eq!(
        engine.extension_state(&ExtensionId::new("feature")),
        Some(ExtensionState::Stopped)
    );
    assert!(engine.ui.presentation_surfaces().is_empty());
    assert_eq!(
        engine.extension_state(&ExtensionId::new("layer")),
        Some(ExtensionState::Active)
    );
    Ok(())
}

#[test]
fn test_should_quarantine_invalidated_runtime_on_direct_ui_dispatch() -> EngineResult<()> {
    let (mut engine, layer) = setup_ui_engine(true)?;

    assert!(matches!(
        engine.dispatch_ui_action(&layer, test_ui_event()),
        Err(EngineError::UiActionFailed { reason, .. })
            if reason == "synthetic fatal UI failure"
    ));
    assert_eq!(
        engine.extension_state(&ExtensionId::new("feature")),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(
        engine.extension_state(&ExtensionId::new("layer")),
        Some(ExtensionState::Active)
    );
    Ok(())
}

#[test]
fn test_should_treat_stale_queued_ui_validation_as_transient() {
    for error in [
        UiError::LayerNotOwner,
        UiError::ScopeNotVisible,
        UiError::InstanceNotRegistered(String::from("feature")),
        UiError::OwnerInactive,
        UiError::SurfaceNotRegistered(String::from("example.main")),
        UiError::SurfaceNotMounted(String::from("example.main")),
        UiError::RevisionMismatch {
            expected: 2,
            actual: 1,
        },
        UiError::NodeNotFound(String::from("button")),
        UiError::ActionNotBound {
            surface: String::from("example.main"),
            node: String::from("button"),
            action: String::from("run"),
        },
        UiError::ActionDisabled {
            node: String::from("button"),
            action: String::from("run"),
        },
        UiError::InvalidActionPayload {
            node: String::from("button"),
            action: String::from("run"),
        },
    ] {
        assert!(is_transient_queued_ui_rejection(&EngineError::Ui(error)));
    }
}

#[test]
fn test_should_keep_ui_runtime_failures_fatal_to_runtime_pump() {
    assert!(!should_contain_queued_ui_error(&EngineError::Ui(
        UiError::RuntimeUnavailable
    )));
    assert!(!should_contain_queued_ui_error(&EngineError::Ui(
        UiError::SurfaceNotOwned(String::from("example.main"))
    )));
}

#[test]
fn test_should_contain_component_ui_action_failure() {
    assert!(should_contain_queued_ui_error(
        &EngineError::UiActionFailed {
            extension_id: String::from("feature"),
            component_id: String::from("ui"),
            action_id: String::from("run"),
            reason: String::from("failed"),
        }
    ));
}
