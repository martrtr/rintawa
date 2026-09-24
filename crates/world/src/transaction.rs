//! Atomic transaction, event-log, and durable effect-job records.

use std::collections::HashSet;

use rintawa_sdk::world::{
    CommandId, CorrelationId, EffectJobId, PrincipalId, SchemaKey, UnixTimeMillis, WorldEventId,
    WorldId,
};
use serde::{Deserialize, Serialize};

use crate::{ActorRef, CausationRef, SchemaKind, SchemaRegistry, WorldMutation, WorldResult};

/// Current serialization format of append-only Core mutation log entries.
pub const WORLD_MUTATION_FORMAT_VERSION: u32 = 1;

/// Durable event proposed as part of one world transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldEventDraft {
    id: WorldEventId,
    schema: SchemaKey,
    payload: serde_json::Value,
}

impl WorldEventDraft {
    /// Creates a durable event draft with a new event identity.
    pub fn new(schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            id: WorldEventId::new(),
            schema,
            payload,
        }
    }

    /// Creates a durable event draft with an existing identity.
    pub const fn with_id(id: WorldEventId, schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            id,
            schema,
            payload,
        }
    }

    /// Returns the durable event identity.
    pub const fn id(&self) -> WorldEventId {
        self.id
    }

    /// Returns the event schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the event payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
}

/// Durable external effect proposed as part of one committed transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectJobDraft {
    id: EffectJobId,
    schema: SchemaKey,
    payload: serde_json::Value,
}

impl EffectJobDraft {
    /// Creates an effect job with a new idempotency identity.
    pub fn new(schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            id: EffectJobId::new(),
            schema,
            payload,
        }
    }

    /// Creates an effect job with an existing idempotency identity.
    pub const fn with_id(id: EffectJobId, schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            id,
            schema,
            payload,
        }
    }

    /// Returns the durable job and idempotency identity.
    pub const fn id(&self) -> EffectJobId {
        self.id
    }

    /// Returns the effect schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the effect payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
}

/// One atomic authoritative world transaction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldTransaction {
    mutations: Vec<WorldMutation>,
    events: Vec<WorldEventDraft>,
    effects: Vec<EffectJobDraft>,
}

impl WorldTransaction {
    /// Creates an empty transaction output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one current-state mutation.
    pub fn push_mutation(&mut self, mutation: WorldMutation) {
        self.mutations.push(mutation);
    }

    /// Adds one durable event.
    pub fn push_event(&mut self, event: WorldEventDraft) {
        self.events.push(event);
    }

    /// Adds one durable external effect job.
    pub fn push_effect(&mut self, effect: EffectJobDraft) {
        self.effects.push(effect);
    }

    /// Returns current-state mutations in application order.
    pub fn mutations(&self) -> &[WorldMutation] {
        &self.mutations
    }

    /// Returns durable event drafts in event order.
    pub fn events(&self) -> &[WorldEventDraft] {
        &self.events
    }

    /// Returns durable effect jobs in outbox order.
    pub fn effects(&self) -> &[EffectJobDraft] {
        &self.effects
    }

    /// Validates every schema reference and feature payload.
    ///
    /// # Errors
    ///
    /// Returns the first missing/wrong-kind schema or invalid payload.
    pub fn validate_schemas(&self, schemas: &SchemaRegistry) -> WorldResult<()> {
        if self.mutations.is_empty() && self.events.is_empty() && self.effects.is_empty() {
            return Err(crate::WorldError::EmptyTransaction);
        }

        let mut event_ids = HashSet::with_capacity(self.events.len());
        for event in &self.events {
            if !event_ids.insert(event.id()) {
                return Err(crate::WorldError::DuplicateEventId(event.id()));
            }
        }
        let mut effect_ids = HashSet::with_capacity(self.effects.len());
        for effect in &self.effects {
            if !effect_ids.insert(effect.id()) {
                return Err(crate::WorldError::DuplicateEffectJobId(effect.id()));
            }
        }

        for mutation in &self.mutations {
            match mutation {
                WorldMutation::CreateEntity { entity } => {
                    schemas.require_kind(entity.schema(), SchemaKind::Entity)?;
                }
                WorldMutation::DeleteEntity { .. } | WorldMutation::DeleteRelation { .. } => {}
                WorldMutation::CreateRelation { relation } => {
                    schemas.require_kind(relation.schema(), SchemaKind::Relation)?;
                }
                WorldMutation::SetFacet { facet } => {
                    schemas.validate_payload(facet.schema(), SchemaKind::Facet, facet.payload())?;
                }
                WorldMutation::RemoveFacet { schema, .. } => {
                    schemas.require_kind(schema, SchemaKind::Facet)?;
                }
                WorldMutation::GrantControl { grant } => {
                    if let Some(command_schemas) = grant.scope().exact_schemas() {
                        if command_schemas.is_empty() {
                            return Err(crate::WorldError::EmptyControlScope);
                        }
                        for command_schema in command_schemas {
                            schemas.require_kind(command_schema, SchemaKind::Command)?;
                        }
                    }
                }
                WorldMutation::RevokeControl { .. } => {}
            }
        }

        for event in &self.events {
            schemas.validate_payload(event.schema(), SchemaKind::Event, event.payload())?;
        }
        for effect in &self.effects {
            schemas.validate_payload(effect.schema(), SchemaKind::Effect, effect.payload())?;
        }
        Ok(())
    }
}

