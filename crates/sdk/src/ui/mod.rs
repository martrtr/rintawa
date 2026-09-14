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
    PORTABLE_UI_PROTOCOL_MAJOR, UI_CAPABILITY_BUTTON, UI_CAPABILITY_COLUMN, UI_CAPABILITY_LIST,
    UI_CAPABILITY_MARKDOWN, UI_CAPABILITY_ROW, UI_CAPABILITY_TEXT, UI_CAPABILITY_TEXT_AREA,
    UI_CAPABILITY_TEXT_INPUT, UiActionId, UiCapabilityId, UiNodeId, UiSurfaceId,
};
pub use nodes::{
    UiButtonNode, UiContainerNode, UiMarkdownNode, UiNode, UiNodeKind, UiTextAreaNode,
    UiTextInputNode, UiTextNode,
};
pub use patches::{UiPatch, UiPatchBatch, UiSurfaceSnapshot};
pub use surfaces::{UiLayerDescriptor, UiPlacementHint, UiSurfaceContribution};
