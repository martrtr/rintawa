//! JSON-compatible Web UI bridge messages.

use std::collections::HashSet;

use rintawa_sdk::{
    contracts::ComponentRef,
    types::ExtensionInstanceId,
    ui::{
        PORTABLE_UI_PROTOCOL_MAJOR, UiActionEvent, UiActionId, UiActionPayload, UiNode, UiNodeId,
        UiSurfaceContribution, UiSurfaceId,
    },
};
use rintawa_ui_runtime::UiPresentationSurface;
use serde::{Deserialize, Serialize};

use crate::{WEB_UI_BRIDGE_PROTOCOL_MAJOR, WebBundleDescriptor, WebRuntimeError, WebRuntimeResult};

/// Presentation snapshot encoded for JavaScript without losing `u64` revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSurfaceSnapshot {
    /// Surface receiving the snapshot.
    pub surface_id: UiSurfaceId,
    /// Exact decimal `u64` surface revision.
    pub revision: String,
    /// Root node identifier.
    pub root: UiNodeId,
    /// Flat portable UI node table.
    pub nodes: Vec<UiNode>,
}

/// Portable UI surface sent from a Rintawa host to a Web renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiPresentationSurface {
    /// Component that owns the surface.
    pub owner: ComponentRef,
    /// Static surface metadata.
    pub contribution: UiSurfaceContribution,
    /// Current presentation snapshot.
    pub snapshot: WebUiSurfaceSnapshot,
}

impl From<UiPresentationSurface> for WebUiPresentationSurface {
    fn from(surface: UiPresentationSurface) -> Self {
        Self {
            owner: surface.owner,
            contribution: surface.contribution,
            snapshot: WebUiSurfaceSnapshot {
                surface_id: surface.snapshot.surface_id,
                revision: surface.snapshot.revision.to_string(),
                root: surface.snapshot.root,
                nodes: surface.snapshot.nodes,
            },
        }
    }
}

/// Semantic action encoded by a Web renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiActionEvent {
    /// Runtime instance that owns the source surface.
    pub owner_instance_id: ExtensionInstanceId,
    /// Surface that produced the action.
    pub surface_id: UiSurfaceId,
    /// Node that produced the action.
    pub node_id: UiNodeId,
    /// Semantic action identifier.
    pub action_id: UiActionId,
    /// Exact decimal `u64` revision rendered by the Web UI.
    pub surface_revision: String,
    /// Typed portable UI action payload.
    pub payload: UiActionPayload,
}

impl TryFrom<WebUiActionEvent> for UiActionEvent {
    type Error = WebRuntimeError;

    fn try_from(event: WebUiActionEvent) -> Result<Self, Self::Error> {
        let surface_revision = event
            .surface_revision
            .parse::<u64>()
            .map_err(|_| WebRuntimeError::InvalidSurfaceRevision(event.surface_revision.clone()))?;
        Ok(Self {
            owner_instance_id: event.owner_instance_id,
            surface_id: event.surface_id,
            node_id: event.node_id,
            action_id: event.action_id,
            surface_revision,
            payload: event.payload,
        })
    }
}

/// Message emitted by the Web renderer toward its host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RendererToHostMessage {
    /// Initial protocol and capability handshake.
    Hello {
        /// Web bridge protocol major version.
        protocol_major: u32,
        /// Portable UI protocol major version.
        portable_ui_protocol_major: u32,
        /// Capabilities implemented by the renderer.
        capabilities: Vec<String>,
    },
    /// Semantic user action from a rendered portable UI surface.
    Action {
        /// Web bridge protocol major version.
        protocol_major: u32,
        /// Action to validate and route through the Extension Engine.
        event: WebUiActionEvent,
    },
}

impl RendererToHostMessage {
    /// Validates the bridge protocol and packaged layer capabilities.
    ///
    /// # Errors
    ///
    /// Returns an error for incompatible protocol versions or capabilities.
    pub fn validate(&self, descriptor: &WebBundleDescriptor) -> WebRuntimeResult<()> {
        match self {
            Self::Hello {
                protocol_major,
                portable_ui_protocol_major,
                capabilities,
            } => {
                if *protocol_major != WEB_UI_BRIDGE_PROTOCOL_MAJOR {
                    return Err(WebRuntimeError::UnsupportedBridgeProtocol(*protocol_major));
                }
                if *portable_ui_protocol_major != PORTABLE_UI_PROTOCOL_MAJOR {
                    return Err(WebRuntimeError::UnsupportedPortableUiProtocol(
                        *portable_ui_protocol_major,
                    ));
                }
                let actual: HashSet<_> = capabilities.iter().map(String::as_str).collect();
                let expected: HashSet<_> = descriptor
                    .ui_layer
                    .as_ref()
                    .map(|ui_layer| {
                        ui_layer
                            .capabilities
                            .iter()
                            .map(|capability| capability.as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                if actual != expected || actual.len() != capabilities.len() {
                    return Err(WebRuntimeError::CapabilityMismatch);
                }
            }
            Self::Action { protocol_major, .. } => {
                if *protocol_major != WEB_UI_BRIDGE_PROTOCOL_MAJOR {
                    return Err(WebRuntimeError::UnsupportedBridgeProtocol(*protocol_major));
                }
            }
        }
        Ok(())
    }
}

/// Message emitted by a Rintawa host toward the Web renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HostToRendererMessage {
    /// Complete current portable UI presentation visible to this layer.
    State {
        /// Web bridge protocol major version.
        protocol_major: u32,
        /// Current visible portable UI surfaces.
        surfaces: Vec<WebUiPresentationSurface>,
    },
    /// Host-side transport or validation error.
    Error {
        /// Web bridge protocol major version.
        protocol_major: u32,
        /// Stable machine-readable error code.
        code: String,
        /// Human-readable diagnostic message.
        message: String,
    },
}

impl HostToRendererMessage {
    /// Creates one complete portable UI state message.
    pub fn state(surfaces: Vec<UiPresentationSurface>) -> Self {
        Self::State {
            protocol_major: WEB_UI_BRIDGE_PROTOCOL_MAJOR,
            surfaces: surfaces.into_iter().map(Into::into).collect(),
        }
    }

    /// Creates a host error message.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            protocol_major: WEB_UI_BRIDGE_PROTOCOL_MAJOR,
            code: code.into(),
            message: message.into(),
        }
    }
}
