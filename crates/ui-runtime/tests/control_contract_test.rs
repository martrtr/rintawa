//! Integration tests for portable control actions and payload validation.

use anyhow::Result;
use rintawa_sdk::{
    contracts::ComponentRef,
    types::{ExtensionInstanceId, RuntimeScopeId},
    ui::{
        UI_CAPABILITY_CHECKBOX, UI_CAPABILITY_COLUMN, UI_CAPABILITY_DATA_GRID,
        UI_CAPABILITY_SELECT, UI_CAPABILITY_TEXT, UiActionEvent, UiActionId, UiActionPayload,
        UiCapabilityId, UiCheckboxNode, UiContainerNode, UiDataGridColumn, UiDataGridNode,
        UiDataGridSortDirection, UiError, UiLayerDescriptor, UiNode, UiNodeKind, UiPlacementHint,
        UiSelectNode, UiSelectOption, UiSurfaceContribution, UiSurfaceId, UiSurfaceSnapshot,
        UiTextNode,
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
fn register_feature(runtime: &UiRuntime) -> Result<()> {
    runtime.register_instance(
        instance("feature"),
        scope(),
        vec![OwnedUiSurfaceContribution {
            owner: owner("feature"),
            contribution: UiSurfaceContribution::new("example.main", UiPlacementHint::Primary),
        }],
        Vec::new(),
    )?;
    Ok(())
}

fn attach_control_layer(runtime: &UiRuntime) -> Result<()> {
    let descriptor = UiLayerDescriptor::new(vec![
        UiCapabilityId::new(UI_CAPABILITY_COLUMN),
        UiCapabilityId::new(UI_CAPABILITY_CHECKBOX),
        UiCapabilityId::new(UI_CAPABILITY_SELECT),
    ]);
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

fn controls_snapshot() -> UiSurfaceSnapshot {
    UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: "root".into(),
        nodes: vec![
            UiNode::new(
                "root",
                UiNodeKind::Column(UiContainerNode {
                    children: vec!["checkbox".into(), "select".into()],
                }),
            ),
            UiNode::new(
                "checkbox",
                UiNodeKind::Checkbox(UiCheckboxNode {
                    label: String::from("Enabled"),
                    checked: false,
                    change_action: UiActionId::new("example.toggle"),
                    is_enabled: true,
                }),
            ),
            UiNode::new(
                "select",
                UiNodeKind::Select(UiSelectNode {
                    value: String::from("stable"),
                    options: vec![
                        UiSelectOption {
                            value: String::from("stable"),
                            label: String::from("Stable"),
                        },
                        UiSelectOption {
                            value: String::from("beta"),
                            label: String::from("Beta"),
                        },
                    ],
                    change_action: UiActionId::new("example.channel"),
                    is_enabled: true,
                }),
            ),
        ],
    }
}
#[test]
fn test_should_validate_checkbox_and_select_action_payloads() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime)?;
    runtime.mount_surface(&owner("feature"), controls_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_control_layer(&runtime)?;

    let checkbox = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "checkbox".into(),
        action_id: UiActionId::new("example.toggle"),
        surface_revision: 1,
        payload: UiActionPayload::Boolean(true),
    };
    runtime.route_action(&owner("layer"), checkbox.clone())?;

    let mut invalid_checkbox = checkbox;
    invalid_checkbox.payload = UiActionPayload::Text(String::from("true"));
    assert!(matches!(
        runtime.route_action(&owner("layer"), invalid_checkbox),
        Err(UiError::InvalidActionPayload { .. })
    ));
    let select = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "select".into(),
        action_id: UiActionId::new("example.channel"),
        surface_revision: 1,
        payload: UiActionPayload::Text(String::from("beta")),
    };
    runtime.route_action(&owner("layer"), select.clone())?;

    let mut invalid_select = select;
    invalid_select.payload = UiActionPayload::Text(String::from("nightly"));
    assert!(matches!(
        runtime.route_action(&owner("layer"), invalid_select),
        Err(UiError::InvalidActionPayload { .. })
    ));
    Ok(())
}

fn attach_data_grid_layer(runtime: &UiRuntime) -> Result<()> {
    let descriptor = UiLayerDescriptor::new(vec![
        UiCapabilityId::new(UI_CAPABILITY_DATA_GRID),
        UiCapabilityId::new(UI_CAPABILITY_TEXT),
    ]);
    runtime.register_instance(
        instance("grid-layer"),
        scope(),
        Vec::new(),
        vec![OwnedUiLayerDescriptor {
            owner: owner("grid-layer"),
            descriptor,
        }],
    )?;
    runtime.set_instance_active(&instance("grid-layer"), true)?;
    runtime.attach_registered_layer(owner("grid-layer"))?;
    Ok(())
}

fn data_grid_snapshot() -> UiSurfaceSnapshot {
    UiSurfaceSnapshot {
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
                        sort_action: Some(UiActionId::new("grid.sort")),
                        sort_direction: Some(UiDataGridSortDirection::Ascending),
                    }],
                    cells: vec!["name".into()],
                    selected_rows: Vec::new(),
                    row_keys: vec![String::from("package.example")],
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
    }
}

#[test]
fn test_should_validate_data_grid_row_and_sort_action_payloads() -> Result<()> {
    let runtime = UiRuntime::new();
    register_feature(&runtime)?;
    runtime.mount_surface(&owner("feature"), data_grid_snapshot())?;
    runtime.set_instance_active(&instance("feature"), true)?;
    attach_data_grid_layer(&runtime)?;

    let row = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "grid".into(),
        action_id: UiActionId::new("grid.activate-row"),
        surface_revision: 1,
        payload: UiActionPayload::Text(String::from("package.example")),
    };
    runtime.route_action(&owner("grid-layer"), row.clone())?;

    let mut invalid_row = row;
    invalid_row.payload = UiActionPayload::Text(String::from("package.missing"));
    assert!(matches!(
        runtime.route_action(&owner("grid-layer"), invalid_row),
        Err(UiError::InvalidActionPayload { .. })
    ));

    let sort = UiActionEvent {
        owner_instance_id: instance("feature"),
        surface_id: UiSurfaceId::new("example.main"),
        node_id: "grid".into(),
        action_id: UiActionId::new("grid.sort"),
        surface_revision: 1,
        payload: UiActionPayload::Text(String::from("name")),
    };
    runtime.route_action(&owner("grid-layer"), sort.clone())?;

    let mut invalid_sort = sort;
    invalid_sort.payload = UiActionPayload::Text(String::from("translated-name"));
    assert!(matches!(
        runtime.route_action(&owner("grid-layer"), invalid_sort),
        Err(UiError::InvalidActionPayload { .. })
    ));
    Ok(())
}
