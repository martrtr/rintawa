//! Versioned world-schema definitions and registry.

use std::collections::BTreeMap;

use jsonschema::Validator;
pub use rintawa_sdk::world::SchemaKind;
use rintawa_sdk::{types::ExtensionId, world::SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{WorldError, WorldResult};

/// Complete immutable definition of one versioned world schema.
///
/// The definition is a JSON Schema document. Rintawa compiles it locally when
/// registered; external HTTP/file schema resolution is intentionally unavailable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    key: SchemaKey,
    kind: SchemaKind,
    owner: ExtensionId,
    definition: serde_json::Value,
}

impl SchemaDefinition {
    /// Creates a schema definition owned by one extension.
    pub fn new(
        key: SchemaKey,
        kind: SchemaKind,
        owner: ExtensionId,
        definition: serde_json::Value,
    ) -> Self {
        Self {
            key,
            kind,
            owner,
            definition,
        }
    }

    /// Returns the stable versioned schema identity.
    pub const fn key(&self) -> &SchemaKey {
        &self.key
    }

    /// Returns the semantic schema category.
    pub const fn kind(&self) -> SchemaKind {
        self.kind
    }

    /// Returns the extension that owns this schema definition.
    pub const fn owner(&self) -> &ExtensionId {
        &self.owner
    }

    /// Returns the extension-defined JSON Schema document.
    pub const fn definition(&self) -> &serde_json::Value {
        &self.definition
    }
}

/// Result category for one schema registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaRegistration {
    /// A previously unknown schema definition entered the registry.
    Registered,
    /// The exact same definition was already registered.
    AlreadyPresent,
}

/// In-memory registry of immutable versioned world schemas and compiled validators.
#[derive(Debug, Clone, Default)]
pub struct SchemaRegistry {
    definitions: BTreeMap<SchemaKey, SchemaDefinition>,
    validators: BTreeMap<SchemaKey, Validator>,
}

impl SchemaRegistry {
    /// Creates an empty schema registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one immutable schema definition.
    ///
    /// Re-registering the exact same definition is idempotent, which permits
    /// extensions to activate again after a world restart.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the key exists with different semantics, or
    /// InvalidSchemaDefinition when the JSON Schema cannot be compiled locally.
    pub fn register(&mut self, definition: SchemaDefinition) -> WorldResult<SchemaRegistration> {
        if let Some(existing) = self.definitions.get(definition.key()) {
            if existing == &definition {
                return Ok(SchemaRegistration::AlreadyPresent);
            }
            return Err(WorldError::SchemaConflict {
                key: definition.key().clone(),
                existing_owner: existing.owner().to_string(),
                incoming_owner: definition.owner().to_string(),
            });
        }

        let validator = jsonschema::validator_for(definition.definition()).map_err(|error| {
            WorldError::InvalidSchemaDefinition {
                key: definition.key().clone(),
                reason: error.masked().to_string(),
            }
        })?;

        let key = definition.key().clone();
        self.definitions.insert(key.clone(), definition);
        self.validators.insert(key, validator);
        Ok(SchemaRegistration::Registered)
    }

    /// Returns one registered schema definition.
    pub fn get(&self, key: &SchemaKey) -> Option<&SchemaDefinition> {
        self.definitions.get(key)
    }

    /// Requires a registered schema to have a particular semantic kind.
    ///
    /// # Errors
    ///
    /// Returns SchemaNotRegistered or SchemaKindMismatch.
    pub fn require_kind(&self, key: &SchemaKey, expected: SchemaKind) -> WorldResult<()> {
        let definition = self
            .definitions
            .get(key)
            .ok_or_else(|| WorldError::SchemaNotRegistered { key: key.clone() })?;
        if definition.kind() != expected {
            return Err(WorldError::SchemaKindMismatch {
                key: key.clone(),
                expected,
                actual: definition.kind(),
            });
        }
        Ok(())
    }

    /// Validates a feature payload against a registered schema of the expected kind.
    ///
    /// # Errors
    ///
    /// Returns a registry/kind error or a masked validation error.
    pub fn validate_payload(
        &self,
        key: &SchemaKey,
        expected: SchemaKind,
        payload: &serde_json::Value,
    ) -> WorldResult<()> {
        self.require_kind(key, expected)?;
        let validator = self
            .validators
            .get(key)
            .ok_or_else(|| WorldError::SchemaNotRegistered { key: key.clone() })?;

        if let Err(error) = validator.validate(payload) {
            return Err(WorldError::PayloadValidationFailed {
                key: key.clone(),
                instance_path: error.instance_path().to_string(),
                reason: error.masked().to_string(),
            });
        }
        Ok(())
    }

    /// Returns registered definitions in deterministic key order.
    pub fn iter(&self) -> impl Iterator<Item = &SchemaDefinition> {
        self.definitions.values()
    }

    /// Returns the number of registered schema definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Returns whether no schema definitions are registered.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use rintawa_sdk::world::SchemaKey;

    use super::*;

    fn definition(owner: &str, marker: u64) -> SchemaDefinition {
        SchemaDefinition::new(
            "rintawa.test.facet@1".parse::<SchemaKey>().unwrap(),
            SchemaKind::Facet,
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
    fn test_should_register_same_schema_idempotently() -> WorldResult<()> {
        let mut registry = SchemaRegistry::new();
        let definition = definition("example.extension", 1);

        assert_eq!(
            registry.register(definition.clone())?,
            SchemaRegistration::Registered
        );
        assert_eq!(
            registry.register(definition)?,
            SchemaRegistration::AlreadyPresent
        );
        assert_eq!(registry.len(), 1);
        Ok(())
    }

    #[test]
    fn test_should_reject_conflicting_schema_definition() -> WorldResult<()> {
        let mut registry = SchemaRegistry::new();
        registry.register(definition("example.first", 1))?;

        let error = registry
            .register(definition("example.second", 2))
            .unwrap_err();

        assert!(matches!(error, WorldError::SchemaConflict { .. }));
        assert_eq!(registry.len(), 1);
        Ok(())
    }

    #[test]
    fn test_should_validate_payload_without_exposing_rejected_value() -> WorldResult<()> {
        let mut registry = SchemaRegistry::new();
        let definition = definition("example.extension", 7);
        let key = definition.key().clone();
        registry.register(definition)?;

        registry.validate_payload(&key, SchemaKind::Facet, &serde_json::json!({ "marker": 7 }))?;
        let error = registry
            .validate_payload(
                &key,
                SchemaKind::Facet,
                &serde_json::json!({ "marker": "secret-value" }),
            )
            .unwrap_err();
        let rendered = error.to_string();

        assert!(matches!(error, WorldError::PayloadValidationFailed { .. }));
        assert!(!rendered.contains("secret-value"));
        Ok(())
    }
}
