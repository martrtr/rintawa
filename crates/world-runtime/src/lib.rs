//! Single-writer authoritative world command runtime.
//!
//! This crate serializes command evaluation and commit for one active world.
//! It intentionally knows nothing about Chat, Narrator, AI providers, games, or
//! renderer semantics.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod error;
mod runtime;
mod snapshot;
mod system;

pub use error::{
    SystemError, SystemResult, WorldReadError, WorldReadResult, WorldRuntimeError,
    WorldRuntimeResult,
};
pub use runtime::{
    DEFAULT_COMMAND_QUEUE_CAPACITY, MAX_COMMAND_QUEUE_CAPACITY, WorldCommandOutcome,
    WorldCommandTicket, WorldRuntime, WorldRuntimeBuilder, WorldRuntimePolicy,
    WorldSystemPrivileges,
};
pub use snapshot::WorldSnapshot;
pub use system::WorldSystem;
