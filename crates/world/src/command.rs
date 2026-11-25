//! Submitted command envelope for an authoritative world.
//!
//! A command is an in-memory request. It is not persisted, has no stored receipt
//! and no optimistic position: concurrency is controlled by the single-writer
//! queue, and atomicity by one SQLite transaction per accepted change.

use rintawa_sdk::world::{CommandId, PrincipalId, SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{SchemaRegistry, WorldResult};

/// Origin attested by the Host for one submitted World command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorldCommandOrigin {
    /// A Host-approved active System submitted this command.
    System,
    /// A trusted Host ingress authenticated this outside principal.
    Interactive(PrincipalId),
}

/// Immutable input envelope submitted to one authoritative world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldCommand {
    id: CommandId,
    schema: SchemaKey,
    origin: WorldCommandOrigin,
    payload: serde_json::Value,
}

impl WorldCommand {
    /// Creates a command submitted by one Host-approved active System.
    pub fn system(schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            id: CommandId::new(),
            schema,
            origin: WorldCommandOrigin::System,
            payload,
        }
    }

    /// Creates a command from an authenticated outside principal.
    pub fn interactive(
        schema: SchemaKey,
        principal: PrincipalId,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: CommandId::new(),
            schema,
            origin: WorldCommandOrigin::Interactive(principal),
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

    /// Returns the Host-attested command origin.
    pub const fn origin(&self) -> WorldCommandOrigin {
        self.origin
    }

    /// Returns the authenticated outside principal for an interactive command.
    pub const fn interactive_principal(&self) -> Option<PrincipalId> {
        match self.origin {
            WorldCommandOrigin::System => None,
            WorldCommandOrigin::Interactive(principal) => Some(principal),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_keep_system_and_interactive_origins_distinct() {
        let schema: SchemaKey = "test.command@1".parse().expect("test schema must be valid");
        let system = WorldCommand::system(schema.clone(), serde_json::json!({}));
        assert_eq!(system.origin(), WorldCommandOrigin::System);
        assert_eq!(system.interactive_principal(), None);

        let principal = PrincipalId::new();
        let interactive = WorldCommand::interactive(schema, principal, serde_json::json!({}));
        assert_eq!(
            interactive.origin(),
            WorldCommandOrigin::Interactive(principal)
        );
        assert_eq!(interactive.interactive_principal(), Some(principal));
    }
}
