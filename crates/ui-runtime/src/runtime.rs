use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use rintawa_sdk::{
    contracts::ComponentRef,
    types::ExtensionId,
    ui::{
        PORTABLE_UI_PROTOCOL_MAJOR, UiActionEvent, UiError, UiLayerDescriptor, UiPatch,
        UiPatchBatch, UiResult, UiSurfaceContribution, UiSurfaceId, UiSurfaceSnapshot,
    },
};

use crate::validation::{
    DEFAULT_MAX_PATCH_OPERATIONS, node_map, required_capabilities, validate_snapshot,
};

/// Static UI surface declaration associated with its owning component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedUiSurfaceContribution {
    /// Component that registered the surface.
    pub owner: ComponentRef,
    /// Static surface metadata.
    pub contribution: UiSurfaceContribution,
}

/// Mounted presentation exposed to the active UI Layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPresentationSurface {
    /// Component that owns and updates the surface.
    pub owner: ComponentRef,
    /// Static surface metadata.
    pub contribution: UiSurfaceContribution,
    /// Current validated presentation snapshot.
    pub snapshot: UiSurfaceSnapshot,
}

/// Validated action ready for dispatch to its owning component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiActionDispatch {
    /// Component allowed to receive this action.
    pub owner: ComponentRef,
    /// Validated semantic action.
    pub event: UiActionEvent,
}

#[derive(Clone)]
struct UiExtension {
    is_active: bool,
    surfaces: HashSet<UiSurfaceId>,
}

#[derive(Clone)]
struct ActiveUiLayer {
    owner: ComponentRef,
    descriptor: UiLayerDescriptor,
    capabilities: HashSet<rintawa_sdk::ui::UiCapabilityId>,
}

#[derive(Default)]
struct UiRuntimeState {
    extensions: HashMap<ExtensionId, UiExtension>,
    registered_surfaces: HashMap<UiSurfaceId, OwnedUiSurfaceContribution>,
    mounted_surfaces: HashMap<UiSurfaceId, UiSurfaceSnapshot>,
    layer: Option<ActiveUiLayer>,
}

/// Shared renderer-neutral runtime for portable surfaces and semantic actions.
#[derive(Clone, Default)]
pub struct UiRuntime {
    state: Arc<RwLock<UiRuntimeState>>,
}

impl UiRuntime {
    /// Creates an empty portable UI runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers static surface declarations for one extension.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate extension or globally conflicting surface identifiers.
    pub fn register_extension(
        &self,
        extension_id: ExtensionId,
        surfaces: Vec<OwnedUiSurfaceContribution>,
    ) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        if state.extensions.contains_key(&extension_id) {
            return Err(UiError::ExtensionAlreadyRegistered(
                extension_id.to_string(),
            ));
        }

        let mut surface_ids = HashSet::new();
        for owned in &surfaces {
            if owned.owner.extension_id != extension_id {
                return Err(UiError::SurfaceNotOwned(owned.contribution.id.to_string()));
            }
            if !surface_ids.insert(owned.contribution.id.clone())
                || state
                    .registered_surfaces
                    .contains_key(&owned.contribution.id)
            {
                return Err(UiError::SurfaceAlreadyRegistered(
                    owned.contribution.id.to_string(),
                ));
            }
        }

