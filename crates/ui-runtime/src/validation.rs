//! Structural validation for renderer-neutral portable UI snapshots.

use std::collections::{HashMap, HashSet};

use rintawa_sdk::ui::{
    UiAssetImageNode, UiAssetPickerNode, UiCapabilityId, UiDataGridNode, UiError, UiIconNode,
    UiImageNode, UiNode, UiNodeId, UiNodeKind, UiResourcePickerNode, UiSelectNode, UiSplitNode,
    UiSurfaceSnapshot,
};

const MAX_LAYOUT_WEIGHT: u32 = 10_000;
const MAX_ICON_SIZE: u32 = 256;
const MAX_IMAGE_DIMENSION: u32 = 4096;
const MAX_IMAGE_BASE64_BYTES: usize = 1024 * 1024;
const MAX_ASSET_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ASSET_PICKER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ASSET_PICKER_MEDIA_TYPES: usize = 16;
const MAX_ASSET_PICKER_MEDIA_TYPE_BYTES: usize = 128;
const MAX_RESOURCE_PICKER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RESOURCE_PICKER_MEDIA_TYPES: usize = 16;
const MAX_RESOURCE_PICKER_EXTENSIONS: usize = 32;
const MAX_RESOURCE_PICKER_EXTENSION_BYTES: usize = 32;
const MAX_SELECT_OPTIONS: usize = 256;
const MAX_DATA_GRID_COLUMNS: usize = 64;

pub(crate) const DEFAULT_MAX_SURFACE_NODES: usize = 4096;
pub(crate) const DEFAULT_MAX_PATCH_OPERATIONS: usize = 1024;

pub(crate) fn validate_snapshot(snapshot: &UiSurfaceSnapshot) -> Result<(), UiError> {
    if snapshot.nodes.len() > DEFAULT_MAX_SURFACE_NODES {
        return Err(UiError::SurfaceTooLarge {
            actual: snapshot.nodes.len(),
            maximum: DEFAULT_MAX_SURFACE_NODES,
        });
    }

    let nodes = node_map(snapshot)?;
    if !nodes.contains_key(&snapshot.root) {
        return Err(UiError::RootNotFound(snapshot.root.to_string()));
    }

    let mut parent_of = HashMap::new();
    for node in nodes.values() {
        match &node.kind {
            UiNodeKind::Icon(icon) => validate_icon(&node.id, icon)?,
            UiNodeKind::Image(image) => validate_image(&node.id, image)?,
            UiNodeKind::AssetImage(image) => validate_asset_image(&node.id, image)?,
            UiNodeKind::AssetPicker(picker) => validate_asset_picker(&node.id, picker)?,
            UiNodeKind::ResourcePicker(picker) => validate_resource_picker(&node.id, picker)?,
            UiNodeKind::Select(select) => validate_select(&node.id, select)?,
            UiNodeKind::Split(split) => validate_split_layout(&node.id, split)?,
            UiNodeKind::DataGrid(grid) => validate_data_grid_layout(&node.id, grid)?,
            _ => {}
        }
        for child in node.kind.children() {
            if !nodes.contains_key(child) {
                return Err(UiError::NodeNotFound(child.to_string()));
            }
            if parent_of.insert(child.clone(), node.id.clone()).is_some() {
                return Err(UiError::MultipleParents(child.to_string()));
            }
        }
    }

    if parent_of.contains_key(&snapshot.root) {
        return Err(UiError::RootHasParent(snapshot.root.to_string()));
    }

    detect_cycles(&nodes, &parent_of)?;
    validate_reachability(snapshot, &nodes)?;
    Ok(())
}

fn validate_icon(node_id: &UiNodeId, icon: &UiIconNode) -> Result<(), UiError> {
    if icon
        .size
        .is_some_and(|size| size == 0 || size > MAX_ICON_SIZE)
    {
        return Err(UiError::InvalidLayout(format!(
            "icon node '{node_id}' requires requested size in 1..={MAX_ICON_SIZE}"
        )));
    }
    Ok(())
}

fn validate_image(node_id: &UiNodeId, image: &UiImageNode) -> Result<(), UiError> {
    if !matches!(image.media_type.as_str(), "image/png" | "image/webp") {
        return Err(UiError::InvalidLayout(format!(
            "image node '{node_id}' requires PNG or WebP media"
        )));
    }
    if image.data_base64.len() > MAX_IMAGE_BASE64_BYTES {
        return Err(UiError::InvalidLayout(format!(
            "image node '{node_id}' exceeds {MAX_IMAGE_BASE64_BYTES} encoded bytes"
        )));
    }
    if image
        .width
        .is_some_and(|width| width == 0 || width > MAX_IMAGE_DIMENSION)
        || image
            .height
            .is_some_and(|height| height == 0 || height > MAX_IMAGE_DIMENSION)
    {
        return Err(UiError::InvalidLayout(format!(
            "image node '{node_id}' requires requested dimensions in 1..={MAX_IMAGE_DIMENSION}"
        )));
    }
    Ok(())
}

