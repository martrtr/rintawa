//! Semantic user actions emitted by a UI Layer.

use serde::{Deserialize, Serialize};

use crate::types::ExtensionInstanceId;

use crate::ui::{UiActionId, UiNodeId, UiSurfaceId};

/// Immutable Host-issued asset selected through a renderer-neutral asset picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiActionAssetRef {
    /// Canonical `sha256:<lowercase-hex>` identity of the exact bytes.
    pub digest: String,
    /// Exact immutable asset byte length.
    pub size: u64,
    /// Canonical media type returned by the Host asset store.
    pub media_type: String,
    /// Original user-facing file name, when supplied by the UI Layer.
    pub name: Option<String>,
}

/// Opaque HostRuntime-local resource selected through a renderer-neutral file picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiActionUserResourceRef {
    /// Unguessable Host-issued bearer identity.
    pub id: String,
    /// Exact selected byte length.
    pub size: u64,
    /// Canonical media type retained by the Host.
    pub media_type: String,
    /// Original user-facing file name when available.
    pub name: Option<String>,
}

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
    /// Immutable asset imported by the trusted UI Layer after explicit user selection.
    Asset(UiActionAssetRef),
    /// Ephemeral resource imported by the trusted UI Layer for one semantic workflow.
    Resource(UiActionUserResourceRef),
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
