//! Durable metadata for one authoritative world session.

use rintawa_sdk::world::WorldId;

use crate::{SchemaDefinition, SchemaRegistration, SchemaRegistry, WorldResult};

/// Current on-disk world format understood by this runtime.
pub const WORLD_FORMAT_VERSION: u32 = 1;

/// Feature-neutral durable state needed to open one authoritative world session.
#[derive(Debug, Clone)]
pub struct WorldSessionState {
    id: WorldId,
    commit_position: u64,
    schemas: SchemaRegistry,
}

impl WorldSessionState {
    /// Creates a new empty world with commit position zero.
    pub fn new(id: WorldId) -> Self {
        Self {
            id,
            commit_position: 0,
            schemas: SchemaRegistry::new(),
        }
    }

    /// Restores persisted world metadata and schema definitions.
    pub fn restore(id: WorldId, commit_position: u64, schemas: SchemaRegistry) -> Self {
        Self {
            id,
            commit_position,
            schemas,
        }
    }

    /// Returns the authoritative world identity.
    pub const fn id(&self) -> WorldId {
        self.id
    }

    /// Returns the last committed local world position.
    pub const fn commit_position(&self) -> u64 {
        self.commit_position
    }

    /// Returns the immutable schema registry.
    pub const fn schemas(&self) -> &SchemaRegistry {
        &self.schemas
    }

    /// Registers one world schema.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the same key already has different semantics.
    pub fn register_schema(
        &mut self,
        definition: SchemaDefinition,
    ) -> WorldResult<SchemaRegistration> {
        self.schemas.register(definition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_create_empty_world_session() {
        let world = WorldSessionState::new(WorldId::new());

        assert_eq!(world.commit_position(), 0);
        assert!(world.schemas().is_empty());
    }
}
