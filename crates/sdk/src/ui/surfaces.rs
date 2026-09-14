//! Static surface declarations and UI Layer capability descriptions.

use serde::{Deserialize, Serialize};

use crate::contracts::ContractKey;

use super::{PORTABLE_UI_PROTOCOL_MAJOR, UiCapabilityId, UiSurfaceId};

/// Abstract placement requested for a portable surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiPlacementHint {
    /// Primary application content.
    Primary,
    /// Secondary application content.
    Secondary,
    /// Sidebar-like supporting content.
    Sidebar,
    /// Settings or preferences content.
    Settings,
    /// Modal or dialog-like content.
    Dialog,
    /// Compact status presentation.
    Status,
    /// Overlay presentation above normal content.
    Overlay,
}

/// Declares one portable UI surface owned by a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSurfaceContribution {
    /// Stable surface identifier.
    pub id: UiSurfaceId,
    /// Renderer-neutral placement hint.
    pub placement: UiPlacementHint,
    /// Optional domain semantic used by future specialized renderers.
    pub semantic: Option<ContractKey>,
    /// Additional capabilities required before this surface may be mounted.
    #[serde(default)]
    pub required_capabilities: Vec<UiCapabilityId>,
}

impl UiSurfaceContribution {
    /// Creates a portable surface declaration.
    pub fn new(id: impl Into<UiSurfaceId>, placement: UiPlacementHint) -> Self {
        Self {
            id: id.into(),
            placement,
            semantic: None,
            required_capabilities: Vec::new(),
        }
    }

    /// Assigns an optional domain semantic to this surface.
    pub fn with_semantic(mut self, semantic: ContractKey) -> Self {
        self.semantic = Some(semantic);
        self
    }

    /// Requires one presentation capability from the active UI Layer.
    pub fn requiring_capability(mut self, capability: impl Into<UiCapabilityId>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }
}

/// Describes the portable UI capabilities implemented by one UI Layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiLayerDescriptor {
    /// Portable UI protocol major version understood by the layer.
    pub protocol_major: u32,
    /// Presentation capabilities implemented by the layer.
    pub capabilities: Vec<UiCapabilityId>,
}

impl UiLayerDescriptor {
    /// Creates a descriptor for the current portable UI protocol.
    pub fn new(capabilities: Vec<UiCapabilityId>) -> Self {
        Self {
            protocol_major: PORTABLE_UI_PROTOCOL_MAJOR,
            capabilities,
        }
    }
}
