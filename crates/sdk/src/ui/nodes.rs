//! Typed nodes used by the portable UI protocol.

use serde::{Deserialize, Serialize};

use crate::contracts::ContractKey;

use super::{
    UI_CAPABILITY_BUTTON, UI_CAPABILITY_CHECKBOX, UI_CAPABILITY_COLUMN, UI_CAPABILITY_DATA_GRID,
    UI_CAPABILITY_ICON, UI_CAPABILITY_IMAGE, UI_CAPABILITY_LIST, UI_CAPABILITY_MARKDOWN,
    UI_CAPABILITY_ROW, UI_CAPABILITY_SELECT, UI_CAPABILITY_SPLIT, UI_CAPABILITY_TEXT,
    UI_CAPABILITY_TEXT_AREA, UI_CAPABILITY_TEXT_INPUT, UiActionId, UiActionPayload, UiCapabilityId,
    UiIconSlotId, UiNodeId, UiNodeSemanticTraitId,
};

/// One node in a portable UI surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    /// Stable identifier within the surface.
    pub id: UiNodeId,
    /// Optional versioned presentation semantic for renderer-local composition policies.
    #[serde(default)]
    pub semantic: Option<ContractKey>,
    /// Renderer-neutral presentation traits for safe theme/adapter matching.
    #[serde(default)]
    pub traits: Vec<UiNodeSemanticTraitId>,
    /// Renderer-neutral node data.
    pub kind: UiNodeKind,
}

impl UiNode {
    /// Creates a portable UI node.
    pub fn new(id: impl Into<UiNodeId>, kind: UiNodeKind) -> Self {
        Self {
            id: id.into(),
            semantic: None,
            traits: Vec::new(),
            kind,
        }
    }

    /// Assigns a versioned presentation semantic understood by interested UI Layers.
    pub fn with_semantic(mut self, semantic: ContractKey) -> Self {
        self.semantic = Some(semantic);
        self
    }

    /// Adds one renderer-neutral presentation trait.
    pub fn with_trait(mut self, trait_id: impl Into<UiNodeSemanticTraitId>) -> Self {
        self.traits.push(trait_id.into());
        self
    }
}

/// Renderer-neutral node variants supported by portable UI protocol v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum UiNodeKind {
    /// Plain text content.
    Text(UiTextNode),
    /// Markdown content rendered according to layer capabilities.
    Markdown(UiMarkdownNode),
    /// Clickable semantic action.
    Button(UiButtonNode),
    /// Semantic interface icon resolved by the active UI Layer/theme.
    Icon(UiIconNode),
    /// Verified raster image content.
    Image(UiImageNode),
    /// Boolean checkbox control.
    Checkbox(UiCheckboxNode),
    /// Single-value selection control.
    Select(UiSelectNode),
    /// Single-line text input.
    TextInput(UiTextInputNode),
    /// Multiline text input.
    TextArea(UiTextAreaNode),
    /// Weighted renderer-neutral split container.
    Split(UiSplitNode),
    /// Horizontal child container.
    Row(UiContainerNode),
    /// Vertical child container.
    Column(UiContainerNode),
    /// Repeating/list-like child container.
    List(UiContainerNode),
    /// Renderer-neutral tabular data with arbitrary portable UI cells.
    DataGrid(UiDataGridNode),
}

impl UiNodeKind {
    /// Returns the capability required to render this node kind.
    pub fn required_capability(&self) -> UiCapabilityId {
        let capability = match self {
            Self::Text(_) => UI_CAPABILITY_TEXT,
            Self::Markdown(_) => UI_CAPABILITY_MARKDOWN,
            Self::Button(_) => UI_CAPABILITY_BUTTON,
            Self::Icon(_) => UI_CAPABILITY_ICON,
            Self::Image(_) => UI_CAPABILITY_IMAGE,
            Self::Checkbox(_) => UI_CAPABILITY_CHECKBOX,
            Self::Select(_) => UI_CAPABILITY_SELECT,
            Self::TextInput(_) => UI_CAPABILITY_TEXT_INPUT,
            Self::TextArea(_) => UI_CAPABILITY_TEXT_AREA,
            Self::Split(_) => UI_CAPABILITY_SPLIT,
            Self::Row(_) => UI_CAPABILITY_ROW,
            Self::Column(_) => UI_CAPABILITY_COLUMN,
            Self::List(_) => UI_CAPABILITY_LIST,
            Self::DataGrid(_) => UI_CAPABILITY_DATA_GRID,
        };
        UiCapabilityId::new(capability)
    }

