//! Durable metadata for one authoritative world session.

use rintawa_sdk::world::WorldId;
use serde::{Deserialize, Serialize};

use crate::{DefinitionRegistration, PropertyDefinition, PropertyRegistry, WorldResult};

/// Current on-disk world format understood by this runtime.
///
/// Format 6 is the four-table item/property model with no durable command ledger,
/// commit position, event log, consumer cursors, job outbox or control grants.
pub const WORLD_FORMAT_VERSION: u32 = 6;

/// The single JSON manifest embedded in `world.manifest`.
///
/// The manifest is Host-owned world composition data. World-level values that used
/// to live in `world_facets` are carried in the documented `world_facets` section
/// of this document rather than in a fifth table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorldManifest(serde_json::Value);

impl WorldManifest {
    /// Wraps an already-validated manifest document.
    pub fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    /// Returns the manifest document.
    pub const fn as_value(&self) -> &serde_json::Value {
        &self.0
    }

    /// Consumes the wrapper and returns the manifest document.
    pub fn into_value(self) -> serde_json::Value {
        self.0
    }
}

impl Default for WorldManifest {
    fn default() -> Self {
        Self(serde_json::json!({}))
    }
}

/// Feature-neutral durable state needed to open one authoritative world.
#[derive(Debug, Clone)]
pub struct WorldSessionState {
    id: WorldId,
    manifest: WorldManifest,
    properties: PropertyRegistry,
}

impl WorldSessionState {
    /// Creates a new empty world with an empty manifest.
    pub fn new(id: WorldId) -> Self {
        Self {
            id,
            manifest: WorldManifest::default(),
            properties: PropertyRegistry::new(),
        }
    }

    /// Restores persisted world metadata and the property vocabulary.
    pub fn restore(id: WorldId, manifest: WorldManifest, properties: PropertyRegistry) -> Self {
        Self {
            id,
            manifest,
            properties,
        }
    }

    /// Returns the authoritative world identity.
    pub const fn id(&self) -> WorldId {
        self.id
    }

    /// Returns the embedded world manifest.
    pub const fn manifest(&self) -> &WorldManifest {
        &self.manifest
    }

    /// Returns the property vocabulary known to this world.
    pub const fn properties(&self) -> &PropertyRegistry {
        &self.properties
    }

    /// Registers one property definition in this session vocabulary.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the same versioned name has other semantics.
    pub fn register_property(
        &mut self,
        definition: PropertyDefinition,
    ) -> WorldResult<DefinitionRegistration> {
        self.properties.register(definition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_create_empty_world_session() {
        let world = WorldSessionState::new(WorldId::new());

        assert!(world.properties().is_empty());
        assert_eq!(world.manifest().as_value(), &serde_json::json!({}));
    }
}
