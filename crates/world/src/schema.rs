//! The world property vocabulary: immutable versioned JSON Schema definitions.
//!
//! A property definition is the replacement for the retired entity/relation type
//! schemas and facets. `name` is unversioned (for example `psychology.mood`);
//! `version` is a positive integer; the definition body is a JSON Schema document
//! compiled locally when it is registered. Dotted names are flat identifiers.

use std::collections::BTreeMap;

use jsonschema::Validator;
use rintawa_sdk::{types::ExtensionId, world::SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{WorldError, WorldResult};

/// Complete immutable definition of one versioned property.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyDefinition {
    key: SchemaKey,
    owner: ExtensionId,
    schema: serde_json::Value,
}

impl PropertyDefinition {
    /// Creates a property definition owned by one extension.
    pub fn new(key: SchemaKey, owner: ExtensionId, schema: serde_json::Value) -> Self {
        Self { key, owner, schema }
    }

    /// Returns the exact versioned property name.
    pub const fn key(&self) -> &SchemaKey {
        &self.key
    }

    /// Returns the extension that owns this definition.
    pub const fn owner(&self) -> &ExtensionId {
        &self.owner
    }

    /// Returns the JSON Schema document.
    pub const fn schema(&self) -> &serde_json::Value {
        &self.schema
    }
}

/// Result category for one property-definition registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionRegistration {
    /// A previously unknown definition entered the vocabulary.
    Registered,
    /// The exact same definition was already registered.
    AlreadyPresent,
}

/// In-memory property vocabulary with compiled JSON Schema validators.
#[derive(Debug, Clone, Default)]
pub struct PropertyRegistry {
    definitions: BTreeMap<SchemaKey, PropertyDefinition>,
    validators: BTreeMap<SchemaKey, Validator>,
}

impl PropertyRegistry {
    /// Creates an empty property vocabulary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one immutable property definition.
    ///
    /// Re-registering the exact same definition is idempotent, so an extension can
    /// activate again after a world restart. Mutating an existing `(name, version)`
    /// is rejected.
    ///
    /// # Errors
    ///
    /// Returns DefinitionConflict when the key exists with other semantics, or
    /// InvalidDefinition when the JSON Schema cannot be compiled locally.
    pub fn register(
        &mut self,
        definition: PropertyDefinition,
    ) -> WorldResult<DefinitionRegistration> {
        if let Some(existing) = self.definitions.get(definition.key()) {
            // The definition is the JSON Schema document. Ownership is Host
            // composition metadata that is not persisted in `property_registry`,
            // so re-registering the same document from an installed package after a
            // restart must stay idempotent.
            if existing.schema() == definition.schema() {
                return Ok(DefinitionRegistration::AlreadyPresent);
            }
            return Err(WorldError::DefinitionConflict {
                key: definition.key().clone(),
                existing_owner: existing.owner().to_string(),
                incoming_owner: definition.owner().to_string(),
            });
        }

        let validator = jsonschema::validator_for(definition.schema()).map_err(|error| {
            WorldError::InvalidDefinition {
                key: definition.key().clone(),
                reason: error.masked().to_string(),
            }
        })?;

        let key = definition.key().clone();
        self.definitions.insert(key.clone(), definition);
        self.validators.insert(key, validator);
        Ok(DefinitionRegistration::Registered)
    }

    /// Returns one registered definition.
    pub fn get(&self, key: &SchemaKey) -> Option<&PropertyDefinition> {
        self.definitions.get(key)
    }

    /// Validates a property value against its registered definition.
    ///
    /// # Errors
    ///
    /// Returns DefinitionNotRegistered when the exact versioned name is unknown,
    /// or PropertyValidationFailed when the value does not match the schema.
    pub fn validate(&self, key: &SchemaKey, data: &serde_json::Value) -> WorldResult<()> {
        let validator = self
            .validators
            .get(key)
            .ok_or_else(|| WorldError::DefinitionNotRegistered { key: key.clone() })?;

        if let Err(error) = validator.validate(data) {
            return Err(WorldError::PropertyValidationFailed {
                key: key.clone(),
                instance_path: error.instance_path().to_string(),
                reason: error.masked().to_string(),
            });
        }
        Ok(())
    }

    /// Returns registered definitions in deterministic key order.
    pub fn iter(&self) -> impl Iterator<Item = &PropertyDefinition> {
        self.definitions.values()
    }

    /// Returns the number of registered property definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Returns whether no property definitions are registered.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use rintawa_sdk::world::SchemaKey;

    use super::*;

    fn definition(owner: &str, marker: u64) -> PropertyDefinition {
        PropertyDefinition::new(
            "psychology.mood@1".parse::<SchemaKey>().unwrap(),
            ExtensionId::new(owner),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "marker": { "const": marker }
                },
                "required": ["marker"],
                "additionalProperties": false
            }),
        )
    }

    #[test]
    fn test_should_register_same_definition_idempotently() -> WorldResult<()> {
        let mut registry = PropertyRegistry::new();
        let definition = definition("example.extension", 1);

        assert_eq!(
            registry.register(definition.clone())?,
            DefinitionRegistration::Registered
        );
        assert_eq!(
            registry.register(definition)?,
            DefinitionRegistration::AlreadyPresent
        );
        assert_eq!(registry.len(), 1);
        Ok(())
    }

    #[test]
    fn test_should_reject_conflicting_definition() -> WorldResult<()> {
        let mut registry = PropertyRegistry::new();
        registry.register(definition("example.first", 1))?;

        let error = registry
            .register(definition("example.second", 2))
            .unwrap_err();

        assert!(matches!(error, WorldError::DefinitionConflict { .. }));
        assert_eq!(registry.len(), 1);
        Ok(())
    }

    #[test]
    fn test_should_validate_value_without_exposing_rejected_data() -> WorldResult<()> {
        let mut registry = PropertyRegistry::new();
        let definition = definition("example.extension", 7);
        let key = definition.key().clone();
        registry.register(definition)?;

        registry.validate(&key, &serde_json::json!({ "marker": 7 }))?;
        let error = registry
            .validate(&key, &serde_json::json!({ "marker": "secret-value" }))
            .unwrap_err();
        let rendered = error.to_string();

        assert!(matches!(error, WorldError::PropertyValidationFailed { .. }));
        assert!(!rendered.contains("secret-value"));
        Ok(())
    }
}
