//! Integration tests for portable UI lifecycle, patching, actions, and teardown.

use anyhow::Result;
use rintawa_sdk::{
    contracts::ComponentRef,
    types::{ExtensionInstanceId, RuntimeScopeId},
    ui::{
        UI_CAPABILITY_BUTTON, UI_CAPABILITY_COLUMN, UI_CAPABILITY_TEXT, UI_CAPABILITY_TEXT_INPUT,
        UiActionEvent, UiActionId, UiActionPayload, UiButtonAppearance, UiButtonNode,
        UiCapabilityId, UiContainerNode, UiError, UiLayerDescriptor, UiNode, UiNodeId, UiNodeKind,
        UiPatch, UiPatchBatch, UiPlacementHint, UiPresentationContext, UiSurfaceContribution,
        UiSurfaceId, UiSurfaceSnapshot, UiTextInputNode, UiTextNode, WorldPresentationDescriptor,
    },
    world::WorldId,
};
use rintawa_ui_runtime::{
    OwnedUiLayerDescriptor, OwnedUiSurfaceContribution, OwnedWorldPresentationDescriptor, UiRuntime,
};

fn instance(id: &str) -> ExtensionInstanceId {
    ExtensionInstanceId::new(id)
}

fn scope() -> RuntimeScopeId {
    RuntimeScopeId::new("default")
}

fn owner(instance_id: &str) -> ComponentRef {
    ComponentRef::new(instance_id, "runtime")
}

fn surface() -> UiSurfaceContribution {
    UiSurfaceContribution::new("example.main", UiPlacementHint::Primary)
}

fn base_snapshot() -> UiSurfaceSnapshot {
    UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            UiNode::new(
                "root",
                UiNodeKind::Column(UiContainerNode {
                    children: vec!["title".into(), "send".into(), "input".into()],
                }),
            ),
            UiNode::new(
                "title",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Hello"),
                }),
            ),
            UiNode::new(
                "send",
                UiNodeKind::Button(UiButtonNode {
                    label: String::from("Send"),
                    action: UiActionId::new("example.send"),
                    is_enabled: true,
                    appearance: UiButtonAppearance::Default,
                }),
            ),
            UiNode::new(
                "input",
                UiNodeKind::TextInput(UiTextInputNode {
                    value: String::new(),
                    placeholder: Some(String::from("Message")),
                    change_action: Some(UiActionId::new("example.change")),
                    submit_action: Some(UiActionId::new("example.send-text")),
                    is_enabled: true,
                }),
            ),
        ],
    }
}

fn compatible_layer() -> UiLayerDescriptor {
    UiLayerDescriptor::new(vec![
        UiCapabilityId::new(UI_CAPABILITY_COLUMN),
        UiCapabilityId::new(UI_CAPABILITY_TEXT),
        UiCapabilityId::new(UI_CAPABILITY_BUTTON),
        UiCapabilityId::new(UI_CAPABILITY_TEXT_INPUT),
    ])
}

fn register_feature(runtime: &UiRuntime, contribution: UiSurfaceContribution) -> Result<()> {
    runtime.register_instance(
        instance("feature"),
        scope(),
        vec![OwnedUiSurfaceContribution {
            owner: owner("feature"),
            contribution,
        }],
        Vec::new(),
    )?;
    Ok(())
}

fn attach_layer(runtime: &UiRuntime, descriptor: UiLayerDescriptor) -> Result<()> {
    runtime.register_instance(
        instance("layer"),
        scope(),
        Vec::new(),
        vec![OwnedUiLayerDescriptor {
            owner: owner("layer"),
            descriptor,
        }],
    )?;
    runtime.set_instance_active(&instance("layer"), true)?;
    runtime.attach_registered_layer(owner("layer"))?;
    Ok(())
}

#[test]
fn test_should_mount_headless_and_attach_compatible_layer_later() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    assert!(runtime.active_layer(&scope()).is_none());
    assert!(runtime.presentation_surfaces().is_empty());

    runtime.set_instance_active(&instance("feature"), true)?;
    attach_layer(&runtime, compatible_layer())?;

    let layer = runtime
        .active_layer(&scope())
        .expect("layer should be attached");
    assert_eq!(layer.0, owner("layer"));
    assert_eq!(runtime.presentation_surfaces()[0].snapshot.revision, 1);
    Ok(())
}