    /// Returns child references for container nodes.
    pub fn children(&self) -> &[UiNodeId] {
        match self {
            Self::Split(node) => &node.children,
            Self::Row(node) | Self::Column(node) | Self::List(node) => &node.children,
            Self::DataGrid(node) => &node.cells,
            _ => &[],
        }
    }

    /// Returns mutable child references for container nodes.
    pub fn children_mut(&mut self) -> Option<&mut Vec<UiNodeId>> {
        match self {
            Self::Split(node) => Some(&mut node.children),
            Self::Row(node) | Self::Column(node) | Self::List(node) => Some(&mut node.children),
            Self::DataGrid(node) => Some(&mut node.cells),
            _ => None,
        }
    }

    /// Returns whether this node exposes the supplied semantic action.
    pub fn has_action(&self, action: &UiActionId) -> bool {
        match self {
            Self::Button(node) => &node.action == action,
            Self::Checkbox(node) => &node.change_action == action,
            Self::Select(node) => &node.change_action == action,
            Self::TextInput(node) => {
                node.change_action.as_ref() == Some(action)
                    || node.submit_action.as_ref() == Some(action)
            }
            Self::TextArea(node) => {
                node.change_action.as_ref() == Some(action)
                    || node.submit_action.as_ref() == Some(action)
            }
            Self::DataGrid(node) => {
                node.row_action.as_ref() == Some(action)
                    || node
                        .columns
                        .iter()
                        .any(|column| column.sort_action.as_ref() == Some(action))
            }
            _ => false,
        }
    }

    /// Returns whether the supplied action is currently enabled.
    pub fn is_action_enabled(&self, action: &UiActionId) -> bool {
        match self {
            Self::Button(node) => &node.action == action && node.is_enabled,
            Self::Checkbox(node) => &node.change_action == action && node.is_enabled,
            Self::Select(node) => &node.change_action == action && node.is_enabled,
            Self::TextInput(node) => self.has_action(action) && node.is_enabled,
            Self::TextArea(node) => self.has_action(action) && node.is_enabled,
            Self::DataGrid(_) => self.has_action(action),
            _ => false,
        }
    }

    /// Returns whether the payload shape matches the supplied action.
    pub fn accepts_action_payload(&self, action: &UiActionId, payload: &UiActionPayload) -> bool {
        match self {
            Self::Button(node) if &node.action == action => {
                matches!(payload, UiActionPayload::None)
            }
            Self::Checkbox(node) if &node.change_action == action => {
                matches!(payload, UiActionPayload::Boolean(_))
            }
            Self::Select(node) if &node.change_action == action => {
                matches!(payload, UiActionPayload::Text(value) if node.options.iter().any(|option| option.value == *value))
            }
            Self::TextInput(_) | Self::TextArea(_) if self.has_action(action) => {
                matches!(payload, UiActionPayload::Text(_))
            }
            Self::DataGrid(node) => match payload {
                UiActionPayload::Text(value)
                    if node.row_action.as_ref() == Some(action)
                        && node.row_keys.iter().any(|key| key == value) =>
                {
                    true
                }
                UiActionPayload::Text(value) => node.columns.iter().any(|column| {
                    column.sort_action.as_ref() == Some(action)
                        && column.key.as_ref() == Some(value)
                }),
                UiActionPayload::None | UiActionPayload::Boolean(_) => false,
            },
            _ => false,
        }
    }
}

/// Plain text node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextNode {
    /// Text presented to the user.
    pub text: String,
}

/// Markdown node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiMarkdownNode {
    /// Markdown source presented by a supporting UI Layer.
    pub source: String,
}

/// Renderer-neutral visual emphasis requested for a button.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiButtonAppearance {
    /// Ordinary action with the theme's default emphasis.
    #[default]
    Default,
    /// Primary action for the current flow.
    Primary,
    /// Low-emphasis action that should read like inline chrome rather than a raised control.
    Subtle,
    /// Destructive action such as removing an installed item.
    Danger,
}

/// Button node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiButtonNode {
    /// User-visible button label.
    pub label: String,
    /// Semantic action emitted when activated.
    pub action: UiActionId,
    /// Whether the action can currently be activated.
    pub is_enabled: bool,
    /// Renderer-neutral visual emphasis interpreted by the active UI layer/theme.
    #[serde(default)]
    pub appearance: UiButtonAppearance,
}

/// Semantic interface icon node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiIconNode {
    /// Semantic icon slot resolved by the active UI Layer/theme.
    pub slot: UiIconSlotId,
    /// Optional accessible label.
    #[serde(default)]
    pub label: Option<String>,
    /// Optional renderer-neutral requested size in logical pixels.
    #[serde(default)]
    pub size: Option<u32>,
}

