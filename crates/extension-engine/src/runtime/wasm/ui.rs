//! Portable UI and UI-layer WASM host adapters.

use std::io::Write;

use rintawa_sdk::{
    contracts::{ContractKey, ContractVersion},
    ui::{
        UiActionEvent, UiActivityContribution, UiActivityId, UiCapabilityId, UiError, UiIconSlotId,
        UiLayerDescriptor, UiPatchBatch, UiPlacementHint, UiSurfaceContribution, UiSurfaceId,
        UiSurfaceSnapshot, UiSurfaceTraitId,
    },
};
use tracing::warn;

use crate::runtime::wasm::{
    PortableUiError, PortableUiHost, UiLayerError, UiLayerHost, WasmHostState, WitActivity,
    WitPlacementHint,
};

#[derive(Debug)]
struct UiSurfaceRegistrationRequest {
    id: String,
    placement: WitPlacementHint,
    semantic_contract: Option<String>,
    semantic_version: Option<u32>,
    activity: Option<WitActivity>,
    traits: Vec<String>,
    required_capabilities: Vec<String>,
}

#[derive(Default)]
struct JsonSizeCounter {
    bytes: usize,
}

impl Write for JsonSizeCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl WasmHostState {
    fn queue_ui_surface(
        &mut self,
        request: UiSurfaceRegistrationRequest,
    ) -> Result<(), PortableUiError> {
        let UiSurfaceRegistrationRequest {
            id,
            placement,
            semantic_contract,
            semantic_version,
            activity,
            traits,
            required_capabilities,
        } = request;
        let semantic = match (semantic_contract, semantic_version) {
            (Some(contract), Some(version)) => {
                Some(ContractKey::new(contract, ContractVersion::new(version)))
            }
            (None, None) => None,
            _ => return Err(PortableUiError::InvalidPayload),
        };
        let activity = match activity {
            Some(activity) => {
                if activity.id.trim().is_empty()
                    || activity.label.trim().is_empty()
                    || activity
                        .icon_slot
                        .as_deref()
                        .is_some_and(|slot| slot.trim().is_empty())
                {
                    return Err(PortableUiError::InvalidPayload);
                }
                Some(UiActivityContribution {
                    id: UiActivityId::new(activity.id),
                    label: activity.label,
                    icon_slot: activity.icon_slot.map(UiIconSlotId::new),
                })
            }
            None => None,
        };
        if traits.iter().any(|trait_id| trait_id.trim().is_empty()) {
            return Err(PortableUiError::InvalidPayload);
        }
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(PortableUiError::RegistrationNotActive);
        };
        let id = UiSurfaceId::new(id);
        if !scope.ui_surface_ids.insert(id.clone()) {
            return Err(PortableUiError::DuplicateSurface);
        }
        let placement = match placement {
            WitPlacementHint::Primary => UiPlacementHint::Primary,
            WitPlacementHint::Secondary => UiPlacementHint::Secondary,
            WitPlacementHint::Sidebar => UiPlacementHint::Sidebar,
            WitPlacementHint::Settings => UiPlacementHint::Settings,
            WitPlacementHint::Dialog => UiPlacementHint::Dialog,
            WitPlacementHint::Status => UiPlacementHint::Status,
            WitPlacementHint::Overlay => UiPlacementHint::Overlay,
        };
        scope.ui_surfaces.push(UiSurfaceContribution {
            id,
            placement,
            semantic,
            activity,
            traits: traits.into_iter().map(UiSurfaceTraitId::new).collect(),
            required_capabilities: required_capabilities
                .into_iter()
                .map(UiCapabilityId::new)
                .collect(),
        });
        Ok(())
    }

    fn queue_ui_layer(
        &mut self,
        protocol_major: u32,
        capabilities: Vec<String>,
    ) -> Result<(), PortableUiError> {
        let mut counter = JsonSizeCounter::default();
        serde_json::to_writer(&mut counter, &(protocol_major, &capabilities))
            .map_err(|_| PortableUiError::InvalidPayload)?;
        self.validate_ui_message_size(counter.bytes)?;
        let Some(scope) = self.registration_scope.as_mut() else {
            return Err(PortableUiError::RegistrationNotActive);
        };
        if scope.ui_layer.is_some() {
            return Err(PortableUiError::DuplicateLayer);
        }
        scope.ui_layer = Some(UiLayerDescriptor {
            protocol_major,
            capabilities: capabilities.into_iter().map(UiCapabilityId::new).collect(),
        });
        Ok(())
    }

    fn ui_owner(&self) -> Result<rintawa_sdk::contracts::ComponentRef, PortableUiError> {
        self.current_execution_owner()
            .cloned()
            .ok_or(PortableUiError::Unavailable)
    }

    fn validate_ui_message_size(&self, message_bytes: usize) -> Result<(), PortableUiError> {
        if message_bytes > self.max_host_message_bytes {
            return Err(PortableUiError::MessageTooLarge);
        }
        Ok(())
    }

    fn validate_ui_registration_size(
        &self,
        request: &UiSurfaceRegistrationRequest,
    ) -> Result<(), PortableUiError> {
        let placement = match request.placement {
            WitPlacementHint::Primary => "primary",
            WitPlacementHint::Secondary => "secondary",
            WitPlacementHint::Sidebar => "sidebar",
            WitPlacementHint::Settings => "settings",
            WitPlacementHint::Dialog => "dialog",
            WitPlacementHint::Status => "status",
            WitPlacementHint::Overlay => "overlay",
        };
        let activity_size = request.activity.as_ref().map(|activity| {
            (
                activity.id.as_str(),
                activity.label.as_str(),
                activity.icon_slot.as_deref(),
            )
        });
        let mut counter = JsonSizeCounter::default();
        serde_json::to_writer(
            &mut counter,
            &(
                request.id.as_str(),
                placement,
                request.semantic_contract.as_deref(),
                request.semantic_version,
                activity_size,
                &request.traits,
                &request.required_capabilities,
            ),
        )
        .map_err(|_| PortableUiError::InvalidPayload)?;
        self.validate_ui_message_size(counter.bytes)
    }

    fn map_ui_error(error: UiError) -> PortableUiError {
        match error {
            UiError::UnsupportedCapability(capability) => {
                PortableUiError::UnsupportedCapability(capability)
            }
            UiError::RuntimeUnavailable => PortableUiError::Unavailable,
            _ => PortableUiError::Rejected,
        }
    }
}

