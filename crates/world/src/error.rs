//! Errors returned by the authoritative world model.

use rintawa_sdk::world::{
    CommandId, ControlGrantId, EffectJobId, EntityId, PrincipalId, RelationId, SchemaKey,
    WorldEventId,
};
use thiserror::Error;

use crate::{ActorRef, SchemaKind};

/// Errors returned by feature-neutral world operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// The same schema key was registered with different semantics.
    #[error(
        "schema {key} conflicts with the existing definition owned by {existing_owner}; incoming owner is {incoming_owner}"
    )]
    SchemaConflict {
        /// Versioned schema identity that conflicts.
        key: SchemaKey,
        /// Owner of the already registered definition.
        existing_owner: String,
        /// Owner attempting to register incompatible semantics.
        incoming_owner: String,
    },

    /// A schema document cannot be compiled as local JSON Schema.
    #[error("schema {key} is invalid: {reason}")]
    InvalidSchemaDefinition {
        /// Versioned schema identity whose definition is invalid.
        key: SchemaKey,
        /// Sanitized validator error without user payload data.
        reason: String,
    },

    /// A requested schema is absent from the world registry.
    #[error("schema {key} is not registered")]
    SchemaNotRegistered {
        /// Missing versioned schema identity.
        key: SchemaKey,
    },

    /// A schema exists but belongs to another semantic category.
    #[error("schema {key} has kind {actual}, expected {expected}")]
    SchemaKindMismatch {
        /// Versioned schema identity with the wrong category.
        key: SchemaKey,
        /// Required schema category.
        expected: SchemaKind,
        /// Registered schema category.
        actual: SchemaKind,
    },

    /// A feature payload violates its registered JSON Schema.
    #[error("payload for schema {key} is invalid at {instance_path}: {reason}")]
    PayloadValidationFailed {
        /// Schema used to validate the payload.
        key: SchemaKey,
        /// JSON Pointer locating the rejected value.
        instance_path: String,
        /// Masked validator message that does not expose the payload value.
        reason: String,
    },

    /// An exact control grant scope contains no command schemas.
    #[error("exact control grant scope must contain at least one command schema")]
    EmptyControlScope,

    /// A control grant identity already exists.
    #[error("control grant {0} already exists")]
    ControlGrantAlreadyExists(ControlGrantId),

    /// A control grant identity does not exist.
    #[error("control grant {0} does not exist")]
    ControlGrantNotFound(ControlGrantId),

    /// The authenticated principal cannot act as the requested actor for this command.
    #[error("principal {principal} is not authorized as actor {actor:?} for command {command}")]
    ActorUnauthorized {
        /// Authenticated outside principal.
        principal: PrincipalId,
        /// Requested world actor.
        actor: ActorRef,
        /// Command schema being authorized.
        command: SchemaKey,
    },

    /// A transaction contains no mutation, event, or durable effect.
    #[error("world transaction is empty")]
    EmptyTransaction,

    /// The same event identity appears more than once in one transaction.
    #[error("world transaction contains duplicate event id {0}")]
    DuplicateEventId(WorldEventId),

    /// The same effect-job identity appears more than once in one transaction.
    #[error("world transaction contains duplicate effect job id {0}")]
    DuplicateEffectJobId(EffectJobId),

    /// A retried command identity was reused for different immutable command input.
    #[error("command id {0} was already committed with different command content")]
    CommandIdConflict(CommandId),

    /// An event identity was already used by another committed command.
    #[error("world event id {0} already exists")]
    WorldEventAlreadyExists(WorldEventId),

    /// An effect-job identity was already used by another committed command.
    #[error("effect job id {0} already exists")]
    EffectJobAlreadyExists(EffectJobId),

    /// State changed after a System evaluated its transaction output.
    #[error("world changed since System evaluation: evaluated {evaluated}, actual {actual}")]
    WorldChangedSinceEvaluation {
        /// Position of the snapshot read by the System.
        evaluated: u64,
        /// Current authoritative position at commit.
        actual: u64,
    },

    /// Optimistic concurrency observed a newer or older world position.
    #[error("world position changed: expected {expected}, actual {actual}")]
    StaleWorldPosition {
        /// Position expected by the transaction.
        expected: u64,
        /// Current authoritative position.
        actual: u64,
    },

    /// The monotonic commit position cannot advance further.
    #[error("world commit position overflow")]
    CommitPositionOverflow,

    /// An entity identity already exists in current state.
    #[error("entity {0} already exists")]
    EntityAlreadyExists(EntityId),

    /// An entity identity does not exist in current state.
    #[error("entity {0} does not exist")]
    EntityNotFound(EntityId),

    /// A relation identity already exists in current state.
    #[error("relation {0} already exists")]
    RelationAlreadyExists(RelationId),

    /// A relation identity does not exist in current state.
    #[error("relation {0} does not exist")]
    RelationNotFound(RelationId),

    /// A relation endpoint references an entity absent from current state.
    #[error("relation {relation_id} references missing entity {entity_id}")]
    RelationEndpointNotFound {
        /// Relation being created.
        relation_id: RelationId,
        /// Missing source or destination entity.
        entity_id: EntityId,
    },

    /// A facet target references current state that does not exist.
    #[error("facet target does not exist")]
    FacetTargetNotFound,
}

/// Result type used by world-model operations.
pub type WorldResult<T> = Result<T, WorldError>;
