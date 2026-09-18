use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
};

use serde::Serialize;

const MAX_QUEUED_ACTIONS: usize = 64;

use rintawa_sdk::{
    contracts::ComponentRef,
    types::{ExtensionInstanceId, RuntimeScopeId},
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

/// Static portable UI Layer descriptor associated with its owning component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedUiLayerDescriptor {
    /// Component that registered the layer descriptor.
    pub owner: ComponentRef,
    /// Renderer capabilities offered by the component.
    pub descriptor: UiLayerDescriptor,
}

/// Mounted presentation exposed to an eligible UI Layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// Renderer action queued by an active UI Layer for Engine dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedUiAction {
    /// Active layer that submitted the event.
    pub layer_owner: ComponentRef,
    /// Semantic action to validate again immediately before dispatch.
    pub event: UiActionEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UiSurfaceKey {
    instance_id: ExtensionInstanceId,
    surface_id: UiSurfaceId,
}

impl UiSurfaceKey {
    fn new(instance_id: ExtensionInstanceId, surface_id: UiSurfaceId) -> Self {
        Self {
            instance_id,
            surface_id,
        }
    }
}

#[derive(Clone)]
struct UiExtensionInstance {
    scope_id: RuntimeScopeId,
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
    instances: HashMap<ExtensionInstanceId, UiExtensionInstance>,
    registered_surfaces: HashMap<UiSurfaceKey, OwnedUiSurfaceContribution>,
    registered_layers: HashMap<ComponentRef, UiLayerDescriptor>,
    mounted_surfaces: HashMap<UiSurfaceKey, UiSurfaceSnapshot>,
    layers: HashMap<RuntimeScopeId, ActiveUiLayer>,
    queued_actions: VecDeque<QueuedUiAction>,
}

/// Shared renderer-neutral runtime for portable surfaces and semantic actions.
///
/// The runtime isolates registrations and mounted surfaces by extension instance.
/// UI layers currently see only instances in their exact runtime scope. A future
/// scope-visibility policy can widen that relation without changing surface ownership.
#[derive(Clone, Default)]
pub struct UiRuntime {
    state: Arc<RwLock<UiRuntimeState>>,
}

impl UiRuntime {
    /// Creates an empty portable UI runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers static surface declarations for one extension runtime instance.
    ///
    /// Surface identifiers are local to an extension instance, so two instances
    /// of the same logical extension may register the same surface IDs safely.
    ///
    /// # Errors
    ///
    /// Returns an error for a duplicate instance, mismatched owner, or duplicate
    /// surface identifier within the same instance.
    pub fn register_instance(
        &self,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
        surfaces: Vec<OwnedUiSurfaceContribution>,
        layers: Vec<OwnedUiLayerDescriptor>,
    ) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        if state.instances.contains_key(&instance_id) {
            return Err(UiError::InstanceAlreadyRegistered(instance_id.to_string()));
        }

        let mut surface_ids = HashSet::new();
        for owned in &surfaces {
            if owned.owner.instance_id != instance_id {
                return Err(UiError::SurfaceNotOwned(owned.contribution.id.to_string()));
            }
            if !surface_ids.insert(owned.contribution.id.clone()) {
                return Err(UiError::SurfaceAlreadyRegistered(
                    owned.contribution.id.to_string(),
                ));
            }
        }

        let mut layer_owners = HashSet::new();
        for layer in &layers {
            if layer.owner.instance_id != instance_id {
                return Err(UiError::LayerNotOwner);
            }
            if !layer_owners.insert(layer.owner.clone()) {
                return Err(UiError::LayerAlreadyRegistered);
            }
        }

