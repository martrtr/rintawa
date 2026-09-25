//! Static surface declarations and UI Layer capability descriptions.

use serde::{Deserialize, Serialize};

use crate::contracts::ContractKey;

use crate::ui::{
    PORTABLE_UI_PROTOCOL_MAJOR, UiActivityId, UiCapabilityId, UiIconSlotId, UiSurfaceId,
    UiSurfaceTraitId,
};

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

/// Renderer-neutral activity metadata associated with one or more surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiActivityContribution {
    /// Stable activity identifier used for grouping related surfaces.
    pub id: UiActivityId,
    /// Human-readable label presented by shells.
    pub label: String,
    /// Optional semantic interface icon slot resolved by the active theme/icon pack.
    pub icon_slot: Option<UiIconSlotId>,
}

impl UiActivityContribution {
    /// Creates activity metadata with no icon preference.
    pub fn new(id: impl Into<UiActivityId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon_slot: None,
        }
    }

    /// Requests one semantic interface icon slot from the active shell theme.
    pub fn with_icon_slot(mut self, icon_slot: impl Into<UiIconSlotId>) -> Self {
        self.icon_slot = Some(icon_slot.into());
        self
    }
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
    /// Optional renderer-neutral activity metadata for shell launchers/tabs.
    #[serde(default)]
    pub activity: Option<UiActivityContribution>,
    /// Semantic presentation traits used by shell routing policies.
    #[serde(default)]
    pub traits: Vec<UiSurfaceTraitId>,
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
            activity: None,
            traits: Vec::new(),
            required_capabilities: Vec::new(),
        }
    }

    /// Assigns an optional domain semantic to this surface.
    pub fn with_semantic(mut self, semantic: ContractKey) -> Self {
        self.semantic = Some(semantic);
        self
    }

    /// Associates this surface with one renderer-neutral shell activity.
    pub fn with_activity(mut self, activity: UiActivityContribution) -> Self {
        self.activity = Some(activity);
        self
    }

    /// Adds one semantic shell-routing trait.
    pub fn with_trait(mut self, trait_id: impl Into<UiSurfaceTraitId>) -> Self {
        self.traits.push(trait_id.into());
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
