//! Read-only authoritative snapshots for Host and owner-scoped command Systems.

use std::{collections::BTreeMap, sync::Arc};

use rintawa_sdk::{
    types::ExtensionId,
    world::{ItemId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldSnapshot;
use rintawa_world::{ItemRecord, PropertyRecord};

use crate::{WorldReadError, WorldReadResult};

/// Read-only view evaluated by one System against one pinned SQLite WAL snapshot.
///
/// The single-writer runtime guarantees that no other writer is active while a
/// System evaluates, so the snapshot cannot drift between the reads a System
/// performs and the commit of its output.
#[derive(Debug)]
pub struct WorldSnapshot {
    storage: SqliteWorldSnapshot,
    // None is reserved for trusted native Host reads; System reads always carry
    // an owner from the registered command schema, never the submitted Principal.
    access: Option<SystemPropertyAccess>,
}

#[derive(Debug)]
struct SystemPropertyAccess {
    owner: ExtensionId,
    // Immutable per-command view; a later Host registration cannot change the
    // authority of an already-running System evaluation.
    active_owners: Arc<BTreeMap<SchemaKey, ExtensionId>>,
}

impl WorldSnapshot {
    pub(crate) const fn new(storage: SqliteWorldSnapshot) -> Self {
        Self {
            storage,
            access: None,
        }
    }

    pub(crate) fn for_system(
        storage: SqliteWorldSnapshot,
        owner: ExtensionId,
        active_owners: Arc<BTreeMap<SchemaKey, ExtensionId>>,
    ) -> Self {
        Self {
            storage,
            access: Some(SystemPropertyAccess {
                owner,
                active_owners,
            }),
        }
    }

    pub(crate) fn permits_property(&self, key: &SchemaKey) -> bool {
        self.access
            .as_ref()
            .is_none_or(|access| access.active_owners.get(key) == Some(&access.owner))
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
    /// Rejects property keys not owned by the active System's extension.
    /// Trusted native Host snapshots are unrestricted.
    ///
    /// # Errors
    ///
    /// Returns PropertyAccessDenied before any storage read for an unapproved key,
    /// or a read error when persistent state cannot be decoded.
    pub fn load_property(
        &self,
        item: ItemId,
        key: &SchemaKey,
    ) -> WorldReadResult<Option<PropertyRecord>> {
        if !self.permits_property(key) {
            return Err(WorldReadError::PropertyAccessDenied(key.clone()));
        }
        Ok(self.storage.load_property(item, key)?)
    }

    /// Loads the properties visible to the active System owner.
    ///
    /// Trusted native Host snapshots can inspect all properties. For Systems,
    /// no unauthorized property value or version is returned.
    ///
    /// # Errors
    ///
    /// Returns a read error when persistent state cannot be decoded.
    pub fn properties_of(&self, item: ItemId) -> WorldReadResult<Vec<PropertyRecord>> {
        let mut properties = self.storage.properties_of(item)?;
        properties.retain(|property| self.permits_property(property.key()));
        Ok(properties)
    }

    /// Finds a bounded keyset page for one active property definition.
    ///
    /// System snapshots reject foreign property keys before the SQLite query,
    /// including when no matching item exists. Host snapshots are unrestricted.
    /// Results are bounded by 256 entries and 8 MiB of JSON per page.
    ///
    /// # Errors
    ///
    /// Returns PropertyAccessDenied or a storage/page-size error.
    pub fn find_properties(
        &self,
        key: &SchemaKey,
        after: Option<ItemId>,
        limit: usize,
    ) -> WorldReadResult<Vec<PropertyRecord>> {
        if !self.permits_property(key) {
            return Err(WorldReadError::PropertyAccessDenied(key.clone()));
        }
        Ok(self.storage.find_properties(key, after, limit)?)
    }

    /// Reads a bounded page of relations adjacent to any supplied item.
    ///
    /// Graph structure is shared state; property values remain owner-scoped.
    ///
    /// # Errors
    ///
    /// Returns a read error on invalid batch/page limits or malformed state.
    pub fn relations_touching_any(
        &self,
        targets: &[ItemId],
        after: Option<ItemId>,
        limit: usize,
    ) -> WorldReadResult<Vec<ItemRecord>> {
        Ok(self.storage.relations_touching_any(targets, after, limit)?)
    }

    // Deleting an object cascades to its relation-items and their properties.
    // Check all distinct keys using one pinned SQLite read before SQL DELETE;
    // a System must not erase foreign values through ON DELETE CASCADE.
    pub(crate) fn foreign_property_on_delete(
        &self,
        item: ItemId,
    ) -> WorldReadResult<Option<SchemaKey>> {
        for key in self.storage.property_keys_deleted_with_item(item)? {
            if !self.permits_property(&key) {
                return Ok(Some(key));
            }
        }
        Ok(None)
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
