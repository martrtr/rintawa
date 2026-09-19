//! Renderer-neutral portable UI protocol shared by extensions and UI Layers.

mod actions;
mod errors;
mod ids;
mod nodes;
mod patches;
mod surfaces;

pub use actions::{UiActionEvent, UiActionPayload};
pub use errors::{UiError, UiResult};
pub use ids::{
    PORTABLE_UI_PROTOCOL_MAJOR, UI_CAPABILITY_BUTTON, UI_CAPABILITY_CHECKBOX, UI_CAPABILITY_COLUMN,
    UI_CAPABILITY_DATA_GRID, UI_CAPABILITY_ICON, UI_CAPABILITY_IMAGE, UI_CAPABILITY_LIST,
    UI_CAPABILITY_MARKDOWN, UI_CAPABILITY_ROW, UI_CAPABILITY_SELECT, UI_CAPABILITY_SPLIT,
    UI_CAPABILITY_TEXT, UI_CAPABILITY_TEXT_AREA, UI_CAPABILITY_TEXT_INPUT, UiActionId,
    UiActivityId, UiCapabilityId, UiIconSlotId, UiNodeId, UiNodeSemanticTraitId, UiSurfaceId,
    UiSurfaceTraitId,
};
pub use nodes::{
    UiButtonAppearance, UiButtonNode, UiCheckboxNode, UiContainerNode, UiDataGridColumn,
    UiDataGridNode, UiDataGridSortDirection, UiIconNode, UiImageNode, UiMarkdownNode, UiNode,
    UiNodeKind, UiSelectNode, UiSelectOption, UiSplitAxis, UiSplitNode, UiTextAreaNode,
    UiTextInputNode, UiTextNode,
};
pub use patches::{UiPatch, UiPatchBatch, UiSurfaceSnapshot};
pub use surfaces::{
    UiActivityContribution, UiLayerDescriptor, UiPlacementHint, UiSurfaceContribution,
};
