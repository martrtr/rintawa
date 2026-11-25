//! Errors returned by the authoritative world model.

use rintawa_sdk::world::{ItemId, SchemaKey};
use thiserror::Error;

/// Errors returned by feature-neutral world operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// The same versioned property definition was registered with other semantics.
    #[error(
        "property definition {key} conflicts with the registered definition owned by {existing_owner}; incoming owner is {incoming_owner}"
    )]
    DefinitionConflict {
        /// Versioned property name whose definition conflicts.
        key: SchemaKey,
        /// Owner of the already registered definition.
        existing_owner: String,
        /// Owner attempting to register incompatible semantics.
        incoming_owner: String,
    },

    /// A property definition cannot be compiled as local JSON Schema.
    #[error("property definition {key} is invalid: {reason}")]
    InvalidDefinition {
        /// Versioned property name whose definition is invalid.
        key: SchemaKey,
        /// Sanitized validator error without user payload data.
        reason: String,
    },

    /// A System attempted to mutate a property outside its active owner bindings.
    #[error("System cannot {operation} property {key}: no active owner binding")]
    PropertyAccessDenied {
        /// Exact property key refused by the command runtime.
        key: SchemaKey,
        /// Operation that was rejected before COMMIT.
        operation: &'static str,
    },

    /// A requested versioned property definition is absent from the world.
    #[error("property definition {key} is not registered")]
    DefinitionNotRegistered {
        /// Missing versioned property name.
        key: SchemaKey,
    },

    /// A runtime API schema exists but belongs to another semantic category.
    #[error("schema {key} has kind {actual}, expected {expected}")]
    DefinitionKindMismatch {
        /// Versioned schema identity with the wrong category.
        key: SchemaKey,
        /// Required schema category.
        expected: crate::SchemaKind,
        /// Registered schema category.
        actual: crate::SchemaKind,
    },

    /// A runtime API payload violates its registered JSON Schema.
    #[error("payload for schema {key} is invalid at {instance_path}: {reason}")]
    PayloadValidationFailed {
        /// Schema used to validate the payload.
        key: SchemaKey,
        /// JSON Pointer locating the rejected value.
        instance_path: String,
        /// Masked validator message that does not expose the payload value.
        reason: String,
    },

    /// A property value violates its registered JSON Schema.
    #[error("property {key} is invalid at {instance_path}: {reason}")]
    PropertyValidationFailed {
        /// Property definition used to validate the value.
        key: SchemaKey,
        /// JSON Pointer locating the rejected value.
        instance_path: String,
        /// Masked validator message that does not expose the property value.
        reason: String,
    },

    /// The stored `items.type` value is neither an object nor a relation.
    #[error("item type {0} is not a valid item kind")]
    InvalidItemKind(i64),

    /// A transaction contains no change at all.
    #[error("world transaction is empty")]
    EmptyTransaction,

    /// The same item identity was created more than once in one transaction.
    #[error("world transaction creates item {0} more than once")]
    DuplicateItemCreation(ItemId),

    /// A relation endpoint is missing from current state.
    #[error("relation {relation} references missing item {endpoint}")]
    RelationEndpointMissing {
        /// Relation being created.
        relation: ItemId,
        /// Missing subject or object endpoint.
        endpoint: ItemId,
    },

    /// A relation endpoint names another relation instead of an object-item.
    #[error("relation {relation} references {endpoint}, which is not an object")]
    RelationEndpointNotObject {
        /// Relation being created.
        relation: ItemId,
        /// Endpoint that is a relation.
        endpoint: ItemId,
    },

    /// A relation-item was found with a missing endpoint.
    #[error("relation {0} has no endpoint")]
    RelationEndpointAbsent(ItemId),

    /// An item identity does not exist in current state.
    #[error("item {0} does not exist")]
    ItemNotFound(ItemId),

    /// An item identity already exists in current state.
    #[error("item {0} already exists")]
    ItemAlreadyExists(ItemId),

    /// A property name is already present in a property vocabulary.
    #[error("property name {0} is empty")]
    EmptyPropertyName(String),
}

/// Result type used by world-model operations.
pub type WorldResult<T> = Result<T, WorldError>;
