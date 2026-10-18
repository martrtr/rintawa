//! Authoritative, feature-neutral world model primitives.
//!
//! This crate owns generic world state semantics. Feature packages define
//! concrete entity, relation, command, event, and facet schemas through the SDK.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod authority;
mod command;
mod error;
mod schema;
mod session;
mod state;
mod transaction;

pub use authority::{ActorRef, ControlGrant, ControlScope};
pub use command::{CausationRef, WorldCommand};
pub use error::{WorldError, WorldResult};
pub use schema::{SchemaDefinition, SchemaKind, SchemaRegistration, SchemaRegistry};
pub use session::{WORLD_FORMAT_VERSION, WorldSessionState};
pub use state::{EntityRecord, FacetRecord, FacetTarget, RelationRecord, WorldMutation};
pub use transaction::{
    CommandProvenance, CommitDisposition, CommitReceipt, EffectJobDraft, StoredEffectJob,
    StoredWorldEvent, StoredWorldMutation, WORLD_MUTATION_FORMAT_VERSION, WorldEventDraft,
    WorldTransaction,
};
