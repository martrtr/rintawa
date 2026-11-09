//! Authoritative, feature-neutral world model primitives.
//!
//! This crate owns the generic world state model: items (objects and relations),
//! versioned properties, one world manifest, and one in-memory atomic change set.
//! There is no durable command ledger, commit position, event log, consumer
//! cursor, effect outbox or control grant.
//!
//! Feature packages define concrete property definitions and command schemas
//! through the SDK; nothing here assigns player, NPC or ownership semantics.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod command;
mod error;
mod runtime_schema;
mod schema;
mod session;
mod state;
mod transaction;

pub use command::WorldCommand;
pub use error::{WorldError, WorldResult};
pub use runtime_schema::{
    RuntimeSchemaDefinition, RuntimeSchemaRegistration, SchemaKind, SchemaRegistry,
};
pub use schema::{DefinitionRegistration, PropertyDefinition, PropertyRegistry};
pub use session::{WorldManifest, WorldSessionState};
pub use state::{
    ITEM_TYPE_OBJECT, ITEM_TYPE_RELATION, ItemKind, ItemRecord, PropertyRecord, RelationDirection,
};
pub use transaction::{StateChange, WorldTransaction};