        for owned in surfaces {
            let key = UiSurfaceKey::new(instance_id.clone(), owned.contribution.id.clone());
            state.registered_surfaces.insert(key, owned);
        }
        for layer in layers {
            state
                .registered_layers
                .insert(layer.owner, layer.descriptor);
        }
        state.instances.insert(
            instance_id,
            UiExtensionInstance {
                scope_id,
                is_active: false,
                surfaces: surface_ids,
            },
        );
        Ok(())
    }

    /// Removes one extension instance and all of its UI state.
    pub fn unregister_instance(&self, instance_id: &ExtensionInstanceId) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(instance) = state.instances.remove(instance_id) else {
            return;
        };
        for surface_id in instance.surfaces {
            let key = UiSurfaceKey::new(instance_id.clone(), surface_id);
            state.registered_surfaces.remove(&key);
            state.mounted_surfaces.remove(&key);
        }
        state
            .registered_layers
            .retain(|owner, _| &owner.instance_id != instance_id);
        state.queued_actions.retain(|queued| {
            &queued.layer_owner.instance_id != instance_id
                && &queued.event.owner_instance_id != instance_id
        });
        if state
            .layers
            .get(&instance.scope_id)
            .is_some_and(|layer| &layer.owner.instance_id == instance_id)
        {
            state.layers.remove(&instance.scope_id);
        }
    }

    /// Updates instance activation and revokes mounted presentation on deactivation.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::InstanceNotRegistered`] for an unknown instance.
    pub fn set_instance_active(
        &self,
        instance_id: &ExtensionInstanceId,
        is_active: bool,
    ) -> UiResult<()> {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) if !is_active => poisoned.into_inner(),
            Err(_) => return Err(UiError::RuntimeUnavailable),
        };
        let (scope_id, surface_ids) = {
            let instance = state
                .instances
                .get_mut(instance_id)
                .ok_or_else(|| UiError::InstanceNotRegistered(instance_id.to_string()))?;
            instance.is_active = is_active;
            (instance.scope_id.clone(), instance.surfaces.clone())
        };

        if !is_active {
            for surface_id in surface_ids {
                state
                    .mounted_surfaces
                    .remove(&UiSurfaceKey::new(instance_id.clone(), surface_id));
            }
            state.queued_actions.retain(|queued| {
                &queued.layer_owner.instance_id != instance_id
                    && &queued.event.owner_instance_id != instance_id
            });
            if state
                .layers
                .get(&scope_id)
                .is_some_and(|layer| &layer.owner.instance_id == instance_id)
            {
                state.layers.remove(&scope_id);
            }
        }
        Ok(())
    }

    /// Returns the statically registered descriptor for one UI Layer candidate.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::LayerNotRegistered`] when the component did not register a
    /// layer descriptor.
    pub fn registered_layer_descriptor(&self, owner: &ComponentRef) -> UiResult<UiLayerDescriptor> {
        self.state
            .read()
            .map_err(|_| UiError::RuntimeUnavailable)?
            .registered_layers
            .get(owner)
            .cloned()
            .ok_or(UiError::LayerNotRegistered)
    }

    /// Attaches the statically registered descriptor for a selected layer provider.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::attach_layer`] or
    /// [`UiError::LayerNotRegistered`] when no descriptor was registered.
    pub fn attach_registered_layer(&self, owner: ComponentRef) -> UiResult<()> {
        let descriptor = self.registered_layer_descriptor(&owner)?;
        self.attach_layer(owner, descriptor)
    }

    /// Attaches or refreshes the selected UI Layer for its runtime scope.
    ///
    /// Existing mounted surfaces in the same scope are validated before the layer
    /// becomes active. Different scopes may host different UI layers concurrently.
    ///
    /// # Errors
    ///
    /// Returns an error for an inactive owner, incompatible protocol version,
    /// unsupported mounted capability, or a different layer already attached in
    /// the same scope.
    pub fn attach_layer(&self, owner: ComponentRef, descriptor: UiLayerDescriptor) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let instance = state
            .instances
            .get(&owner.instance_id)
            .ok_or_else(|| UiError::InstanceNotRegistered(owner.instance_id.to_string()))?;
        if !instance.is_active {
            return Err(UiError::OwnerInactive);
        }
        let scope_id = instance.scope_id.clone();
        if descriptor.protocol_major != PORTABLE_UI_PROTOCOL_MAJOR {
            return Err(UiError::UnsupportedProtocol {
                expected: PORTABLE_UI_PROTOCOL_MAJOR,
                actual: descriptor.protocol_major,
            });
        }
        if state
            .layers
            .get(&scope_id)
            .is_some_and(|layer| layer.owner != owner)
        {
            return Err(UiError::LayerAlreadyAttached);
        }

        let capabilities: HashSet<_> = descriptor.capabilities.iter().cloned().collect();
        for (key, snapshot) in &state.mounted_surfaces {
            let Some(surface_instance) = state.instances.get(&key.instance_id) else {
                continue;
            };
            if surface_instance.scope_id != scope_id || !surface_instance.is_active {
                continue;
            }
            let registered = state
                .registered_surfaces
                .get(key)
                .ok_or_else(|| UiError::SurfaceNotRegistered(key.surface_id.to_string()))?;
            ensure_surface_supported(&capabilities, &registered.contribution, snapshot)?;
        }

        state.layers.insert(
            scope_id,
            ActiveUiLayer {
                owner,
                descriptor,
                capabilities,
            },
        );
        Ok(())
    }

    /// Detaches the current UI Layer in the owner's scope without deleting surfaces.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown instance or if another component owns the
    /// active layer in that scope.
    pub fn detach_layer(&self, owner: &ComponentRef) -> UiResult<()> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        let scope_id = state
            .instances
            .get(&owner.instance_id)
            .ok_or_else(|| UiError::InstanceNotRegistered(owner.instance_id.to_string()))?
            .scope_id
            .clone();
        if let Some(layer) = state.layers.get(&scope_id) {
            if &layer.owner != owner {
                return Err(UiError::LayerNotOwner);
            }
            state.layers.remove(&scope_id);
        }
        Ok(())
    }

    /// Returns the active UI Layer in one runtime scope, if attached.
    pub fn active_layer(
        &self,
        scope_id: &RuntimeScopeId,
    ) -> Option<(ComponentRef, UiLayerDescriptor)> {
        self.state.read().ok().and_then(|state| {
            state
                .layers
                .get(scope_id)
                .map(|layer| (layer.owner.clone(), layer.descriptor.clone()))
        })
    }

    /// Mounts an initial snapshot for a statically registered surface.
    ///
    /// A surface can be mounted while no layer is attached. If a layer exists in
    /// the same scope, its capabilities are validated immediately.
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
        let key = UiSurfaceKey::new(owner.instance_id.clone(), snapshot.surface_id.clone());
        let registered = state
            .registered_surfaces
            .get(&key)
            .ok_or_else(|| UiError::SurfaceNotRegistered(snapshot.surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(snapshot.surface_id.to_string()));
        }
        if state.mounted_surfaces.contains_key(&key) {
            return Err(UiError::SurfaceAlreadyMounted(
                snapshot.surface_id.to_string(),
            ));
        }
        let scope_id = state
            .instances
            .get(&owner.instance_id)
            .ok_or_else(|| UiError::InstanceNotRegistered(owner.instance_id.to_string()))?
            .scope_id
            .clone();
        if let Some(layer) = state.layers.get(&scope_id) {
            ensure_surface_supported(&layer.capabilities, &registered.contribution, &snapshot)?;
        }
        state.mounted_surfaces.insert(key, snapshot);
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
        let key = UiSurfaceKey::new(owner.instance_id.clone(), batch.surface_id.clone());
        let registered = state
            .registered_surfaces
            .get(&key)
            .ok_or_else(|| UiError::SurfaceNotRegistered(batch.surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(batch.surface_id.to_string()));
        }
        let current = state
            .mounted_surfaces
            .get(&key)
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
        let scope_id = state
            .instances
            .get(&owner.instance_id)
            .ok_or_else(|| UiError::InstanceNotRegistered(owner.instance_id.to_string()))?
            .scope_id
            .clone();
        if let Some(layer) = state.layers.get(&scope_id) {
            ensure_surface_supported(&layer.capabilities, &registered.contribution, &next)?;
        }
        state.mounted_surfaces.insert(key, next);
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
        let key = UiSurfaceKey::new(owner.instance_id.clone(), surface_id.clone());
        let registered = state
            .registered_surfaces
            .get(&key)
            .ok_or_else(|| UiError::SurfaceNotRegistered(surface_id.to_string()))?;
        if &registered.owner != owner {
            return Err(UiError::SurfaceNotOwned(surface_id.to_string()));
        }
        if state.mounted_surfaces.remove(&key).is_none() {
            return Err(UiError::SurfaceNotMounted(surface_id.to_string()));
        }
        Ok(())
    }

    /// Returns a stable host snapshot of all active mounted portable surfaces.
    pub fn presentation_surfaces(&self) -> Vec<UiPresentationSurface> {
        let Ok(state) = self.state.read() else {
            return Vec::new();
        };
        collect_presentations(&state, None)
    }

    /// Returns surfaces visible to the active UI Layer in its exact runtime scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the layer instance is unknown or is not the active
    /// layer for its scope.
    pub fn presentation_surfaces_for_layer(
        &self,
        layer_owner: &ComponentRef,
    ) -> UiResult<Vec<UiPresentationSurface>> {
        let state = self.state.read().map_err(|_| UiError::RuntimeUnavailable)?;
        let scope_id = state
            .layers
            .iter()
            .find_map(|(scope_id, layer)| (&layer.owner == layer_owner).then_some(scope_id.clone()))
            .ok_or(UiError::LayerNotOwner)?;
        Ok(collect_presentations(&state, Some(&scope_id)))
    }

    /// Validates and queues one renderer action for later Engine dispatch.
    ///
    /// The Engine validates the event again immediately before invoking the target
    /// component so queued work cannot bypass lifecycle or revision changes.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::route_action`] or
    /// [`UiError::ActionQueueFull`] when the bounded host queue is saturated.
    pub fn queue_action(&self, layer_owner: &ComponentRef, event: UiActionEvent) -> UiResult<()> {
        self.route_action(layer_owner, event.clone())?;
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        if state.queued_actions.len() >= MAX_QUEUED_ACTIONS {
            return Err(UiError::ActionQueueFull);
        }
        state.queued_actions.push_back(QueuedUiAction {
            layer_owner: layer_owner.clone(),
            event,
        });
        Ok(())
    }

    /// Drains renderer actions queued since the previous Engine pump.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::RuntimeUnavailable`] when shared UI state cannot be accessed.
    pub fn drain_queued_actions(&self) -> UiResult<Vec<QueuedUiAction>> {
        let mut state = self
            .state
            .write()
            .map_err(|_| UiError::RuntimeUnavailable)?;
        Ok(state.queued_actions.drain(..).collect())
    }

    /// Validates an input event from an active UI Layer and resolves its owner.
    ///
    /// Current scope policy is intentionally conservative: a layer may route input
    /// only to surfaces in its exact runtime scope. Future scope imports can widen
    /// visibility without weakening instance ownership.
    ///
    /// # Errors
    ///
    /// Returns an error for layer spoofing, cross-scope input, stale revisions,
    /// inactive owners, missing nodes, unbound actions, disabled controls, or
    /// invalid payloads.
    pub fn route_action(
        &self,
        layer_owner: &ComponentRef,
        event: UiActionEvent,
    ) -> UiResult<UiActionDispatch> {
        let state = self.state.read().map_err(|_| UiError::RuntimeUnavailable)?;
        let layer_scope = state
            .layers
            .iter()
            .find_map(|(scope_id, layer)| (&layer.owner == layer_owner).then_some(scope_id.clone()))
            .ok_or(UiError::LayerNotOwner)?;

        let target_instance = state
            .instances
            .get(&event.owner_instance_id)
            .ok_or_else(|| UiError::InstanceNotRegistered(event.owner_instance_id.to_string()))?;
        if target_instance.scope_id != layer_scope {
            return Err(UiError::ScopeNotVisible);
        }
        if !target_instance.is_active {
            return Err(UiError::OwnerInactive);
        }

        let key = UiSurfaceKey::new(event.owner_instance_id.clone(), event.surface_id.clone());
        let registered = state
            .registered_surfaces
            .get(&key)
            .ok_or_else(|| UiError::SurfaceNotRegistered(event.surface_id.to_string()))?;
        let snapshot = state
            .mounted_surfaces
            .get(&key)
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

fn collect_presentations(
    state: &UiRuntimeState,
    scope_filter: Option<&RuntimeScopeId>,
) -> Vec<UiPresentationSurface> {
    let mut surfaces: Vec<_> = state
        .mounted_surfaces
        .iter()
        .filter_map(|(key, snapshot)| {
            let registered = state.registered_surfaces.get(key)?;
            let instance = state.instances.get(&key.instance_id)?;
            if !instance.is_active || scope_filter.is_some_and(|scope| scope != &instance.scope_id)
            {
                return None;
            }
            Some(UiPresentationSurface {
                owner: registered.owner.clone(),
                contribution: registered.contribution.clone(),
                snapshot: snapshot.clone(),
            })
        })
        .collect();
    surfaces.sort_by(|left, right| {
        left.owner
            .instance_id
            .as_str()
            .cmp(right.owner.instance_id.as_str())
            .then_with(|| {
                left.contribution
                    .id
                    .as_str()
                    .cmp(right.contribution.id.as_str())
            })
    });
    surfaces
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

#[cfg(test)]
mod poison_cleanup_tests {
    use std::thread;

    use super::*;

    #[test]
    fn test_should_unregister_instance_after_ui_state_lock_is_poisoned() {
        let runtime = UiRuntime::new();
        let instance_id = ExtensionInstanceId::new("example.ui");
        runtime
            .register_instance(
                instance_id.clone(),
                RuntimeScopeId::new("host"),
                Vec::new(),
                Vec::new(),
            )
            .expect("test UI instance should register");

        let poisoner = runtime.clone();
        let _ = thread::spawn(move || {
            let _state = poisoner
                .state
                .write()
                .expect("test UI state lock should start healthy");
            panic!("poison UI runtime state lock");
        })
        .join();

        runtime.unregister_instance(&instance_id);
        let state = match runtime.state.read() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!state.instances.contains_key(&instance_id));
    }
}
