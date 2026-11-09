//! Consistent read-only SQLite snapshot for one authoritative world.

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_world::{ItemRecord, PropertyRecord, RelationDirection};
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

    /// Loads an item batch and selected properties through the same pinned snapshot.
    ///
    /// Both the item batch and property-key list are bounded by storage policy.
    /// Missing items or absent properties are omitted from the returned vectors.
    ///
    /// # Errors
    ///
    /// Returns an error when a batch limit is exceeded or persisted state is invalid.
    pub fn read(
        &self,
        items: &[ItemId],
        properties: &[SchemaKey],
    ) -> StorageResult<(Vec<ItemRecord>, Vec<PropertyRecord>)> {
        Ok((
            query::load_items(&self.connection, items)?,
            query::load_properties_for_items(&self.connection, items, properties)?,
        ))
    }

    /// Finds a bounded page of values for one exact property definition.
    ///
    /// Results are ordered by item UUID; pass the last returned item as the next
    /// exclusive cursor. A page is capped at 256 records and 8 MiB of JSON.
    /// This is an unrestricted native storage operation.
    ///
    /// # Errors
    ///
    /// Returns a storage or malformed page-size error.
    pub fn find_properties(
        &self,
        key: &SchemaKey,
        after: Option<ItemId>,
        limit: usize,
    ) -> StorageResult<Vec<PropertyRecord>> {
        query::find_properties(&self.connection, key, after, limit)
    }

    /// Reads relations adjacent to any of the given objects in one SQL query.
    ///
    /// Relations are returned once, in ID order, even when both endpoints are
    /// present in the input batch. Both batch and result size are bounded.
    ///
    /// # Errors
    ///
    /// Returns invalid page/batch limits or a storage error.
    pub fn relations_touching_any(
        &self,
        targets: &[ItemId],
        after: Option<ItemId>,
        limit: usize,
    ) -> StorageResult<Vec<ItemRecord>> {
        query::relations_touching_any(&self.connection, targets, after, limit)
    }

    /// Finds a bounded directional relation page from the pinned snapshot.
    ///
    /// When `property` is supplied, only relation-items carrying that exact
    /// property name and version are returned.
    ///
    /// # Errors
    ///
    /// Returns invalid batch/page limits or a storage error.
    pub fn find_relations(
        &self,
        targets: &[ItemId],
        direction: RelationDirection,
        property: Option<&SchemaKey>,
        after: Option<ItemId>,
        limit: usize,
    ) -> StorageResult<Vec<ItemRecord>> {
        query::find_relations(&self.connection, targets, direction, property, after, limit)
    }

    /// Returns keys on the item and its incident relations that SQLite would
    /// remove as part of an item deletion.
    ///
    /// # Errors
    ///
    /// Returns an error if the pinned state contains invalid property keys.
    pub fn property_keys_deleted_with_item(&self, item: ItemId) -> StorageResult<Vec<SchemaKey>> {
        query::property_keys_deleted_with_item(&self.connection, item)
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
