//! Errors returned by the authoritative world runtime.

use rintawa_sdk::world::SchemaKey;
use thiserror::Error;

/// Read-side errors visible to a world System.
#[derive(Debug, Error)]
pub enum WorldReadError {
    /// The pinned snapshot became unavailable after an internal failure.
    #[error("world snapshot is unavailable")]
    SnapshotUnavailable,
    /// The calling System does not own this active property definition.
    #[error("System cannot read property {0}: no active owner binding")]
    PropertyAccessDenied(SchemaKey),

    /// Persistent world state could not be read.
    #[error("world snapshot read failed")]
    Storage(#[source] rintawa_storage::StorageError),
}

impl From<rintawa_storage::StorageError> for WorldReadError {
    fn from(source: rintawa_storage::StorageError) -> Self {
        Self::Storage(source)
    }
}

impl WorldReadError {
    /// Whether arguments or the requested exact property version are invalid.
    pub fn invalid_request(&self) -> bool {
        matches!(
            self,
            Self::Storage(
                rintawa_storage::StorageError::StalePropertyVersion { .. }
                    | rintawa_storage::StorageError::InvalidQueryPageSize { .. }
                    | rintawa_storage::StorageError::InvalidQueryBatchSize { .. }
                    | rintawa_storage::StorageError::InvalidPropertyReadKeyCount { .. }
            )
        )
    }
    /// Whether the native read result exceeds its bounded byte budget.
    pub fn message_too_large(&self) -> bool {
        matches!(
            self,
            Self::Storage(rintawa_storage::StorageError::QueryPageTooLarge { .. })
        )
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

    /// Serialized queued and evaluating commands exhaust the World byte budget.
    #[error("world pending-command byte budget exceeded")]
    PendingCommandBytesExceeded,

    /// Graceful shutdown is incomplete; worker/writer ownership is retained.
    #[error("world shutdown is busy; retry after admitted work finishes")]
    ShutdownBusy,

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

    /// A System produced a change set that the world model or vocabulary rejected.
    #[error("World System for {schema} produced an invalid transaction")]
    InvalidTransaction {
        /// Command schema owned by the System.
        schema: SchemaKey,
        /// World-model validation error.
        #[source]
        source: rintawa_world::WorldError,
    },

    /// The runtime session state could not be locked after an internal panic.
    #[error("world runtime session state is unavailable")]
    RuntimePoisoned,

    /// The runtime worker thread could not be spawned.
    #[error("failed to spawn world runtime worker")]
    WorkerSpawn(#[source] std::io::Error),
}

/// Result type used by the world runtime.
pub type WorldRuntimeResult<T> = Result<T, WorldRuntimeError>;

impl WorldRuntimeError {
    /// Whether a command completion error fails to establish that no COMMIT occurred.
    /// Callers must report unavailable and must not automatically retry the command.
    pub fn command_outcome_unknown(&self) -> bool {
        match self {
            Self::WorkerStopped | Self::WorkerPanicked => true,
            Self::Storage(rintawa_storage::StorageError::Io(_)) => true,
            Self::Storage(rintawa_storage::StorageError::Sqlite(error)) => {
                !matches!(error, rusqlite::Error::SqliteFailure(error, _)
                    if error.code == rusqlite::ErrorCode::ConstraintViolation)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_should_distinguish_unknown_commit_from_constraint_rejection() {
        let storage_error = |code| {
            WorldRuntimeError::Storage(rintawa_storage::StorageError::Sqlite(
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None),
            ))
        };
        assert!(storage_error(rusqlite::ffi::SQLITE_IOERR).command_outcome_unknown());
        assert!(!storage_error(rusqlite::ffi::SQLITE_CONSTRAINT).command_outcome_unknown());
        assert!(WorldRuntimeError::WorkerStopped.command_outcome_unknown());
        assert!(
            !WorldRuntimeError::SystemPanicked("fixture.test@1".parse().unwrap())
                .command_outcome_unknown()
        );
    }
}
