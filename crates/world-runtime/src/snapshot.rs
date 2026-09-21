//! Read-only snapshot capability exposed to command Systems.

use rintawa_sdk::world::{EntityId, RelationId, SchemaKey, WorldId};
use rintawa_storage::SqliteWorldSnapshot;
use rintawa_world::{EntityRecord, FacetRecord, FacetTarget, RelationRecord};

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
}
