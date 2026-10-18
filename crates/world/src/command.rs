//! Submitted command envelope and causal metadata.

use rintawa_sdk::world::{
    CommandId, CorrelationId, EffectJobId, PrincipalId, SchemaKey, UnixTimeMillis, WorldEventId,
};
use serde::{Deserialize, Serialize};

use crate::{ActorRef, SchemaKind, SchemaRegistry, WorldResult};

/// Durable reference to the operation that directly caused another command/event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum CausationRef {
    /// Another submitted command.
    Command(CommandId),
    /// A previously committed world event.
    Event(WorldEventId),
    /// A durable external effect result.
    Effect(EffectJobId),
}

/// Immutable input envelope submitted to one authoritative world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldCommand {
    id: CommandId,
    schema: SchemaKey,
    principal: PrincipalId,
    actor: ActorRef,
    expected_position: Option<u64>,
    causation: Option<CausationRef>,
    correlation_id: CorrelationId,
    effective_at: Option<UnixTimeMillis>,
    payload: serde_json::Value,
}

impl WorldCommand {
    /// Creates a new command and correlation identity.
    pub fn new(
        schema: SchemaKey,
        principal: PrincipalId,
        actor: ActorRef,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: CommandId::new(),
            schema,
            principal,
            actor,
            expected_position: None,
            causation: None,
            correlation_id: CorrelationId::new(),
            effective_at: None,
            payload,
        }
    }

    /// Creates a command with externally preserved durable identities.
    pub fn with_ids(
        id: CommandId,
        correlation_id: CorrelationId,
        schema: SchemaKey,
        principal: PrincipalId,
        actor: ActorRef,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id,
            schema,
            principal,
            actor,
            expected_position: None,
            causation: None,
            correlation_id,
            effective_at: None,
            payload,
        }
    }

    /// Requires the world to still be at the supplied snapshot position.
    pub const fn expecting_position(mut self, position: u64) -> Self {
        self.expected_position = Some(position);
        self
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

    /// Returns the stable command idempotency identity.
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

    /// Returns the role on whose behalf the command executes.
    pub const fn actor(&self) -> ActorRef {
        self.actor
    }

    /// Returns the optional optimistic-concurrency snapshot position.
    pub const fn expected_position(&self) -> Option<u64> {
        self.expected_position
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

    /// Validates this command payload against the registered command schema.
    ///
    /// # Errors
    ///
    /// Returns a missing/wrong-kind schema or payload validation error.
    pub fn validate_schema(&self, schemas: &SchemaRegistry) -> WorldResult<()> {
        schemas.validate_payload(self.schema(), SchemaKind::Command, self.payload())
    }
}
