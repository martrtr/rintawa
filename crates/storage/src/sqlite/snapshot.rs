//! Consistent read-only SQLite snapshot for one authoritative world.

use rintawa_sdk::world::{EntityId, PrincipalId, RelationId, SchemaKey, WorldId};
use rintawa_world::{EntityRecord, FacetRecord, FacetTarget, RelationRecord, SchemaDefinition};
use rusqlite::Connection;

use crate::StorageResult;

use crate::sqlite::{codec, query};

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

    /// Loads one immutable world-schema definition from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted schema metadata cannot be decoded.
    pub fn load_schema(&self, key: &SchemaKey) -> StorageResult<Option<SchemaDefinition>> {
        codec::load_schema(&self.connection, key)
    }

    /// Returns whether one principal may issue an exact command as an entity actor
    /// at this snapshot position.
    ///
    /// # Errors
    ///
    /// Returns a storage error when durable authority state cannot be queried.
    pub fn principal_can_control(
        &self,
        principal: PrincipalId,
        actor_entity: EntityId,
        command_schema: &SchemaKey,
    ) -> StorageResult<bool> {
        query::principal_can_control(
            &self.connection,
            principal,
            actor_entity,
            command_schema,
            self.position,
        )
    }
}
