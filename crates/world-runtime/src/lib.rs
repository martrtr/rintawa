//! Single-writer authoritative world command runtime.
//!
//! This crate serializes command evaluation and commit for one active world.
//! It intentionally knows nothing about Chat, Narrator, AI providers, games, or
//! renderer semantics.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod effect;
mod error;
mod runtime;
mod service;
mod snapshot;
mod system;

pub use effect::{
    MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES, ServiceWorldEffectHandler,
    WORLD_EFFECT_SERVICE_PROTOCOL_VERSION, WorldEffectServiceClient, WorldEffectServiceError,
    WorldEffectServiceRequest, WorldEffectServiceResponse, WorldEffectServiceResult,
    effect_completion_command_id, world_effect_service_contract_key,
};
pub use error::{
    SystemError, SystemResult, WorldReadError, WorldReadResult, WorldRuntimeError,
    WorldRuntimeResult,
};
pub use runtime::{
    DEFAULT_COMMAND_QUEUE_CAPACITY, MAX_COMMAND_QUEUE_CAPACITY, WorldCommandOutcome,
    WorldCommandTicket, WorldRuntime, WorldRuntimeBuilder, WorldRuntimePolicy,
    WorldSystemPrivileges,
};
pub use service::{
    MAX_WORLD_SYSTEM_SERVICE_READS, MAX_WORLD_SYSTEM_SERVICE_ROUNDS, ServiceWorldSystem,
    WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION, WorldSystemReadRequest, WorldSystemReadResult,
    WorldSystemServiceClient, WorldSystemServiceRequest, WorldSystemServiceResponse,
    world_system_service_contract_key,
};
pub use snapshot::WorldSnapshot;
pub use system::WorldSystem;