        for owned in surfaces {
            state
                .registered_surfaces
                .insert(owned.contribution.id.clone(), owned);
        }
        state.extensions.insert(
            extension_id,
            UiExtension {
                is_active: false,
                surfaces: surface_ids,
            },
        );
        Ok(())
    }

    /// Removes one extension and all of its UI state.
    pub fn unregister_extension(&self, extension_id: &ExtensionId) {
        if let Ok(mut state) = self.state.write()
            && let Some(extension) = state.extensions.remove(extension_id)
        {
            for surface_id in extension.surfaces {
                state.registered_surfaces.remove(&surface_id);
                state.mounted_surfaces.remove(&surface_id);
            }
            if state
                .layer
                .as_ref()
                .is_some_and(|layer| &layer.owner.extension_id == extension_id)
            {
                state.layer = None;
            }
        }
    }

    /// Updates extension activation and revokes mounted presentation on deactivation.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ExtensionNotRegistered`] for an unknown extension.
    pub fn set_extension_active(
        &self,
        extension_id: &ExtensionId,
        is_active: bool,
    ) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let surface_ids = {
            let extension = state
                .extensions
                .get_mut(extension_id)
                .ok_or_else(|| UiError::ExtensionNotRegistered(extension_id.to_string()))?;
            extension.is_active = is_active;
            extension.surfaces.clone()
        };

        if !is_active {
            for surface_id in surface_ids {
                state.mounted_surfaces.remove(&surface_id);
            }
            if state
                .layer
                .as_ref()
                .is_some_and(|layer| &layer.owner.extension_id == extension_id)
            {
                state.layer = None;
            }
        }
        Ok(())
    }

    /// Attaches or refreshes the selected UI Layer.
    ///
    /// Existing mounted surfaces are validated before the layer becomes active.
    ///
    /// # Errors
    ///
    /// Returns an error for an inactive owner, incompatible protocol version,
    /// unsupported mounted capability, or a different already attached layer.
    pub fn attach_layer(&self, owner: ComponentRef, descriptor: UiLayerDescriptor) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let extension = state
            .extensions
            .get(&owner.extension_id)
            .ok_or_else(|| UiError::ExtensionNotRegistered(owner.extension_id.to_string()))?;
        if !extension.is_active {
            return Err(UiError::OwnerInactive);
        }
        if descriptor.protocol_major != PORTABLE_UI_PROTOCOL_MAJOR {
            return Err(UiError::UnsupportedProtocol {
                expected: PORTABLE_UI_PROTOCOL_MAJOR,
                actual: descriptor.protocol_major,
            });
        }
        if state
            .layer
            .as_ref()
            .is_some_and(|layer| layer.owner != owner)
        {
            return Err(UiError::LayerAlreadyAttached);
        }

        let capabilities: HashSet<_> = descriptor.capabilities.iter().cloned().collect();
        for (surface_id, snapshot) in &state.mounted_surfaces {
            let registered = state
                .registered_surfaces
                .get(surface_id)
                .ok_or_else(|| UiError::SurfaceNotRegistered(surface_id.to_string()))?;
            ensure_surface_supported(&capabilities, &registered.contribution, snapshot)?;
        }

        state.layer = Some(ActiveUiLayer {
            owner,
            descriptor,
            capabilities,
        });
        Ok(())
    }

    /// Detaches the current UI Layer without deleting feature presentation snapshots.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::LayerNotOwner`] if another component owns the active layer.
    pub fn detach_layer(&self, owner: &ComponentRef) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        if let Some(layer) = &state.layer {
            if &layer.owner != owner {
                return Err(UiError::LayerNotOwner);
            }
            state.layer = None;
        }
        Ok(())
    }

    /// Returns the descriptor of the current UI Layer, if attached.
    pub fn active_layer(&self) -> Option<(ComponentRef, UiLayerDescriptor)> {
        self.state.read().ok().and_then(|state| {
            state
                .layer
                .as_ref()
                .map(|layer| (layer.owner.clone(), layer.descriptor.clone()))
        })
    }

    /// Mounts an initial snapshot for a statically registered surface.
    ///
    /// A surface can be mounted while no layer is attached. If a layer exists,
    /// its capabilities are validated immediately.
    ///
    /// # Errors
    ///
    /// Returns an error for ownership mismatch, duplicate mount, invalid tree,
    /// or unsupported presentation capability.
    pub fn mount_surface(&self, owner: &ComponentRef, snapshot: UiSurfaceSnapshot) -> UiResult<()> {
        validate_snapshot(&snapshot)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let registered = state
            .registered_surfaces
            .get(&snapshot.surface_id)
            .ok_or_else(|| UiError::SurfaceNotRegistered(snapshot.surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(snapshot.surface_id.to_string()));
        }
        if state.mounted_surfaces.contains_key(&snapshot.surface_id) {
            return Err(UiError::SurfaceAlreadyMounted(
                snapshot.surface_id.to_string(),
            ));
        }
        if let Some(layer) = &state.layer {
            ensure_surface_supported(&layer.capabilities, &registered.contribution, &snapshot)?;
        }
        state
            .mounted_surfaces
            .insert(snapshot.surface_id.clone(), snapshot);
        Ok(())
    }

    /// Applies one atomic incremental patch batch to a mounted surface.
    ///
    /// # Errors
    ///
    /// Returns an error for ownership mismatch, stale revision, invalid patch,
    /// invalid resulting tree, or a capability unsupported by the active layer.
    pub fn apply_patches(&self, owner: &ComponentRef, batch: UiPatchBatch) -> UiResult<()> {
        if batch.patches.len() > DEFAULT_MAX_PATCH_OPERATIONS {
            return Err(UiError::PatchBatchTooLarge {
                actual: batch.patches.len(),
                maximum: DEFAULT_MAX_PATCH_OPERATIONS,
            });
        }

        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let registered = state
            .registered_surfaces
            .get(&batch.surface_id)
            .ok_or_else(|| UiError::SurfaceNotRegistered(batch.surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(batch.surface_id.to_string()));
        }
        let current = state
            .mounted_surfaces
            .get(&batch.surface_id)
            .ok_or_else(|| UiError::SurfaceNotMounted(batch.surface_id.to_string()))?;
        if current.revision != batch.base_revision {
            return Err(UiError::RevisionMismatch {
                expected: current.revision,
                actual: batch.base_revision,
            });
        }
        if batch.base_revision.checked_add(1) != Some(batch.next_revision) {
            return Err(UiError::InvalidRevision {
                base: batch.base_revision,
                next: batch.next_revision,
            });
        }

        let mut next = current.clone();
        apply_patch_operations(&mut next, &batch.patches)?;
        next.revision = batch.next_revision;
        validate_snapshot(&next)?;
        if let Some(layer) = &state.layer {
            ensure_surface_supported(&layer.capabilities, &registered.contribution, &next)?;
        }
        state.mounted_surfaces.insert(batch.surface_id, next);
        Ok(())
    }

    /// Removes the current presentation snapshot for one owned surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface is unregistered, owned by another component,
    /// or not currently mounted.
    pub fn unmount_surface(&self, owner: &ComponentRef, surface_id: &UiSurfaceId) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let registered = state
            .registered_surfaces
            .get(surface_id)
            .ok_or_else(|| UiError::SurfaceNotRegistered(surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(surface_id.to_string()));
        }
        if state.mounted_surfaces.remove(surface_id).is_none() {
            return Err(UiError::SurfaceNotMounted(surface_id.to_string()));
        }
        Ok(())
    }

    /// Returns a stable snapshot of all currently mounted portable surfaces.
    pub fn presentation_surfaces(&self) -> Vec<UiPresentationSurface> {
        let Ok(state) = self.state.read() else {
            return Vec::new();
        };
        let mut surfaces: Vec<_> = state
            .mounted_surfaces
            .iter()
            .filter_map(|(surface_id, snapshot)| {
                let registered = state.registered_surfaces.get(surface_id)?;
                let extension = state.extensions.get(&registered.owner.extension_id)?;
                extension.is_active.then(|| UiPresentationSurface {
                    owner: registered.owner.clone(),
                    contribution: registered.contribution.clone(),
                    snapshot: snapshot.clone(),
                })
            })
            .collect();
        surfaces.sort_by(|left, right| {
            left.contribution
                .id
                .as_str()
                .cmp(right.contribution.id.as_str())
        });
        surfaces
    }

    /// Validates an input event from the active UI Layer and resolves its owner.
    ///
    /// # Errors
    ///
    /// Returns an error for layer spoofing, stale revisions, inactive owners,
    /// missing nodes, unbound actions, disabled controls, or invalid payloads.
    pub fn route_action(
        &self,
        layer_owner: &ComponentRef,
        event: UiActionEvent,
    ) -> UiResult<UiActionDispatch> {
        let state = self.state.read().map_err(|_| UiError::RuntimeUnavailable)?;
        let layer = state.layer.as_ref().ok_or(UiError::LayerUnavailable)?;
        if &layer.owner != layer_owner {
            return Err(UiError::LayerNotOwner);
        }
        let registered = state
            .registered_surfaces
            .get(&event.surface_id)
            .ok_or_else(|| UiError::SurfaceNotRegistered(event.surface_id.to_string()))?;
        let extension = state
            .extensions
            .get(&registered.owner.extension_id)
            .ok_or_else(|| {
                UiError::ExtensionNotRegistered(registered.owner.extension_id.to_string())
            })?;
        if !extension.is_active {
            return Err(UiError::OwnerInactive);
        }
        let snapshot = state
            .mounted_surfaces
            .get(&event.surface_id)
            .ok_or_else(|| UiError::SurfaceNotMounted(event.surface_id.to_string()))?;
        if snapshot.revision != event.surface_revision {
            return Err(UiError::RevisionMismatch {
                expected: snapshot.revision,
                actual: event.surface_revision,
            });
        }
        let node = snapshot
            .nodes
            .iter()
            .find(|node| node.id == event.node_id)
            .ok_or_else(|| UiError::NodeNotFound(event.node_id.to_string()))?;
        if !node.kind.has_action(&event.action_id) {
            return Err(UiError::ActionNotBound {
                surface: event.surface_id.to_string(),
                node: event.node_id.to_string(),
                action: event.action_id.to_string(),
            });
        }
        if !node.kind.is_action_enabled(&event.action_id) {
            return Err(UiError::ActionDisabled {
                node: event.node_id.to_string(),
                action: event.action_id.to_string(),
            });
        }
        if !node
            .kind
            .accepts_action_payload(&event.action_id, &event.payload)
        {
            return Err(UiError::InvalidActionPayload {
                node: event.node_id.to_string(),
                action: event.action_id.to_string(),
            });
        }

        Ok(UiActionDispatch {
            owner: registered.owner.clone(),
            event,
        })
    }
}

