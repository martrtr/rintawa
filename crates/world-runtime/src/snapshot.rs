//! Read-only authoritative snapshot capability used by Systems and Host-side projections.

use rintawa_sdk::world::{EntityId, PrincipalId, RelationId, SchemaKey, WorldId};
use rintawa_storage::SqliteWorldSnapshot;
use rintawa_world::{EntityRecord, FacetRecord, FacetTarget, RelationRecord, SchemaDefinition};

use crate::WorldReadResult;

/// Read-only view evaluated by one System at a fixed authoritative position.
///
/// All reads use one pinned SQLite WAL snapshot. The runtime also requires the
/// authoritative position to remain unchanged before committing System output.
#[derive(Debug)]
pub struct WorldSnapshot {
    storage: SqliteWorldSnapshot,
}

impl WorldSnapshot {
    pub(crate) const fn new(storage: SqliteWorldSnapshot) -> Self {
        Self { storage }
    }

    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.storage.world_id()
    }

    /// Returns the exact position against which the System is evaluating.
    pub const fn position(&self) -> u64 {
        self.storage.position()
    }

    /// Loads one entity from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn load_entity(&self, entity_id: EntityId) -> WorldReadResult<Option<EntityRecord>> {
        Ok(self.storage.load_entity(entity_id)?)
    }

    /// Loads one relation from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn load_relation(
        &self,
        relation_id: RelationId,
    ) -> WorldReadResult<Option<RelationRecord>> {
        Ok(self.storage.load_relation(relation_id)?)
    }

    /// Loads one facet from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn load_facet(
        &self,
        target: FacetTarget,
        schema: &SchemaKey,
    ) -> WorldReadResult<Option<FacetRecord>> {
        Ok(self.storage.load_facet(target, schema)?)
    }

    pub(crate) fn load_schema_definition(
        &self,
        key: &SchemaKey,
    ) -> WorldReadResult<Option<SchemaDefinition>> {
        Ok(self.storage.load_schema(key)?)
    }

    pub(crate) fn principal_can_control(
        &self,
        principal: PrincipalId,
        actor_entity: EntityId,
        command_schema: &SchemaKey,
    ) -> WorldReadResult<bool> {
        Ok(self
            .storage
            .principal_can_control(principal, actor_entity, command_schema)?)
    }
}