/// Verified raster image node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiImageNode {
    /// MIME type of the encoded raster payload.
    pub media_type: String,
    /// Base64-encoded verified image bytes.
    pub data_base64: String,
    /// Alternative text used when the image cannot be presented.
    pub alt: String,
    /// Optional renderer-neutral requested width in logical pixels.
    #[serde(default)]
    pub width: Option<u32>,
    /// Optional renderer-neutral requested height in logical pixels.
    #[serde(default)]
    pub height: Option<u32>,
}

/// Boolean checkbox control data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiCheckboxNode {
    /// User-visible control label.
    pub label: String,
    /// Current checked state.
    pub checked: bool,
    /// Semantic action emitted with a boolean payload when changed.
    pub change_action: UiActionId,
    /// Whether user interaction is currently enabled.
    pub is_enabled: bool,
}

/// One select option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSelectOption {
    /// Stable value emitted by the control.
    pub value: String,
    /// User-visible option label.
    pub label: String,
}

/// Single-value selection control data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSelectNode {
    /// Current selected value.
    pub value: String,
    /// Available values in renderer-neutral order.
    pub options: Vec<UiSelectOption>,
    /// Semantic action emitted with the selected text value when changed.
    pub change_action: UiActionId,
    /// Whether user interaction is currently enabled.
    pub is_enabled: bool,
}

/// Single-line text input node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextInputNode {
    /// Current presentation value.
    pub value: String,
    /// Optional renderer-neutral placeholder.
    pub placeholder: Option<String>,
    /// Optional action emitted as the value changes.
    pub change_action: Option<UiActionId>,
    /// Optional action emitted on submit.
    pub submit_action: Option<UiActionId>,
    /// Whether user input is currently enabled.
    pub is_enabled: bool,
}

/// Multiline text input node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTextAreaNode {
    /// Current presentation value.
    pub value: String,
    /// Optional renderer-neutral placeholder.
    pub placeholder: Option<String>,
    /// Optional action emitted as the value changes.
    pub change_action: Option<UiActionId>,
    /// Optional action emitted on submit.
    pub submit_action: Option<UiActionId>,
    /// Whether user input is currently enabled.
    pub is_enabled: bool,
}

/// Axis used by a weighted split container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiSplitAxis {
    /// Children are laid out from left to right.
    Horizontal,
    /// Children are laid out from top to bottom.
    Vertical,
}

/// Weighted split layout data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSplitNode {
    /// Ordered child node identifiers.
    pub children: Vec<UiNodeId>,
    /// Positive relative weights corresponding one-to-one with children.
    pub weights: Vec<u32>,
    /// Split orientation.
    pub axis: UiSplitAxis,
}

/// Sort direction advertised by a renderer-neutral data-grid column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiDataGridSortDirection {
    /// Smaller / earlier values are presented first.
    Ascending,
    /// Larger / later values are presented first.
    Descending,
}

/// One renderer-neutral data-grid column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiDataGridColumn {
    /// Stable feature-owned identity for this column.
    ///
    /// Renderers must not derive behavior from the localized label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// User-visible column heading.
    pub label: String,
    /// Positive relative width weight used by supporting renderers.
    pub weight: u32,
    /// Optional action emitted when a renderer requests sorting by this column.
    ///
    /// The emitted action carries the column key as a text payload. The feature
    /// remains the authoritative owner of the actual row ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_action: Option<UiActionId>,
    /// Current sort direction when this column owns the active ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<UiDataGridSortDirection>,
}

/// Renderer-neutral tabular layout using row-major portable UI cell references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiDataGridNode {
    /// Ordered column definitions.
    pub columns: Vec<UiDataGridColumn>,
    /// Row-major child node identifiers. The length must be divisible by the column count.
    pub cells: Vec<UiNodeId>,
    /// Zero-based rows that the feature marks as selected for presentation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_rows: Vec<u32>,
    /// Stable feature-owned identities corresponding one-to-one with rendered rows.
    ///
    /// A renderer uses these identities only as opaque payloads for row actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_keys: Vec<String>,
    /// Optional action emitted when the user activates a non-interactive part of a row.
    ///
    /// The emitted action carries the corresponding row key as a text payload.
    /// Interactive child controls retain their own actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_action: Option<UiActionId>,
}

/// Child references used by portable layout and list nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiContainerNode {
    /// Ordered child node identifiers.
    pub children: Vec<UiNodeId>,
}
