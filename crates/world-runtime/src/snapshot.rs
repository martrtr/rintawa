//! Read-only authoritative snapshot capability used by Systems and Host-side projections.

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_storage::SqliteWorldSnapshot;
use rintawa_world::{ItemRecord, PropertyRecord};

use crate::WorldReadResult;

/// Read-only view evaluated by one System against one pinned SQLite WAL snapshot.
///
/// The single-writer runtime guarantees that no other writer is active while a
/// System evaluates, so the snapshot cannot drift between the reads a System
/// performs and the commit of its output.
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

    /// Loads one item from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn load_item(&self, id: ItemId) -> WorldReadResult<Option<ItemRecord>> {
        Ok(self.storage.load_item(id)?)
    }

    /// Loads one property value from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded, including a
    /// value stored under a different property version.
    pub fn load_property(
        &self,
        item: ItemId,
        key: &SchemaKey,
    ) -> WorldReadResult<Option<PropertyRecord>> {
        Ok(self.storage.load_property(item, key)?)
    }

    /// Loads every current property of one item.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn properties_of(&self, item: ItemId) -> WorldReadResult<Vec<PropertyRecord>> {
        Ok(self.storage.properties_of(item)?)
    }

    /// Loads every relation whose subject or object is the given item.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn relations_pointing_at(&self, item: ItemId) -> WorldReadResult<Vec<ItemRecord>> {
        Ok(self.storage.relations_pointing_at(item)?)
    }
}
