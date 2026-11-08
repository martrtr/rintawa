//! Submitted command envelope for an authoritative world.
//!
//! A command is an in-memory request. It is not persisted, has no stored receipt
//! and no optimistic position: concurrency is controlled by the single-writer
//! queue, and atomicity by one SQLite transaction per accepted change.

use rintawa_sdk::world::{CommandId, PrincipalId, SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{SchemaRegistry, WorldResult};

/// Immutable input envelope submitted to one authoritative world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldCommand {
    id: CommandId,
    schema: SchemaKey,
    principal: PrincipalId,
    payload: serde_json::Value,
}

impl WorldCommand {
    /// Creates a new command identity.
    pub fn new(schema: SchemaKey, principal: PrincipalId, payload: serde_json::Value) -> Self {
        Self {
            id: CommandId::new(),
            schema,
            principal,
            payload,
        }
    }

    /// Returns the command identity used for logs and correlation.
    pub const fn id(&self) -> CommandId {
        self.id
    }

    /// Returns the versioned command schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the authenticated outside principal.
    pub const fn principal(&self) -> PrincipalId {
        self.principal
    }

    /// Returns the immutable command payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Validates this command payload against the registered runtime command schema.
    ///
    /// Command schemas are extension runtime API descriptions; they are never
    /// stored in the property vocabulary.
    ///
    /// # Errors
    ///
    /// Returns a missing definition or payload validation error.
    pub fn validate_schema(&self, schemas: &SchemaRegistry) -> WorldResult<()> {
        schemas.validate_payload(&self.schema, crate::SchemaKind::Command, &self.payload)
    }
}