#[test]
fn test_should_reject_layer_missing_required_capability() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(
        &runtime,
        surface().requiring_capability(UiCapabilityId::new("example.canvas@1")),
    )?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    runtime.register_instance(
        instance("layer"),
        scope(),
        Vec::new(),
        vec![OwnedUiLayerDescriptor {
            owner: owner("layer"),
            descriptor: compatible_layer(),
        }],
    )?;
    runtime.set_instance_active(&instance("layer"), true)?;

    assert_eq!(
        runtime.attach_registered_layer(owner("layer")),
        Err(UiError::UnsupportedCapability(String::from(
            "example.canvas@1"
        )))
    );
    assert!(runtime.active_layer(&scope()).is_none());
    Ok(())
}

#[test]
fn test_should_apply_patch_batch_atomically_and_advance_revision() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;

    runtime.apply_patches(
        &owner("feature"),
        UiPatchBatch {
            surface_id: UiSurfaceId::new("example.main"),
            base_revision: 1,
            next_revision: 2,
            patches: vec![UiPatch::UpsertNode {
                node: UiNode::new(
                    "title",
                    UiNodeKind::Text(UiTextNode {
                        text: String::from("Updated"),
                    }),
                ),
            }],
        },
    )?;

    let snapshot = &runtime.presentation_surfaces()[0].snapshot;
    assert_eq!(snapshot.revision, 2);
    let title = snapshot
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "title")
        .expect("title should exist");
    assert!(matches!(
        &title.kind,
        UiNodeKind::Text(UiTextNode { text }) if text == "Updated"
    ));
    Ok(())
}

#[test]
fn test_should_keep_previous_snapshot_when_patch_batch_is_invalid() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;

    let error = runtime.apply_patches(
        &owner("feature"),
        UiPatchBatch {
            surface_id: UiSurfaceId::new("example.main"),
            base_revision: 1,
            next_revision: 2,
            patches: vec![
                UiPatch::UpsertNode {
                    node: UiNode::new(
                        "title",
                        UiNodeKind::Text(UiTextNode {
                            text: String::from("Should rollback"),
                        }),
                    ),
                },
                UiPatch::InsertChild {
                    parent: "root".into(),
                    index: 0,
                    child: "missing".into(),
                },
            ],
        },
    );
    assert_eq!(error, Err(UiError::NodeNotFound(String::from("missing"))));

    let snapshot = &runtime.presentation_surfaces()[0].snapshot;
    assert_eq!(snapshot.revision, 1);
    let title = snapshot
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "title")
        .expect("title should exist");
    assert!(matches!(
        &title.kind,
        UiNodeKind::Text(UiTextNode { text }) if text == "Hello"
    ));
    Ok(())
}

#[test]
fn test_should_route_only_owned_bound_actions_from_active_layer() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_layer(&runtime, compatible_layer())?;

    let valid = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "send".into(),
        action_id: UiActionId::new("example.send"),
        surface_revision: 1,
        payload: UiActionPayload::None,
    };
    let dispatch = runtime.route_action(&owner("layer"), valid.clone())?;
    assert_eq!(dispatch.owner, owner("feature"));
    assert_eq!(dispatch.event, valid);

    assert_eq!(
        runtime.route_action(&owner("attacker"), valid.clone()),
        Err(UiError::LayerNotOwner)
    );

    let mut unbound = valid.clone();
    unbound.action_id = UiActionId::new("other.action");
    assert!(matches!(
        runtime.route_action(&owner("layer"), unbound),
        Err(UiError::ActionNotBound { .. })
    ));

    let mut stale = valid.clone();
    stale.surface_revision = 0;
    assert_eq!(
        runtime.route_action(&owner("layer"), stale),
        Err(UiError::RevisionMismatch {
            expected: 1,
            actual: 0,
        })
    );

    let mut invalid_payload = valid;
    invalid_payload.payload = UiActionPayload::Text(String::from("spoof"));
    assert!(matches!(
        runtime.route_action(&owner("layer"), invalid_payload),
        Err(UiError::InvalidActionPayload { .. })
    ));
    Ok(())
}

#[test]
fn test_should_bound_queued_renderer_actions() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_layer(&runtime, compatible_layer())?;

    let event = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "send".into(),
        action_id: UiActionId::new("example.send"),
        surface_revision: 1,
        payload: UiActionPayload::None,
    };

    for _ in 0..64 {
        runtime.queue_action(&owner("layer"), event.clone())?;
    }
    assert_eq!(
        runtime.queue_action(&owner("layer"), event),
        Err(UiError::ActionQueueFull)
    );
    assert_eq!(runtime.drain_queued_actions()?.len(), 64);
    Ok(())
}

