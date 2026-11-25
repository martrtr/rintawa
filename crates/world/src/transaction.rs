//! One in-memory atomic change set.
//!
//! There is no durable command, mutation, event or effect log. A change set is
//! applied in one SQLite transaction and the reply is produced only after commit.

use std::collections::BTreeSet;

use rintawa_sdk::world::{ItemId, SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{PropertyRecord, PropertyRegistry, WorldError, WorldResult};

/// One current-state change inside an atomic world transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "operation")]
pub enum StateChange {
    /// Creates one object-item.
    CreateObject {
        /// Identity of the object to create.
        id: ItemId,
    },
    /// Creates one relation-item between two object-items.
    CreateRelation {
        /// Identity of the relation to create.
        id: ItemId,
        /// Subject endpoint, which must be an existing object-item.
        subject: ItemId,
        /// Object endpoint, which must be an existing object-item.
        object: ItemId,
    },
    /// Deletes one item, the relations that reference it, and their properties.
    DeleteItem {
        /// Item identity to delete.
        id: ItemId,
    },
    /// Inserts or replaces the current value of one property.
    SetProperty {
        /// Item that owns the property.
        item: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
        /// Validated property value.
        data: serde_json::Value,
    },
    /// Removes one property when present.
    RemoveProperty {
        /// Item that owns the property.
        item: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
    },
}

/// One atomic authoritative world transaction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldTransaction {
    changes: Vec<StateChange>,
}

impl WorldTransaction {
    /// Creates an empty transaction output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one current-state change.
    pub fn push_change(&mut self, change: StateChange) {
        self.changes.push(change);
    }

    /// Returns current-state changes in application order.
    pub fn changes(&self) -> &[StateChange] {
        &self.changes
    }

    /// Returns whether the transaction changes no current state.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Validates property values against the registered property vocabulary.
    ///
    /// # Errors
    ///
    /// Returns EmptyTransaction when no current-state change is present,
    /// DuplicateItemCreation for a repeated item identity, or a definition and
    /// payload validation error for an unregistered or malformed property.
    pub fn validate(&self, registry: &PropertyRegistry) -> WorldResult<()> {
        if self.changes.is_empty() {
            return Err(WorldError::EmptyTransaction);
        }

        let mut created = BTreeSet::new();
        for change in &self.changes {
            match change {
                StateChange::CreateObject { id } | StateChange::CreateRelation { id, .. } => {
                    if !created.insert(*id) {
                        return Err(WorldError::DuplicateItemCreation(*id));
                    }
                }
                StateChange::DeleteItem { .. } => {}
                StateChange::SetProperty { key, data, .. } => registry.validate(key, data)?,
                StateChange::RemoveProperty { .. } => {}
            }
        }
        Ok(())
    }

    /// Returns the property value written by this transaction, when present.
    pub fn property(&self) -> Option<PropertyRecord> {
        self.changes.iter().find_map(|change| match change {
            StateChange::SetProperty { item, key, data } => {
                Some(PropertyRecord::new(*item, key.clone(), data.clone()))
            }
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use rintawa_sdk::types::ExtensionId;

    use super::*;
    use crate::PropertyDefinition;

    fn registry() -> PropertyRegistry {
        let mut registry = PropertyRegistry::new();
        registry
            .register(PropertyDefinition::new(
                "character.identity@1".parse().expect("valid key"),
                ExtensionId::new("example.extension"),
                serde_json::json!({
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            ))
            .expect("definition must register");
        registry
    }

    #[test]
    fn test_should_reject_empty_transaction() {
        let transaction = WorldTransaction::new();
        assert!(matches!(
            transaction.validate(&registry()),
            Err(WorldError::EmptyTransaction)
        ));
    }

    #[test]
    fn test_should_reject_duplicate_item_creation() {
        let id = ItemId::new();
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id });
        transaction.push_change(StateChange::CreateObject { id });

        assert!(matches!(
            transaction.validate(&registry()),
            Err(WorldError::DuplicateItemCreation(duplicate)) if duplicate == id
        ));
    }

    #[test]
    fn test_should_validate_property_values_before_write() {
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::SetProperty {
            item: ItemId::new(),
            key: "character.identity@1".parse().expect("valid key"),
            data: serde_json::json!({ "name": 7 }),
        });

        assert!(matches!(
            transaction.validate(&registry()),
            Err(WorldError::PropertyValidationFailed { .. })
        ));
    }
}