fn validate_asset_image(node_id: &UiNodeId, image: &UiAssetImageNode) -> Result<(), UiError> {
    let Some(hex) = image.digest.strip_prefix("sha256:") else {
        return Err(UiError::InvalidLayout(format!(
            "asset-image node '{node_id}' requires canonical sha256 digest"
        )));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UiError::InvalidLayout(format!(
            "asset-image node '{node_id}' requires canonical sha256 digest"
        )));
    }
    if image.size == 0 || image.size > MAX_ASSET_IMAGE_BYTES {
        return Err(UiError::InvalidLayout(format!(
            "asset-image node '{node_id}' requires asset size in 1..={MAX_ASSET_IMAGE_BYTES}"
        )));
    }
    if !matches!(
        image.media_type.as_str(),
        "image/png" | "image/webp" | "image/jpeg"
    ) {
        return Err(UiError::InvalidLayout(format!(
            "asset-image node '{node_id}' requires PNG, WebP, or JPEG media"
        )));
    }
    if image
        .width
        .is_some_and(|width| width == 0 || width > MAX_IMAGE_DIMENSION)
        || image
            .height
            .is_some_and(|height| height == 0 || height > MAX_IMAGE_DIMENSION)
    {
        return Err(UiError::InvalidLayout(format!(
            "asset-image node '{node_id}' requires requested dimensions in 1..={MAX_IMAGE_DIMENSION}"
        )));
    }
    Ok(())
}

fn validate_asset_picker(node_id: &UiNodeId, picker: &UiAssetPickerNode) -> Result<(), UiError> {
    if picker.max_bytes == 0 || picker.max_bytes > MAX_ASSET_PICKER_BYTES {
        return Err(UiError::InvalidLayout(format!(
            "asset-picker node '{node_id}' requires max-bytes in 1..={MAX_ASSET_PICKER_BYTES}"
        )));
    }
    if picker.accepted_media_types.is_empty()
        || picker.accepted_media_types.len() > MAX_ASSET_PICKER_MEDIA_TYPES
    {
        return Err(UiError::InvalidLayout(format!(
            "asset-picker node '{node_id}' requires 1..={MAX_ASSET_PICKER_MEDIA_TYPES} media types"
        )));
    }
    let mut seen = HashSet::new();
    for media_type in &picker.accepted_media_types {
        if media_type.is_empty()
            || media_type.len() > MAX_ASSET_PICKER_MEDIA_TYPE_BYTES
            || media_type != &media_type.to_ascii_lowercase()
            || !media_type.contains('/')
            || !seen.insert(media_type)
        {
            return Err(UiError::InvalidLayout(format!(
                "asset-picker node '{node_id}' requires unique canonical lowercase media types"
            )));
        }
    }
    Ok(())
}

fn validate_resource_picker(
    node_id: &UiNodeId,
    picker: &UiResourcePickerNode,
) -> Result<(), UiError> {
    if picker.max_bytes == 0 || picker.max_bytes > MAX_RESOURCE_PICKER_BYTES {
        return Err(UiError::InvalidLayout(format!(
            "resource-picker node '{node_id}' requires max-bytes in 1..={MAX_RESOURCE_PICKER_BYTES}"
        )));
    }
    if picker.accepted_media_types.len() > MAX_RESOURCE_PICKER_MEDIA_TYPES
        || picker.accepted_extensions.len() > MAX_RESOURCE_PICKER_EXTENSIONS
        || (picker.accepted_media_types.is_empty() && picker.accepted_extensions.is_empty())
    {
        return Err(UiError::InvalidLayout(format!(
            "resource-picker node '{node_id}' requires at least one bounded media type or extension"
        )));
    }
    let mut seen_media_types = HashSet::new();
    for media_type in &picker.accepted_media_types {
        if media_type.is_empty()
            || media_type.len() > MAX_ASSET_PICKER_MEDIA_TYPE_BYTES
            || media_type != &media_type.to_ascii_lowercase()
            || !media_type.contains('/')
            || !seen_media_types.insert(media_type)
        {
            return Err(UiError::InvalidLayout(format!(
                "resource-picker node '{node_id}' requires unique canonical lowercase media types"
            )));
        }
    }
    let mut seen_extensions = HashSet::new();
    for extension in &picker.accepted_extensions {
        if extension.len() < 2
            || extension.len() > MAX_RESOURCE_PICKER_EXTENSION_BYTES
            || !extension.starts_with('.')
            || extension != &extension.to_ascii_lowercase()
            || !extension
                .bytes()
                .skip(1)
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || !seen_extensions.insert(extension)
        {
            return Err(UiError::InvalidLayout(format!(
                "resource-picker node '{node_id}' requires unique lowercase file extensions"
            )));
        }
    }
    Ok(())
}

fn validate_select(node_id: &UiNodeId, select: &UiSelectNode) -> Result<(), UiError> {
    if select.options.len() > MAX_SELECT_OPTIONS {
        return Err(UiError::InvalidLayout(format!(
            "select node '{node_id}' exceeds {MAX_SELECT_OPTIONS} options"
        )));
    }
    Ok(())
}

