//! Typed nodes used by the portable UI protocol.

use serde::{Deserialize, Serialize};

use super::{
    UI_CAPABILITY_BUTTON, UI_CAPABILITY_COLUMN, UI_CAPABILITY_LIST, UI_CAPABILITY_MARKDOWN,
    UI_CAPABILITY_ROW, UI_CAPABILITY_TEXT, UI_CAPABILITY_TEXT_AREA, UI_CAPABILITY_TEXT_INPUT,
    UiActionId, UiActionPayload, UiCapabilityId, UiNodeId,
};

/// One node in a portable UI surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiNode {
    /// Stable identifier within the surface.
    pub id: UiNodeId,
    /// Renderer-neutral node data.
    pub kind: UiNodeKind,
}

impl UiNode {
    /// Creates a portable UI node.
    pub fn new(id: impl Into<UiNodeId>, kind: UiNodeKind) -> Self {
        Self {
            id: id.into(),
            kind,
        }
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
    /// Single-line text input.
    TextInput(UiTextInputNode),
    /// Multiline text input.
    TextArea(UiTextAreaNode),
    /// Horizontal child container.
    Row(UiContainerNode),
    /// Vertical child container.
    Column(UiContainerNode),
    /// Repeating/list-like child container.
    List(UiContainerNode),
}

impl UiNodeKind {
    /// Returns the capability required to render this node kind.
    pub fn required_capability(&self) -> UiCapabilityId {
        let capability = match self {
            Self::Text(_) => UI_CAPABILITY_TEXT,
            Self::Markdown(_) => UI_CAPABILITY_MARKDOWN,
            Self::Button(_) => UI_CAPABILITY_BUTTON,
            Self::TextInput(_) => UI_CAPABILITY_TEXT_INPUT,
            Self::TextArea(_) => UI_CAPABILITY_TEXT_AREA,
            Self::Row(_) => UI_CAPABILITY_ROW,
            Self::Column(_) => UI_CAPABILITY_COLUMN,
            Self::List(_) => UI_CAPABILITY_LIST,
        };
        UiCapabilityId::new(capability)
    }

    /// Returns child references for container nodes.
    pub fn children(&self) -> &[UiNodeId] {
        match self {
            Self::Row(node) | Self::Column(node) | Self::List(node) => &node.children,
            _ => &[],
        }
    }

    /// Returns mutable child references for container nodes.
    pub fn children_mut(&mut self) -> Option<&mut Vec<UiNodeId>> {
        match self {
            Self::Row(node) | Self::Column(node) | Self::List(node) => Some(&mut node.children),
            _ => None,
        }
    }

    /// Returns whether this node exposes the supplied semantic action.
    pub fn has_action(&self, action: &UiActionId) -> bool {
        match self {
            Self::Button(node) => &node.action == action,
            Self::TextInput(node) => {
                node.change_action.as_ref() == Some(action)
                    || node.submit_action.as_ref() == Some(action)
            }
            Self::TextArea(node) => {
                node.change_action.as_ref() == Some(action)
                    || node.submit_action.as_ref() == Some(action)
            }
            _ => false,
        }
    }

    /// Returns whether the supplied action is currently enabled.
    pub fn is_action_enabled(&self, action: &UiActionId) -> bool {
        match self {
            Self::Button(node) => &node.action == action && node.is_enabled,
            Self::TextInput(node) => self.has_action(action) && node.is_enabled,
            Self::TextArea(node) => self.has_action(action) && node.is_enabled,
            _ => false,
        }
    }

    /// Returns whether the payload shape matches the supplied action.
    pub fn accepts_action_payload(&self, action: &UiActionId, payload: &UiActionPayload) -> bool {
        match self {
            Self::Button(node) if &node.action == action => {
                matches!(payload, UiActionPayload::None)
            }
            Self::TextInput(_) | Self::TextArea(_) if self.has_action(action) => {
                matches!(payload, UiActionPayload::Text(_))
            }
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

/// Button node data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiButtonNode {
    /// User-visible button label.
    pub label: String,
    /// Semantic action emitted when activated.
    pub action: UiActionId,
    /// Whether the action can currently be activated.
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

/// Child references used by portable layout and list nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiContainerNode {
    /// Ordered child node identifiers.
    pub children: Vec<UiNodeId>,
}