#[test]
fn test_should_remove_surfaces_and_layer_on_extension_deactivation() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_layer(&runtime, compatible_layer())?;

    runtime.set_instance_active(&instance("feature"), false)?;
    assert!(runtime.presentation_surfaces().is_empty());
    assert!(runtime.active_layer(&scope()).is_some());

    runtime.queue_world_focus_request(&owner("layer"), WorldId::new())?;
    runtime.set_instance_active(&instance("layer"), false)?;
    assert!(runtime.active_layer(&scope()).is_none());
    assert!(runtime.drain_world_focus_requests()?.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_invalid_surface_tree() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    let invalid = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            UiNode::new(
                "root",
                UiNodeKind::Column(UiContainerNode {
                    children: vec!["child".into()],
                }),
            ),
            UiNode::new(
                "child",
                UiNodeKind::Column(UiContainerNode {
                    children: vec!["root".into()],
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid),
        Err(UiError::RootHasParent(_)) | Err(UiError::CycleDetected(_))
    ));
    Ok(())
}

#[test]
fn test_should_import_only_focused_world_scope_into_layer_session() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;
    attach_layer(&runtime, compatible_layer())?;

    let world_a = WorldId::new();
    let world_b = WorldId::new();
    let scope_a = RuntimeScopeId::new("world:a");
    let scope_b = RuntimeScopeId::new("world:b");
    for (instance_id, scope_id) in [("world-a", scope_a.clone()), ("world-b", scope_b.clone())] {
        runtime.register_instance(
            instance(instance_id),
            scope_id,
            vec![OwnedUiSurfaceContribution {
                owner: owner(instance_id),
                contribution: surface(),
            }],
            Vec::new(),
        )?;
        runtime.set_instance_active(&instance(instance_id), true)?;
        runtime.mount_surface(&owner(instance_id), base_snapshot())?;
    }

    let layer_owner = owner("layer");
    let local = runtime.presentation_surfaces_for_layer(&layer_owner)?;
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].owner, owner("feature"));
    assert_eq!(local[0].context, Some(UiPresentationContext::LayerLocal));

    let world_action = |instance_id: &str| UiActionEvent {
        owner_instance_id: instance(instance_id),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "send".into(),
        action_id: UiActionId::new("example.send"),
        surface_revision: 1,
        payload: UiActionPayload::None,
    };
    assert_eq!(
        runtime.route_action(&layer_owner, world_action("world-a")),
        Err(UiError::ScopeNotVisible)
    );

    runtime.set_focused_world_scope(
        &layer_owner,
        world_a,
        scope_a,
        WorldPresentationDescriptor::new("example.main"),
    )?;
    assert_eq!(
        runtime.focused_world_for_layer(&layer_owner)?,
        Some(world_a)
    );
    let focused_a = runtime.presentation_surfaces_for_layer(&layer_owner)?;
    assert_eq!(focused_a.len(), 2);
    assert!(focused_a.iter().any(|surface| {
        surface.owner == owner("feature")
            && surface.context == Some(UiPresentationContext::LayerLocal)
    }));
    assert!(focused_a.iter().any(|surface| {
        surface.owner == owner("world-a")
            && surface.context == Some(UiPresentationContext::FocusedWorld { world_id: world_a })
    }));
    assert!(
        !focused_a
            .iter()
            .any(|surface| surface.owner == owner("world-b"))
    );
    assert!(
        runtime
            .route_action(&layer_owner, world_action("world-a"))
            .is_ok()
    );

    runtime.set_focused_world_scope(
        &layer_owner,
        world_b,
        scope_b,
        WorldPresentationDescriptor::new("example.main"),
    )?;
    assert_eq!(
        runtime.focused_world_for_layer(&layer_owner)?,
        Some(world_b)
    );
    assert_eq!(
        runtime.route_action(&layer_owner, world_action("world-a")),
        Err(UiError::ScopeNotVisible)
    );
    assert!(
        runtime
            .route_action(&layer_owner, world_action("world-b"))
            .is_ok()
    );
    assert_eq!(runtime.presentation_surfaces().len(), 3);

    runtime.clear_focused_world_scope(&layer_owner)?;
    assert_eq!(runtime.focused_world_for_layer(&layer_owner)?, None);
    assert_eq!(
        runtime.presentation_surfaces_for_layer(&layer_owner)?.len(),
        1
    );
    assert_eq!(
        runtime.route_action(&layer_owner, world_action("world-b")),
        Err(UiError::ScopeNotVisible)
    );
    Ok(())
}

