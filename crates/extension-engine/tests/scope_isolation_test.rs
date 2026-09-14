use anyhow::Result;
use rintawa_extension_engine::{ExtensionEngine, ExtensionState};
use rintawa_sdk::prelude::*;

struct ScopedComponent {
    id: ComponentId,
}

impl ScopedComponent {
    fn new() -> Self {
        Self {
            id: ComponentId::new("runtime"),
        }
    }
}

impl Component for ScopedComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        ctx.register(ContributionDescriptor::new(
            "example.shared",
            ContributionKind::capability(),
        ))?;
        ctx.register_ui_surface(UiSurfaceContribution::new(
            "example.main",
            UiPlacementHint::Primary,
        ))?;
        Ok(())
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        ctx.register_runtime_effect(RuntimeEffect::event_subscription("example.event"))?;
        ctx.mount_ui_surface(UiSurfaceSnapshot {
            surface_id: UiSurfaceId::new("example.main"),
            revision: 1,
            root: UiNodeId::new("root"),
            nodes: vec![UiNode::new(
                "root",
                UiNodeKind::Text(UiTextNode {
                    text: ctx.runtime_scope_id().to_string(),
                }),
            )],
        })
        .map_err(|error| ExtensionError::Message(error.to_string()))?;
        Ok(())
    }
}

fn manifest() -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new("example.shared-extension"),
        name: String::from("Shared Extension"),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: Vec::new(),
    }
}

#[test]
fn test_same_logical_extension_is_isolated_between_runtime_scopes() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let instance_a = ExtensionInstanceId::new("shared@world-a");
    let instance_b = ExtensionInstanceId::new("shared@world-b");
    let scope_a = RuntimeScopeId::new("world-a");
    let scope_b = RuntimeScopeId::new("world-b");

    engine.register_extension_instance(
        instance_a.clone(),
        scope_a.clone(),
        manifest(),
        vec![Box::new(ScopedComponent::new())],
    )?;
    engine.register_extension_instance(
        instance_b.clone(),
        scope_b.clone(),
        manifest(),
        vec![Box::new(ScopedComponent::new())],
    )?;

    engine.start_extension_instance(&instance_a)?;
    engine.start_extension_instance(&instance_b)?;

    assert_eq!(
        engine.extension_instance_state(&instance_a),
        Some(ExtensionState::Active)
    );
    assert_eq!(
        engine.extension_instance_state(&instance_b),
        Some(ExtensionState::Active)
    );
    assert!(
        engine
            .active_contribution_owner_in_scope(&scope_a, &ContributionId::new("example.shared"))
            .is_some()
    );
    assert!(
        engine
            .active_contribution_owner_in_scope(&scope_b, &ContributionId::new("example.shared"))
            .is_some()
    );

    let surfaces = engine.portable_ui_surfaces();
    assert_eq!(surfaces.len(), 2);
    assert!(
        surfaces
            .iter()
            .any(|surface| surface.owner.instance_id == instance_a)
    );
    assert!(
        surfaces
            .iter()
            .any(|surface| surface.owner.instance_id == instance_b)
    );
    assert_eq!(engine.active_runtime_effect_principals().len(), 2);

    engine.stop_extension_instance(&instance_a)?;

    assert_eq!(
        engine.extension_instance_state(&instance_a),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(
        engine.extension_instance_state(&instance_b),
        Some(ExtensionState::Active)
    );
    assert!(
        engine
            .active_contribution_owner_in_scope(&scope_a, &ContributionId::new("example.shared"))
            .is_none()
    );
    assert!(
        engine
            .active_contribution_owner_in_scope(&scope_b, &ContributionId::new("example.shared"))
            .is_some()
    );
    let surfaces = engine.portable_ui_surfaces();
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].owner.instance_id, instance_b);
    let effects = engine.active_runtime_effect_principals();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].1.instance_id, instance_b);

    Ok(())
}
