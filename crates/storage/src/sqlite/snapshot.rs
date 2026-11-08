//! Consistent read-only SQLite snapshot for one authoritative world.

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_world::{ItemRecord, PropertyRecord};
use rusqlite::Connection;

use crate::StorageResult;

use crate::sqlite::query;

/// Consistent read-only view pinned to one SQLite WAL snapshot.
///
/// The owning connection keeps a read transaction open for the lifetime of this
/// value. Writers may continue committing in WAL mode, but every read through
/// this snapshot observes the same historical state.
#[derive(Debug)]
pub struct SqliteWorldSnapshot {
    connection: Connection,
    world_id: WorldId,
}

impl SqliteWorldSnapshot {
    pub(super) const fn new(connection: Connection, world_id: WorldId) -> Self {
        Self {
            connection,
            world_id,
        }
    }

    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Loads one item from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_item(&self, id: ItemId) -> StorageResult<Option<ItemRecord>> {
        query::load_item(&self.connection, id)
    }

    /// Loads one property value from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded, including a
    /// value written under a different property version.
    pub fn load_property(
        &self,
        item: ItemId,
        key: &SchemaKey,
    ) -> StorageResult<Option<PropertyRecord>> {
        query::load_property(&self.connection, item, key)
    }

    /// Loads every current property of one item.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn properties_of(&self, item: ItemId) -> StorageResult<Vec<PropertyRecord>> {
        query::list_properties(&self.connection, item)
    }

    /// Loads every relation whose subject or object is the given item.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn relations_pointing_at(&self, item: ItemId) -> StorageResult<Vec<ItemRecord>> {
        query::relations_pointing_at(&self.connection, item)
    }
}
