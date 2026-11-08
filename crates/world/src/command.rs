//! Submitted command envelope and causal metadata.
//!
//! A command is an in-memory request. It is not persisted, has no stored receipt
//! and no optimistic position: concurrency is controlled by the single-writer
//! queue, and atomicity by one SQLite transaction per accepted change.

use rintawa_sdk::world::{CommandId, CorrelationId, PrincipalId, SchemaKey, UnixTimeMillis};
use serde::{Deserialize, Serialize};

use crate::{SchemaRegistry, WorldResult};

/// Direct cause of one command, when it was issued by another command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum CausationRef {
    /// Another submitted command.
    Command(CommandId),
}

/// Immutable input envelope submitted to one authoritative world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldCommand {
    id: CommandId,
    schema: SchemaKey,
    principal: PrincipalId,
    causation: Option<CausationRef>,
    correlation_id: CorrelationId,
    effective_at: Option<UnixTimeMillis>,
    payload: serde_json::Value,
}

impl WorldCommand {
    /// Creates a new command and correlation identity.
    pub fn new(schema: SchemaKey, principal: PrincipalId, payload: serde_json::Value) -> Self {
        Self {
            id: CommandId::new(),
            schema,
            principal,
            causation: None,
            correlation_id: CorrelationId::new(),
            effective_at: None,
            payload,
        }
    }

    /// Creates a command with externally preserved identities.
    pub fn with_ids(
        id: CommandId,
        correlation_id: CorrelationId,
        schema: SchemaKey,
        principal: PrincipalId,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id,
            schema,
            principal,
            causation: None,
            correlation_id,
            effective_at: None,
            payload,
        }
    }

    /// Attaches direct causal provenance.
    pub const fn caused_by(mut self, causation: CausationRef) -> Self {
        self.causation = Some(causation);
        self
    }

    /// Sets semantic effective time without affecting commit ordering.
    pub const fn effective_at(mut self, timestamp: UnixTimeMillis) -> Self {
        self.effective_at = Some(timestamp);
        self
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

    /// Returns direct causal provenance, if any.
    pub const fn causation(&self) -> Option<CausationRef> {
        self.causation
    }

    /// Returns the stable correlation identity shared by one operation chain.
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns semantic effective time, if supplied.
    pub const fn effective_at_timestamp(&self) -> Option<UnixTimeMillis> {
        self.effective_at
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
