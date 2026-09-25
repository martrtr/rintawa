//! Semantic user actions emitted by a UI Layer.

use serde::{Deserialize, Serialize};

use crate::types::ExtensionInstanceId;

use crate::ui::{UiActionId, UiNodeId, UiSurfaceId};

/// Typed payload carried by a portable UI action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum UiActionPayload {
    /// Action without an additional value.
    None,
    /// Text supplied by an input control.
    Text(String),
    /// Boolean supplied by a toggle control.
    Boolean(bool),
}

/// One semantic action emitted by the active UI Layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiActionEvent {
    /// Runtime instance that owns the surface that produced the action.
    pub owner_instance_id: ExtensionInstanceId,
    /// Surface that produced the action within its owning instance.
    pub surface_id: UiSurfaceId,
    /// Node that produced the action.
    pub node_id: UiNodeId,
    /// Action bound to the node.
    pub action_id: UiActionId,
    /// Surface revision rendered when the action was emitted.
    pub surface_revision: u64,
    /// Typed action payload.
    pub payload: UiActionPayload,
}
