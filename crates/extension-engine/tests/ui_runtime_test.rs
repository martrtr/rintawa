use std::sync::{Arc, Mutex};

use anyhow::Result;
use rintawa_extension_engine::ExtensionEngine;
use rintawa_sdk::prelude::*;
use rintawa_sdk::ui::{UI_CAPABILITY_BUTTON, UI_CAPABILITY_COLUMN};

struct FeatureComponent {
    id: ComponentId,
    actions: Arc<Mutex<Vec<UiActionEvent>>>,
    stop_ui_denied: Arc<Mutex<bool>>,
}

impl FeatureComponent {
    fn new(actions: Arc<Mutex<Vec<UiActionEvent>>>, stop_ui_denied: Arc<Mutex<bool>>) -> Self {
        Self {
            id: ComponentId::new("runtime"),
            actions,
            stop_ui_denied,
        }
    }

    fn snapshot() -> UiSurfaceSnapshot {
        UiSurfaceSnapshot {
            surface_id: UiSurfaceId::new("example.main"),
            revision: 1,
            root: UiNodeId::new("root"),
            nodes: vec![
                UiNode::new(
                    "root",
                    UiNodeKind::Column(UiContainerNode {
                        children: vec![UiNodeId::new("button")],
                    }),
                ),
                UiNode::new(
                    "button",
                    UiNodeKind::Button(UiButtonNode {
                        label: String::from("Run"),
                        action: UiActionId::new("example.run"),
                        is_enabled: true,
                    }),
                ),
            ],
        }
    }
}

impl Component for FeatureComponent {
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
        ctx.mount_ui_surface(Self::snapshot())
            .map_err(|error| ExtensionError::Message(error.to_string()))
    }

    fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let result = ctx.unmount_ui_surface(&UiSurfaceId::new("example.main"));
        *self
            .stop_ui_denied
            .lock()
            .expect("test mutex should be valid") = matches!(result, Err(UiError::OwnerInactive));
        Ok(())
    }

    fn handle_ui_action(
        &mut self,
        _ctx: &mut dyn ComponentContext,
        event: &UiActionEvent,
    ) -> ExtensionResult<()> {
        self.actions
            .lock()
            .expect("test mutex should be valid")
            .push(event.clone());
        Ok(())
    }
}

struct LayerComponent {
    id: ComponentId,
}

impl Component for LayerComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }
}

fn manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_string(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: Vec::new(),
    }
}

fn layer_descriptor() -> UiLayerDescriptor {
    UiLayerDescriptor::new(vec![
        UiCapabilityId::new(UI_CAPABILITY_COLUMN),
        UiCapabilityId::new(UI_CAPABILITY_BUTTON),
    ])
}

#[test]
fn test_portable_ui_lifecycle_and_action_dispatch() -> Result<()> {
    let actions = Arc::new(Mutex::new(Vec::new()));
    let stop_ui_denied = Arc::new(Mutex::new(false));
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("feature"),
        vec![Box::new(FeatureComponent::new(
            actions.clone(),
            stop_ui_denied.clone(),
        ))],
    )?;
    engine.register_extension(
        manifest("layer"),
        vec![Box::new(LayerComponent {
            id: ComponentId::new("runtime"),
        })],
    )?;

    engine.start_extension(&ExtensionId::new("feature"))?;
    assert_eq!(engine.portable_ui_surfaces().len(), 1);
    engine.start_extension(&ExtensionId::new("layer"))?;
    let layer = ComponentRef::new("layer", "runtime");
    engine.attach_ui_layer(layer.clone(), layer_descriptor())?;

    let event = UiActionEvent {
        owner_instance_id: ExtensionInstanceId::new("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: UiNodeId::new("button"),
        action_id: UiActionId::new("example.run"),
        surface_revision: 1,
        payload: UiActionPayload::None,
    };
    engine.dispatch_ui_action(&layer, event.clone())?;
    assert_eq!(
        actions
            .lock()
            .expect("test mutex should be valid")
            .as_slice(),
        &[event]
    );

    engine.stop_extension(&ExtensionId::new("feature"))?;
    assert!(engine.portable_ui_surfaces().is_empty());
    assert!(*stop_ui_denied.lock().expect("test mutex should be valid"));

    engine.start_extension(&ExtensionId::new("feature"))?;
    assert_eq!(engine.portable_ui_surfaces().len(), 1);
    Ok(())
}