fn validate_split_layout(node_id: &UiNodeId, split: &UiSplitNode) -> Result<(), UiError> {
    if split.children.len() != split.weights.len() {
        return Err(UiError::InvalidLayout(format!(
            "split node '{node_id}' has {} children but {} weights",
            split.children.len(),
            split.weights.len()
        )));
    }
    if split
        .weights
        .iter()
        .any(|weight| *weight == 0 || *weight > MAX_LAYOUT_WEIGHT)
    {
        return Err(UiError::InvalidLayout(format!(
            "split node '{node_id}' requires weights in 1..={MAX_LAYOUT_WEIGHT}"
        )));
    }
    Ok(())
}

fn validate_data_grid_layout(node_id: &UiNodeId, grid: &UiDataGridNode) -> Result<(), UiError> {
    if grid.columns.is_empty() || grid.columns.len() > MAX_DATA_GRID_COLUMNS {
        return Err(UiError::InvalidLayout(format!(
            "data-grid node '{node_id}' requires 1..={MAX_DATA_GRID_COLUMNS} columns"
        )));
    }
    if grid
        .columns
        .iter()
        .any(|column| column.weight == 0 || column.weight > MAX_LAYOUT_WEIGHT)
    {
        return Err(UiError::InvalidLayout(format!(
            "data-grid node '{node_id}' requires column weights in 1..={MAX_LAYOUT_WEIGHT}"
        )));
    }

    let mut column_keys = HashSet::new();
    for column in &grid.columns {
        if let Some(key) = column.key.as_deref()
            && (key.is_empty() || !column_keys.insert(key))
        {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' requires non-empty unique column keys"
            )));
        }
        if column.sort_action.is_some() && column.key.is_none() {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' requires a column key for sortable columns"
            )));
        }
        if column.sort_direction.is_some() && column.sort_action.is_none() {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' cannot advertise sort direction without a sort action"
            )));
        }
    }
    if !grid.cells.len().is_multiple_of(grid.columns.len()) {
        return Err(UiError::InvalidLayout(format!(
            "data-grid node '{node_id}' has {} cells for {} columns",
            grid.cells.len(),
            grid.columns.len()
        )));
    }

    let row_count = grid.cells.len() / grid.columns.len();
    if grid.row_action.is_some() && grid.row_keys.len() != row_count {
        return Err(UiError::InvalidLayout(format!(
            "data-grid node '{node_id}' requires one row key per row when row activation is enabled"
        )));
    }
    if !grid.row_keys.is_empty() {
        if grid.row_keys.len() != row_count {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' has {} row keys for {row_count} rows",
                grid.row_keys.len()
            )));
        }
        let mut keys = HashSet::new();
        if grid
            .row_keys
            .iter()
            .any(|key| key.is_empty() || !keys.insert(key.as_str()))
        {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' requires non-empty unique row keys"
            )));
        }
    }

    let mut unique_rows = HashSet::new();
    for selected_row in &grid.selected_rows {
        let row_index = usize::try_from(*selected_row).map_err(|_| {
            UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' contains selected row {selected_row} outside this target"
            ))
        })?;
        if row_index >= row_count || !unique_rows.insert(*selected_row) {
            return Err(UiError::InvalidLayout(format!(
                "data-grid node '{node_id}' contains invalid selected row {selected_row}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn node_map(snapshot: &UiSurfaceSnapshot) -> Result<HashMap<UiNodeId, UiNode>, UiError> {
    let mut nodes = HashMap::with_capacity(snapshot.nodes.len());
    for node in &snapshot.nodes {
        if nodes.insert(node.id.clone(), node.clone()).is_some() {
            return Err(UiError::DuplicateNode(node.id.to_string()));
        }
    }
    Ok(nodes)
}

pub(crate) fn required_capabilities(snapshot: &UiSurfaceSnapshot) -> HashSet<UiCapabilityId> {
    snapshot
        .nodes
        .iter()
        .map(|node| node.kind.required_capability())
        .collect()
}

fn detect_cycles(
    nodes: &HashMap<UiNodeId, UiNode>,
    parent_of: &HashMap<UiNodeId, UiNodeId>,
) -> Result<(), UiError> {
    for start in nodes.keys() {
        let mut chain = HashSet::new();
        let mut current = start;
        while let Some(parent) = parent_of.get(current) {
            if !chain.insert(current.clone()) {
                return Err(UiError::CycleDetected(current.to_string()));
            }
            current = parent;
        }
    }
    Ok(())
}

fn validate_reachability(
    snapshot: &UiSurfaceSnapshot,
    nodes: &HashMap<UiNodeId, UiNode>,
) -> Result<(), UiError> {
    let mut visited = HashSet::new();
    let mut pending = vec![snapshot.root.clone()];
    while let Some(node_id) = pending.pop() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(&node_id) {
            pending.extend(node.kind.children().iter().cloned());
        }
    }

    if visited.len() != nodes.len() {
        let mut orphaned: Vec<_> = nodes.keys().filter(|id| !visited.contains(*id)).collect();
        orphaned.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        if let Some(orphan) = orphaned.first() {
            return Err(UiError::OrphanNode(orphan.to_string()));
        }
    }
    Ok(())
}