impl PortableUiHost for WasmHostState {
    fn register_surface(
        &mut self,
        id: String,
        placement: WitPlacementHint,
        semantic_contract: Option<String>,
        semantic_version: Option<u32>,
        activity: Option<WitActivity>,
        traits: Vec<String>,
        required_capabilities: Vec<String>,
    ) -> Result<(), PortableUiError> {
        let request = UiSurfaceRegistrationRequest {
            id,
            placement,
            semantic_contract,
            semantic_version,
            activity,
            traits,
            required_capabilities,
        };
        self.validate_ui_registration_size(&request)?;
        self.queue_ui_surface(request)
    }

    fn register_layer(
        &mut self,
        protocol_major: u32,
        capabilities: Vec<String>,
    ) -> Result<(), PortableUiError> {
        self.queue_ui_layer(protocol_major, capabilities)
    }

    fn mount_surface(&mut self, snapshot_json: Vec<u8>) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(snapshot_json.len())?;
        let snapshot: UiSurfaceSnapshot =
            serde_json::from_slice(&snapshot_json).map_err(|error| {
                warn!(
                    plugin = %self.component_id,
                    error = %error,
                    "Rejecting invalid portable UI snapshot JSON"
                );
                PortableUiError::InvalidPayload
            })?;
        let owner = self.ui_owner()?;
        self.ui
            .mount_surface(&owner, snapshot)
            .map_err(Self::map_ui_error)
    }

    fn patch_surface(&mut self, batch_json: Vec<u8>) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(batch_json.len())?;
        let batch: UiPatchBatch = serde_json::from_slice(&batch_json).map_err(|error| {
            warn!(
                plugin = %self.component_id,
                error = %error,
                "Rejecting invalid portable UI patch JSON"
            );
            PortableUiError::InvalidPayload
        })?;
        let owner = self.ui_owner()?;
        self.ui
            .apply_patches(&owner, batch)
            .map_err(Self::map_ui_error)
    }

    fn unmount_surface(&mut self, surface_id: String) -> Result<(), PortableUiError> {
        if !self.ui_access_active {
            return Err(PortableUiError::Unavailable);
        }
        self.validate_ui_message_size(surface_id.len())?;
        let owner = self.ui_owner()?;
        self.ui
            .unmount_surface(&owner, &UiSurfaceId::new(surface_id))
            .map_err(Self::map_ui_error)
    }
}

impl UiLayerHost for WasmHostState {
    fn presentation_surfaces(&mut self) -> Result<Vec<u8>, UiLayerError> {
        if !self.ui_access_active {
            return Err(UiLayerError::AccessNotActive);
        }
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(UiLayerError::AccessNotActive)?;
        let surfaces =
            self.ui
                .presentation_surfaces_for_layer(&owner)
                .map_err(|error| match error {
                    UiError::LayerNotOwner | UiError::LayerNotRegistered => {
                        UiLayerError::NotActiveLayer
                    }
                    UiError::RuntimeUnavailable => UiLayerError::Unavailable,
                    _ => UiLayerError::Rejected,
                })?;
        let mut counter = JsonSizeCounter::default();
        serde_json::to_writer(&mut counter, &surfaces).map_err(|_| UiLayerError::Unavailable)?;
        if counter.bytes > self.max_host_message_bytes {
            return Err(UiLayerError::MessageTooLarge);
        }
        serde_json::to_vec(&surfaces).map_err(|_| UiLayerError::Unavailable)
    }

    fn dispatch_action(&mut self, action_json: Vec<u8>) -> Result<(), UiLayerError> {
        if !self.ui_access_active {
            return Err(UiLayerError::AccessNotActive);
        }
        if action_json.len() > self.max_host_message_bytes {
            return Err(UiLayerError::MessageTooLarge);
        }
        let event: UiActionEvent =
            serde_json::from_slice(&action_json).map_err(|_| UiLayerError::InvalidPayload)?;
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(UiLayerError::AccessNotActive)?;
        self.ui
            .queue_action(&owner, event)
            .map_err(|error| match error {
                UiError::LayerNotOwner | UiError::LayerNotRegistered => {
                    UiLayerError::NotActiveLayer
                }
                UiError::RuntimeUnavailable => UiLayerError::Unavailable,
                _ => UiLayerError::Rejected,
            })
    }
}
