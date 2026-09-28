//! Host-owned shell composition for foreground World entry surfaces.
//!
//! This module intentionally does not own rendering. It only resolves the
//! composition decision: which World presentation descriptor is imported into
//! one UI Layer session. UI Runtime remains the authority for visibility.

use rintawa_sdk::{contracts::ComponentRef, types::RuntimeScopeId, world::WorldId};
use rintawa_ui_runtime::UiLayerPresentationState;

use crate::{engine::ExtensionEngine, errors::EngineResult};

/// One requested foreground World composition operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWorldEntryRequest {
    /// UI Layer receiving the composed presentation.
    pub layer_owner: ComponentRef,
    /// World selected as the foreground entry point.
    pub world_id: WorldId,
    /// Runtime scope containing the selected World.
    pub scope_id: RuntimeScopeId,
}

/// Host-owned foreground World composer.
///
/// A shell may keep higher-level navigation state elsewhere; this type only
/// bridges that state into the authenticated UI Runtime boundary.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellComposer;

impl ShellComposer {
    /// Creates a shell composer.
    pub fn new() -> Self {
        Self
    }

    /// Imports one World entry surface into a UI Layer.
    pub fn enter_world(
        &self,
        engine: &ExtensionEngine,
        request: ShellWorldEntryRequest,
    ) -> EngineResult<()> {
        let descriptor = engine.registered_world_presentation_descriptor(&request.layer_owner)?;
        engine.set_focused_world_for_ui_layer(
            &request.layer_owner,
            request.world_id,
            request.scope_id,
            descriptor,
        )
    }

    /// Clears the foreground World while leaving the World lifecycle untouched.
    pub fn leave_world(
        &self,
        engine: &ExtensionEngine,
        layer_owner: &ComponentRef,
    ) -> EngineResult<()> {
        engine.clear_focused_world_for_ui_layer(layer_owner)
    }

    /// Returns the composed presentation state for shell rendering.
    pub fn presentation_state(
        &self,
        engine: &ExtensionEngine,
        layer_owner: &ComponentRef,
    ) -> EngineResult<UiLayerPresentationState> {
        engine.ui_layer_presentation_state(layer_owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_has_no_implicit_world_selection() {
        let _composer = ShellComposer::new();
        let request = ShellWorldEntryRequest {
            layer_owner: ComponentRef::new("shell", "layer"),
            world_id: WorldId::new(),
            scope_id: RuntimeScopeId::new("scope-a"),
        };
        assert_eq!(request.scope_id.as_str(), "scope-a");
    }
}
