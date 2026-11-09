//! Current-state records for items and properties.
//!
//! The persistent model has exactly one identity system: every object and every
//! relation is an [`ItemRecord`] with its own UUIDv7 [`ItemId`]. Relations are
//! items whose endpoints point at object-items, and all extension-defined data is
//! a [`PropertyRecord`] attached to an item by an exact versioned property name.

use rintawa_sdk::world::{ItemId, SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{WorldError, WorldResult};

/// Value of `items.type` for an object-item.
pub const ITEM_TYPE_OBJECT: i64 = 1;
/// Value of `items.type` for a relation-item.
pub const ITEM_TYPE_RELATION: i64 = 2;

/// Persistent kind of one item, stored as the integer `items.type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    /// A plain item that can carry properties and be a relation endpoint.
    Object,
    /// A directed link between two object-items, with its own identity.
    Relation,
}

/// Direction used when finding relation-items adjacent to object-items.
///
/// `Outgoing` matches relations whose subject is one of the supplied targets,
/// `Incoming` matches relations whose object is a target, and `Either` matches
/// either endpoint. Relations remain directed even when an `Either` query is
/// used; the returned [`ItemRecord`] always includes both endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationDirection {
    /// Match only relations directed away from a target.
    Outgoing,
    /// Match only relations directed toward a target.
    Incoming,
    /// Match relations in either direction.
    Either,
}

impl ItemKind {
    /// Returns the integer stored in `items.type`.
    pub const fn code(self) -> i64 {
        match self {
            Self::Object => ITEM_TYPE_OBJECT,
            Self::Relation => ITEM_TYPE_RELATION,
        }
    }

    /// Decodes an `items.type` value.
    ///
    /// # Errors
    ///
    /// Returns WorldError::InvalidItemKind for any value other than 1 or 2.
    pub fn from_code(code: i64) -> WorldResult<Self> {
        match code {
            ITEM_TYPE_OBJECT => Ok(Self::Object),
            ITEM_TYPE_RELATION => Ok(Self::Relation),
            other => Err(WorldError::InvalidItemKind(other)),
        }
    }
}

/// One item in authoritative current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRecord {
    id: ItemId,
    kind: ItemKind,
    subject: Option<ItemId>,
    object: Option<ItemId>,
}

impl ItemRecord {
    /// Creates an object-item with no endpoints.
    pub const fn object(id: ItemId) -> Self {
        Self {
            id,
            kind: ItemKind::Object,
            subject: None,
            object: None,
        }
    }

    /// Creates a relation-item between two object-items.
    ///
    /// Endpoint existence and endpoint kind are enforced by the authoritative
    /// store; this constructor only builds the record.
    pub const fn relation(id: ItemId, subject: ItemId, object: ItemId) -> Self {
        Self {
            id,
            kind: ItemKind::Relation,
            subject: Some(subject),
            object: Some(object),
        }
    }

    /// Returns the stable item identity.
    pub const fn id(&self) -> ItemId {
        self.id
    }

    /// Returns the persistent kind of this item.
    pub const fn kind(&self) -> ItemKind {
        self.kind
    }

    /// Returns the subject endpoint of a relation-item.
    pub const fn subject(&self) -> Option<ItemId> {
        self.subject
    }

    /// Returns the object endpoint of a relation-item.
    pub const fn object_target(&self) -> Option<ItemId> {
        self.object
    }

    /// Returns whether this item is a relation.
    pub const fn is_relation(&self) -> bool {
        matches!(self.kind, ItemKind::Relation)
    }
}

/// One current property value attached to an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyRecord {
    item: ItemId,
    key: SchemaKey,
    data: serde_json::Value,
}

impl PropertyRecord {
    /// Creates a property value for one item.
    pub fn new(item: ItemId, key: SchemaKey, data: serde_json::Value) -> Self {
        Self { item, key, data }
    }

    /// Returns the item that owns this property.
    pub const fn item(&self) -> ItemId {
        self.item
    }

    /// Returns the exact versioned property name.
    pub const fn key(&self) -> &SchemaKey {
        &self.key
    }

    /// Returns the property value.
    pub const fn data(&self) -> &serde_json::Value {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_round_trip_item_kind_codes() -> WorldResult<()> {
        assert_eq!(ItemKind::from_code(1)?, ItemKind::Object);
        assert_eq!(ItemKind::from_code(2)?, ItemKind::Relation);
        assert_eq!(ItemKind::Object.code(), 1);
        assert_eq!(ItemKind::Relation.code(), 2);
        assert!(matches!(
            ItemKind::from_code(3),
            Err(WorldError::InvalidItemKind(3))
        ));
        Ok(())
    }

    #[test]
    fn test_should_keep_endpoints_only_on_relations() {
        let item = ItemId::new();
        let object = ItemRecord::object(item);
        let relation = ItemRecord::relation(ItemId::new(), item, item);

        assert!(!object.is_relation());
        assert_eq!(object.subject(), None);
        assert_eq!(object.object_target(), None);
        assert!(relation.is_relation());
        assert_eq!(relation.subject(), Some(item));
        assert_eq!(relation.object_target(), Some(item));
    }
}
