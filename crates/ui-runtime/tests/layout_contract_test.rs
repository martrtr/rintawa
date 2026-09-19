//! Integration tests for renderer-neutral portable layout contracts.

use anyhow::Result;
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey, ContractVersion},
    types::{ExtensionInstanceId, RuntimeScopeId},
    ui::{
        UI_CAPABILITY_DATA_GRID, UI_CAPABILITY_TEXT, UiActionId, UiCapabilityId, UiDataGridColumn,
        UiDataGridNode, UiDataGridSortDirection, UiError, UiIconNode, UiIconSlotId, UiImageNode,
        UiLayerDescriptor, UiNode, UiNodeKind, UiPlacementHint, UiSplitAxis, UiSplitNode,
        UiSurfaceContribution, UiSurfaceId, UiSurfaceSnapshot, UiTextNode,
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

fn text_layer() -> UiLayerDescriptor {
    UiLayerDescriptor::new(vec![UiCapabilityId::new(UI_CAPABILITY_TEXT)])
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

#[test]
fn test_should_reject_invalid_weighted_split_layout() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let snapshot = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "split".into(),
        nodes: vec![
            UiNode::new(
                "split",
                UiNodeKind::Split(UiSplitNode {
                    children: vec!["left".into(), "right".into()],
                    weights: vec![1],
                    axis: UiSplitAxis::Horizontal,
                }),
            ),
            UiNode::new(
                "left",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Left"),
                }),
            ),
            UiNode::new(
                "right",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Right"),
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), snapshot),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_validate_data_grid_layout_and_capability() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let snapshot = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "grid".into(),
        nodes: vec![
            UiNode::new(
                "grid",
                UiNodeKind::DataGrid(UiDataGridNode {
                    columns: vec![
                        UiDataGridColumn {
                            key: Some(String::from("name")),
                            label: String::from("Name"),
                            weight: 3,
                            sort_action: Some(UiActionId::new("grid.sort")),
                            sort_direction: Some(UiDataGridSortDirection::Ascending),
                        },
                        UiDataGridColumn {
                            key: Some(String::from("status")),
                            label: String::from("Status"),
                            weight: 1,
                            sort_action: Some(UiActionId::new("grid.sort")),
                            sort_direction: None,
                        },
                    ],
                    cells: vec!["name".into(), "status".into()],
                    selected_rows: vec![0],
                    row_keys: vec![String::from("example")],
                    row_action: Some(UiActionId::new("grid.activate-row")),
                }),
            ),
            UiNode::new(
                "name",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            ),
            UiNode::new(
                "status",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Ready"),
                }),
            ),
        ],
    };
    runtime.mount_surface(&owner("feature"), snapshot)?;
    runtime.set_instance_active(&instance("feature"), true)?;

    let missing_grid = text_layer();
    runtime.register_instance(
        instance("layer"),
        scope(),
        Vec::new(),
        vec![OwnedUiLayerDescriptor {
            owner: owner("layer"),
            descriptor: missing_grid,
        }],
    )?;
    runtime.set_instance_active(&instance("layer"), true)?;
    assert_eq!(
        runtime.attach_registered_layer(owner("layer")),
        Err(UiError::UnsupportedCapability(String::from(
            UI_CAPABILITY_DATA_GRID
        )))
    );

    Ok(())
}

