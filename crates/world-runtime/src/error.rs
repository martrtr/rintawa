//! Errors returned by the authoritative world runtime.

use rintawa_sdk::world::SchemaKey;
use thiserror::Error;

/// Read-side errors visible to a world System.
#[derive(Debug, Error)]
pub enum WorldReadError {
    /// Persistent world state could not be read.
    #[error("world snapshot read failed")]
    Storage(#[source] rintawa_storage::StorageError),
}

impl From<rintawa_storage::StorageError> for WorldReadError {
    fn from(source: rintawa_storage::StorageError) -> Self {
        Self::Storage(source)
    }
}

/// Result type used by snapshot reads.
pub type WorldReadResult<T> = Result<T, WorldReadError>;

/// Error deliberately returned by one command System.
#[derive(Debug, Error)]
pub enum SystemError {
    /// Snapshot state required by the System could not be read.
    #[error(transparent)]
    Read(#[from] WorldReadError),

    /// The System rejected the command according to its domain rules.
    #[error("command rejected by System: {0}")]
    Rejected(String),

    /// The System could not produce a valid transaction result.
    #[error("System execution failed: {0}")]
    Failed(String),
}

/// Result type returned by command Systems.
pub type SystemResult<T> = Result<T, SystemError>;

/// Errors returned by the single-writer world runtime.
#[derive(Debug, Error)]
pub enum WorldRuntimeError {
    /// Generic world-model validation rejected runtime configuration.
    #[error(transparent)]
    World(#[from] rintawa_world::WorldError),

    /// Persistent world storage rejected an operation.
    #[error(transparent)]
    Storage(#[from] rintawa_storage::StorageError),

    /// Runtime queue policy is invalid.
    #[error("command queue capacity {capacity} is outside the supported range 1..={maximum}")]
    InvalidQueueCapacity {
        /// Rejected queue capacity.
        capacity: usize,
        /// Maximum supported queue capacity.
        maximum: usize,
    },

    /// Two Systems attempted to own the same command schema.
    #[error("a World System is already registered for command schema {0}")]
    DuplicateSystem(SchemaKey),

    /// No active System handles the submitted command schema.
    #[error("no World System is registered for command schema {0}")]
    SystemUnavailable(SchemaKey),

    /// The bounded command queue has no free capacity.
    #[error("world command queue is full")]
    QueueFull,

    /// The worker has already stopped accepting commands.
    #[error("world runtime is stopped")]
    RuntimeStopped,

    /// The worker ended before returning a command result.
    #[error("world runtime worker stopped before returning a result")]
    WorkerStopped,

    /// The runtime worker thread terminated unexpectedly.
    #[error("world runtime worker panicked")]
    WorkerPanicked,

    /// A command System returned an explicit failure.
    #[error("World System for {schema} failed")]
    SystemFailed {
        /// Command schema owned by the failing System.
        schema: SchemaKey,
        /// System-provided error.
        #[source]
        source: SystemError,
    },

    /// A trusted native System panicked while evaluating a command.
    #[error("World System for {0} panicked")]
    SystemPanicked(SchemaKey),

    /// Ordinary extension Systems may not mint or revoke Core control grants.
    #[error("World System for {0} attempted a privileged authority mutation")]
    AuthorityMutationDenied(SchemaKey),

    /// The runtime worker thread could not be spawned.
    #[error("failed to spawn world runtime worker")]
    WorkerSpawn(#[source] std::io::Error),
}

/// Result type used by the world runtime.
pub type WorldRuntimeResult<T> = Result<T, WorldRuntimeError>;