/// Whether a commit created a new world position or replayed an existing command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitDisposition {
    /// The transaction created a new authoritative commit.
    Committed,
    /// The same command identity had already committed and was returned idempotently.
    AlreadyCommitted,
}

/// Durable receipt for one command commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReceipt {
    disposition: CommitDisposition,
    command_id: CommandId,
    position: u64,
    event_ids: Vec<WorldEventId>,
    effect_job_ids: Vec<EffectJobId>,
}

impl CommitReceipt {
    /// Creates a durable commit receipt.
    pub fn new(
        disposition: CommitDisposition,
        command_id: CommandId,
        position: u64,
        event_ids: Vec<WorldEventId>,
        effect_job_ids: Vec<EffectJobId>,
    ) -> Self {
        Self {
            disposition,
            command_id,
            position,
            event_ids,
            effect_job_ids,
        }
    }

    /// Returns whether this call committed or replayed a previous command.
    pub const fn disposition(&self) -> CommitDisposition {
        self.disposition
    }

    /// Returns the command identity.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Returns the authoritative commit position.
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Returns durable event IDs in event order.
    pub fn event_ids(&self) -> &[WorldEventId] {
        &self.event_ids
    }

    /// Returns durable effect job IDs in outbox order.
    pub fn effect_job_ids(&self) -> &[EffectJobId] {
        &self.effect_job_ids
    }
}

/// One persisted generic Core mutation used for rebuild and audit.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredWorldMutation {
    /// Commit that made the mutation visible.
    pub commit_position: u64,
    /// Mutation order within the commit.
    pub mutation_index: u32,
    /// Version of the serialized mutation representation.
    pub format_version: u32,
    /// Generic mutation payload.
    pub mutation: WorldMutation,
}

/// Immutable command provenance attached to committed events and effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandProvenance {
    /// Stable command identity.
    pub command_id: CommandId,
    /// Versioned command schema.
    pub command_schema: SchemaKey,
    /// Authenticated outside principal.
    pub principal: PrincipalId,
    /// Role on whose behalf the command executed.
    pub actor: ActorRef,
    /// Direct causal predecessor, when present.
    pub causation: Option<CausationRef>,
    /// Stable operation-chain correlation identity.
    pub correlation_id: CorrelationId,
    /// Semantic effective time supplied by the caller, when present.
    pub effective_at: Option<UnixTimeMillis>,
    /// Host-recorded commit time.
    pub recorded_at: UnixTimeMillis,
}

/// One persisted durable world-event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredWorldEvent {
    /// Authoritative world that owns the event.
    pub world_id: WorldId,
    /// Durable event identity.
    pub id: WorldEventId,
    /// Commit that made the event visible.
    pub commit_position: u64,
    /// Event order within the commit.
    pub event_index: u32,
    /// Immutable command provenance.
    pub provenance: CommandProvenance,
    /// Versioned event schema.
    pub schema: SchemaKey,
    /// Validated event payload.
    pub payload: serde_json::Value,
}

/// One persisted pending external effect job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEffectJob {
    /// Authoritative world that owns the job.
    pub world_id: WorldId,
    /// Durable job identity and external idempotency key.
    pub id: EffectJobId,
    /// Commit that enqueued the effect.
    pub commit_position: u64,
    /// Effect order within the commit.
    pub job_index: u32,
    /// Immutable command provenance.
    pub provenance: CommandProvenance,
    /// Versioned effect schema.
    pub schema: SchemaKey,
    /// Validated effect payload.
    pub payload: serde_json::Value,
    /// Number of execution attempts recorded by the outbox worker.
    pub attempt_count: u32,
}