#[test]
fn test_should_coalesce_deferred_world_focus_requests_per_layer() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_layer(&runtime, compatible_layer())?;

    let layer_owner = owner("layer");
    let world_a = WorldId::new();
    let world_b = WorldId::new();

    runtime.queue_world_focus_request(&layer_owner, world_a)?;
    runtime.queue_world_focus_request(&layer_owner, world_b)?;

    let state = runtime.presentation_state_for_layer(&layer_owner)?;
    assert!(state.focused_world.is_none());
    assert_eq!(state.pending_world_id, Some(world_b));
    assert!(state.last_focus_error.is_none());

    let requests = runtime.drain_world_focus_requests()?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].layer_owner, layer_owner);
    assert_eq!(requests[0].world_id, world_b);

    runtime.record_world_focus_failure(&layer_owner, world_a, "stale failure")?;
    let state = runtime.presentation_state_for_layer(&layer_owner)?;
    assert_eq!(state.pending_world_id, Some(world_b));
    assert!(state.last_focus_error.is_none());

    runtime.record_world_focus_failure(&layer_owner, world_b, &"é".repeat(2_048))?;
    let state = runtime.presentation_state_for_layer(&layer_owner)?;
    assert!(state.pending_world_id.is_none());
    let diagnostic = state
        .last_focus_error
        .expect("matching focus failure should be retained");
    assert!(diagnostic.len() <= 2 * 1_024);

    runtime.queue_world_focus_request(&layer_owner, world_a)?;
    runtime.clear_focused_world_scope(&layer_owner)?;
    assert!(runtime.drain_world_focus_requests()?.is_empty());
    let state = runtime.presentation_state_for_layer(&layer_owner)?;
    assert!(state.focused_world.is_none());
    assert!(state.pending_world_id.is_none());
    assert!(state.last_focus_error.is_none());

    runtime.queue_world_focus_request(&layer_owner, world_b)?;
    runtime.detach_layer(&layer_owner)?;
    assert!(runtime.drain_world_focus_requests()?.is_empty());
    Ok(())
}

#[test]
fn test_should_bind_world_presentation_descriptor_to_same_component_surface() -> Result<()> {
    let runtime = UiRuntime::new();
    runtime.register_instance(
        instance("world-feature"),
        RuntimeScopeId::new("world:test"),
        vec![
            OwnedUiSurfaceContribution {
                owner: owner("world-feature"),
                contribution: surface(),
            },
            OwnedUiSurfaceContribution {
                owner: ComponentRef::new("world-feature", "other"),
                contribution: UiSurfaceContribution::new(
                    "example.other",
                    UiPlacementHint::Secondary,
                ),
            },
        ],
        Vec::new(),
    )?;

    let descriptor = WorldPresentationDescriptor::new("example.main");
    runtime.register_world_presentation(OwnedWorldPresentationDescriptor {
        owner: owner("world-feature"),
        descriptor: descriptor.clone(),
    })?;
    assert_eq!(
        runtime.registered_world_presentation_descriptor(&owner("world-feature"))?,
        descriptor
    );
    assert_eq!(
        runtime.register_world_presentation(OwnedWorldPresentationDescriptor {
            owner: owner("world-feature"),
            descriptor: WorldPresentationDescriptor::new("example.main"),
        }),
        Err(UiError::WorldPresentationAlreadyRegistered)
    );

    assert_eq!(
        runtime.register_world_presentation(OwnedWorldPresentationDescriptor {
            owner: ComponentRef::new("world-feature", "missing-owner"),
            descriptor: WorldPresentationDescriptor::new("example.main"),
        }),
        Err(UiError::SurfaceNotOwned(String::from("example.main")))
    );
    assert_eq!(
        runtime.register_world_presentation(OwnedWorldPresentationDescriptor {
            owner: ComponentRef::new("world-feature", "other"),
            descriptor: WorldPresentationDescriptor::new("missing.surface"),
        }),
        Err(UiError::SurfaceNotRegistered(String::from(
            "missing.surface"
        )))
    );
    Ok(())
}

#[test]
fn test_should_move_child_using_post_removal_index() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    runtime.mount_surface(&owner("feature"), base_snapshot())?;

    runtime.apply_patches(
        &owner("feature"),
        UiPatchBatch {
            surface_id: UiSurfaceId::new("example.main"),
            base_revision: 1,
            next_revision: 2,
            patches: vec![UiPatch::MoveChild {
                parent: "root".into(),
                child: "title".into(),
                index: 2,
            }],
        },
    )?;

    let snapshot = &runtime.presentation_surfaces()[0].snapshot;
    let root = snapshot
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "root")
        .expect("root should exist");
    assert_eq!(
        root.kind.children(),
        &[
            UiNodeId::new("send"),
            UiNodeId::new("input"),
            UiNodeId::new("title")
        ]
    );
    Ok(())
}
