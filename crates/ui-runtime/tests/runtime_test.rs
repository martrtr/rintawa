//! Integration tests for portable UI lifecycle, patching, actions, and teardown.

use anyhow::Result;
use rintawa_sdk::{
    contracts::ComponentRef,
    types::{ExtensionInstanceId, RuntimeScopeId},
    ui::{
        UI_CAPABILITY_BUTTON, UI_CAPABILITY_COLUMN, UI_CAPABILITY_TEXT, UI_CAPABILITY_TEXT_INPUT,
        UiActionEvent, UiActionId, UiActionPayload, UiButtonAppearance, UiButtonNode,
        UiCapabilityId, UiContainerNode, UiError, UiLayerDescriptor, UiNode, UiNodeId, UiNodeKind,
        UiPatch, UiPatchBatch, UiPlacementHint, UiSurfaceContribution, UiSurfaceId,
        UiSurfaceSnapshot, UiTextInputNode, UiTextNode,
    },
};
use rintawa_ui_runtime::{OwnedUiLayerDescriptor, OwnedUiSurfaceContribution, UiRuntime};

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

    runtime.set_instance_active(&instance("layer"), false)?;
    assert!(runtime.active_layer(&scope()).is_none());
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