#[test]
fn test_should_reject_invalid_data_grid_layout() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let invalid = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "grid".into(),
        nodes: vec![
            UiNode::new(
                "grid",
                UiNodeKind::DataGrid(UiDataGridNode {
                    columns: vec![
                        UiDataGridColumn {
                            key: None,
                            label: String::from("Name"),
                            weight: 2,
                            sort_action: None,
                            sort_direction: None,
                        },
                        UiDataGridColumn {
                            key: None,
                            label: String::from("Version"),
                            weight: 1,
                            sort_action: None,
                            sort_direction: None,
                        },
                    ],
                    cells: vec!["name".into()],
                    selected_rows: Vec::new(),
                    row_keys: Vec::new(),
                    row_action: None,
                }),
            ),
            UiNode::new(
                "name",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_reject_data_grid_selected_row_out_of_bounds() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let invalid = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "grid".into(),
        nodes: vec![
            UiNode::new(
                "grid",
                UiNodeKind::DataGrid(UiDataGridNode {
                    columns: vec![UiDataGridColumn {
                        key: None,
                        label: String::from("Name"),
                        weight: 1,
                        sort_action: None,
                        sort_direction: None,
                    }],
                    cells: vec!["name".into()],
                    selected_rows: vec![1],
                    row_keys: Vec::new(),
                    row_action: None,
                }),
            ),
            UiNode::new(
                "name",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_reject_interactive_data_grid_without_row_keys() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let invalid = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "grid".into(),
        nodes: vec![
            UiNode::new(
                "grid",
                UiNodeKind::DataGrid(UiDataGridNode {
                    columns: vec![UiDataGridColumn {
                        key: Some(String::from("name")),
                        label: String::from("Name"),
                        weight: 1,
                        sort_action: None,
                        sort_direction: None,
                    }],
                    cells: vec!["name".into()],
                    selected_rows: Vec::new(),
                    row_keys: Vec::new(),
                    row_action: Some(UiActionId::new("grid.activate-row")),
                }),
            ),
            UiNode::new(
                "name",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_reject_sortable_data_grid_column_without_stable_key() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let invalid = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "grid".into(),
        nodes: vec![
            UiNode::new(
                "grid",
                UiNodeKind::DataGrid(UiDataGridNode {
                    columns: vec![UiDataGridColumn {
                        key: None,
                        label: String::from("Name"),
                        weight: 1,
                        sort_action: Some(UiActionId::new("grid.sort")),
                        sort_direction: None,
                    }],
                    cells: vec!["name".into()],
                    selected_rows: Vec::new(),
                    row_keys: Vec::new(),
                    row_action: None,
                }),
            ),
            UiNode::new(
                "name",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            ),
        ],
    };

    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_reject_image_and_icon_values_unsupported_by_capability_v1() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let invalid_image = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "image".into(),
        nodes: vec![UiNode::new(
            "image",
            UiNodeKind::Image(UiImageNode {
                media_type: String::from("image/svg+xml"),
                data_base64: String::from("PHN2Zz48L3N2Zz4="),
                alt: String::from("Example"),
                width: Some(32),
                height: Some(32),
            }),
        )],
    };
    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid_image),
        Err(UiError::InvalidLayout(_))
    ));

    let invalid_icon = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "icon".into(),
        nodes: vec![UiNode::new(
            "icon",
            UiNodeKind::Icon(UiIconNode {
                slot: UiIconSlotId::new("example.icon"),
                label: Some(String::from("Example")),
                size: Some(0),
            }),
        )],
    };
    assert!(matches!(
        runtime.mount_surface(&owner("feature"), invalid_icon),
        Err(UiError::InvalidLayout(_))
    ));
    Ok(())
}

#[test]
fn test_should_preserve_node_presentation_semantics_for_renderer_matching() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime, surface())?;

    let snapshot = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "content".into(),
        nodes: vec![
            UiNode::new(
                "content",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("Example"),
                }),
            )
            .with_semantic(ContractKey::new("example.content", ContractVersion::new(1)))
            .with_trait("primary-content"),
        ],
    };
    runtime.mount_surface(&owner("feature"), snapshot)?;
    runtime.set_instance_active(&instance("feature"), true)?;

    runtime.register_instance(
        instance("layer"),
        scope(),
        Vec::new(),
        vec![OwnedUiLayerDescriptor {
            owner: owner("layer"),
            descriptor: text_layer(),
        }],
    )?;
    runtime.set_instance_active(&instance("layer"), true)?;
    runtime.attach_registered_layer(owner("layer"))?;

    let surfaces = runtime.presentation_surfaces();
    let node = &surfaces[0].snapshot.nodes[0];
    assert_eq!(
        node.semantic.as_ref().map(ToString::to_string).as_deref(),
        Some("example.content@1")
    );
    assert_eq!(node.traits.len(), 1);
    assert_eq!(node.traits[0].as_str(), "primary-content");
    Ok(())
}