fn ensure_surface_supported(
    layer_capabilities: &HashSet<rintawa_sdk::ui::UiCapabilityId>,
    contribution: &UiSurfaceContribution,
    snapshot: &UiSurfaceSnapshot,
) -> UiResult<()> {
    for capability in contribution
        .required_capabilities
        .iter()
        .cloned()
        .chain(required_capabilities(snapshot))
    {
        if !layer_capabilities.contains(&capability) {
            return Err(UiError::UnsupportedCapability(capability.to_string()));
        }
    }
    Ok(())
}

fn apply_patch_operations(snapshot: &mut UiSurfaceSnapshot, patches: &[UiPatch]) -> UiResult<()> {
    let mut nodes = node_map(snapshot)?;
    for patch in patches {
        match patch {
            UiPatch::UpsertNode { node } => {
                nodes.insert(node.id.clone(), node.clone());
            }
            UiPatch::RemoveNode { node_id } => {
                if nodes.remove(node_id).is_none() {
                    return Err(UiError::NodeNotFound(node_id.to_string()));
                }
            }
            UiPatch::InsertChild {
                parent,
                index,
                child,
            } => {
                if !nodes.contains_key(child) {
                    return Err(UiError::NodeNotFound(child.to_string()));
                }
                let parent_node = nodes
                    .get_mut(parent)
                    .ok_or_else(|| UiError::NodeNotFound(parent.to_string()))?;
                let children = parent_node
                    .kind
                    .children_mut()
                    .ok_or_else(|| UiError::NodeNotContainer(parent.to_string()))?;
                let requested_index = *index;
                let index =
                    usize::try_from(requested_index).map_err(|_| UiError::InvalidChildIndex {
                        parent: parent.to_string(),
                        index: requested_index,
                    })?;
                if index > children.len() {
                    return Err(UiError::InvalidChildIndex {
                        parent: parent.to_string(),
                        index: requested_index,
                    });
                }
                children.insert(index, child.clone());
            }
            UiPatch::RemoveChild { parent, child } => {
                let parent_node = nodes
                    .get_mut(parent)
                    .ok_or_else(|| UiError::NodeNotFound(parent.to_string()))?;
                let children = parent_node
                    .kind
                    .children_mut()
                    .ok_or_else(|| UiError::NodeNotContainer(parent.to_string()))?;
                let index = children
                    .iter()
                    .position(|candidate| candidate == child)
                    .ok_or_else(|| UiError::ChildNotFound {
                        parent: parent.to_string(),
                        child: child.to_string(),
                    })?;
                children.remove(index);
            }
            UiPatch::MoveChild {
                parent,
                child,
                index,
            } => {
                let parent_node = nodes
                    .get_mut(parent)
                    .ok_or_else(|| UiError::NodeNotFound(parent.to_string()))?;
                let children = parent_node
                    .kind
                    .children_mut()
                    .ok_or_else(|| UiError::NodeNotContainer(parent.to_string()))?;
                let current = children
                    .iter()
                    .position(|candidate| candidate == child)
                    .ok_or_else(|| UiError::ChildNotFound {
                        parent: parent.to_string(),
                        child: child.to_string(),
                    })?;
                let child = children.remove(current);
                let requested_index = *index;
                let index =
                    usize::try_from(requested_index).map_err(|_| UiError::InvalidChildIndex {
                        parent: parent.to_string(),
                        index: requested_index,
                    })?;
                if index > children.len() {
                    return Err(UiError::InvalidChildIndex {
                        parent: parent.to_string(),
                        index: requested_index,
                    });
                }
                children.insert(index, child);
            }
        }
    }

    let mut rebuilt: Vec<_> = nodes.into_values().collect();
    rebuilt.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    snapshot.nodes = rebuilt;
    Ok(())
}
