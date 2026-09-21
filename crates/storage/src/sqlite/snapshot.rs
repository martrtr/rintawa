//! Consistent read-only SQLite snapshot for one authoritative world.

use rintawa_sdk::world::{EntityId, RelationId, SchemaKey, WorldId};
use rintawa_world::{EntityRecord, FacetRecord, FacetTarget, RelationRecord};
use rusqlite::Connection;

use crate::StorageResult;

use super::query;

/// Consistent read-only view pinned to one SQLite WAL snapshot.
///
/// The owning connection keeps a read transaction open for the lifetime of this
/// value. Writers may continue committing in WAL mode, but all reads through
/// this snapshot observe the same historical state.
#[derive(Debug)]
pub struct SqliteWorldSnapshot {
    connection: Connection,
    world_id: WorldId,
    position: u64,
}

impl SqliteWorldSnapshot {
    pub(super) const fn new(connection: Connection, world_id: WorldId, position: u64) -> Self {
        Self {
            connection,
            world_id,
            position,
        }
    }

    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the commit position pinned by this read transaction.
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Loads one entity from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_entity(&self, entity_id: EntityId) -> StorageResult<Option<EntityRecord>> {
        query::load_entity(&self.connection, entity_id)
    }

    /// Loads one relation from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_relation(&self, relation_id: RelationId) -> StorageResult<Option<RelationRecord>> {
        query::load_relation(&self.connection, relation_id)
    }

    /// Loads one facet from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_facet(
        &self,
        target: FacetTarget,
        schema: &SchemaKey,
    ) -> StorageResult<Option<FacetRecord>> {
        query::load_facet(&self.connection, target, schema)
    }
}
