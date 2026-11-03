//! Transient extension runtime-API schemas.
//!
//! Command, projection and runtime-signal schemas describe extension-provided
//! operation payloads. They are compiled in memory from exact installed packages
//! and are deliberately **not** persisted in `property_registry`, which holds
//! property definitions only.

use std::collections::BTreeMap;

use jsonschema::Validator;
pub use rintawa_sdk::world::SchemaKind;
use rintawa_sdk::{types::ExtensionId, world::SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{WorldError, WorldResult};

/// Complete immutable definition of one versioned runtime API schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSchemaDefinition {
    key: SchemaKey,
    kind: SchemaKind,
    owner: ExtensionId,
    definition: serde_json::Value,
}

impl RuntimeSchemaDefinition {
    /// Creates a runtime schema definition owned by one extension.
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

/// Result category for one runtime schema registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSchemaRegistration {
    /// A previously unknown schema definition entered the registry.
    Registered,
    /// The exact same definition was already registered.
    AlreadyPresent,
}

/// In-memory registry of immutable versioned runtime API schemas.
#[derive(Debug, Clone, Default)]
pub struct SchemaRegistry {
    definitions: BTreeMap<SchemaKey, RuntimeSchemaDefinition>,
    validators: BTreeMap<SchemaKey, Validator>,
}

impl SchemaRegistry {
    /// Creates an empty schema registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one immutable runtime schema definition.
    ///
    /// Re-registering the exact same definition is idempotent, which permits
    /// extensions to activate again after a world restart.
    ///
    /// # Errors
    ///
    /// Returns DefinitionConflict when the key exists with different semantics, or
    /// InvalidDefinition when the JSON Schema cannot be compiled locally.
    pub fn register(
        &mut self,
        definition: RuntimeSchemaDefinition,
    ) -> WorldResult<RuntimeSchemaRegistration> {
        if let Some(existing) = self.definitions.get(definition.key()) {
            if existing == &definition {
                return Ok(RuntimeSchemaRegistration::AlreadyPresent);
            }
            return Err(WorldError::DefinitionConflict {
                key: definition.key().clone(),
                existing_owner: existing.owner().to_string(),
                incoming_owner: definition.owner().to_string(),
            });
        }

        let validator = jsonschema::validator_for(definition.definition()).map_err(|error| {
            WorldError::InvalidDefinition {
                key: definition.key().clone(),
                reason: error.masked().to_string(),
            }
        })?;

        let key = definition.key().clone();
        self.definitions.insert(key.clone(), definition);
        self.validators.insert(key, validator);
        Ok(RuntimeSchemaRegistration::Registered)
    }

    /// Returns one registered runtime schema definition.
    pub fn get(&self, key: &SchemaKey) -> Option<&RuntimeSchemaDefinition> {
        self.definitions.get(key)
    }

    /// Requires a registered runtime schema to have a particular semantic kind.
    ///
    /// # Errors
    ///
    /// Returns DefinitionNotRegistered or DefinitionKindMismatch.
    pub fn require_kind(&self, key: &SchemaKey, expected: SchemaKind) -> WorldResult<()> {
        let definition = self
            .definitions
            .get(key)
            .ok_or_else(|| WorldError::DefinitionNotRegistered { key: key.clone() })?;
        if definition.kind() != expected {
            return Err(WorldError::DefinitionKindMismatch {
                key: key.clone(),
                expected,
                actual: definition.kind(),
            });
        }
        Ok(())
    }

    /// Validates a runtime payload against a registered schema of the expected kind.
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
            .ok_or_else(|| WorldError::DefinitionNotRegistered { key: key.clone() })?;

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
    pub fn iter(&self) -> impl Iterator<Item = &RuntimeSchemaDefinition> {
        self.definitions.values()
    }

    /// Returns the number of registered runtime schema definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Returns whether no runtime schema definitions are registered.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}
