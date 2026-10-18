//! Errors returned by world storage implementations.

use std::path::PathBuf;

use rintawa_sdk::world::EffectJobId;
use thiserror::Error;

/// Errors returned by persistent world storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    /// A filesystem operation failed.
    #[error("world storage filesystem operation failed")]
    Io(#[from] std::io::Error),

    /// The SQLite implementation failed an internal storage operation.
    #[error("world storage database operation failed")]
    Sqlite(#[from] rusqlite::Error),

    /// Persisted or incoming JSON could not be encoded or decoded.
    #[error("world storage JSON data is invalid")]
    Json(#[from] serde_json::Error),

    /// World-model validation rejected persisted or incoming state.
    #[error(transparent)]
    World(#[from] rintawa_world::WorldError),

    /// A new world cannot be created over an existing filesystem entry.
    #[error("world storage already exists at {0}")]
    AlreadyExists(PathBuf),

    /// A requested world database does not exist.
    #[error("world storage does not exist at {0}")]
    NotFound(PathBuf),

    /// The world path is a symlink or another unsupported filesystem entry.
    #[error("invalid world storage entry at {path}: {reason}")]
    InvalidEntry {
        /// Rejected filesystem path.
        path: PathBuf,
        /// Storage invariant that was violated.
        reason: &'static str,
    },

    /// The database was not initialized as a Rintawa world.
    #[error("database is not an initialized Rintawa world")]
    Uninitialized,

    /// The storage migration schema is newer than this runtime supports.
    #[error("unsupported world storage schema version {found}; supported version is {supported}")]
    UnsupportedStorageSchema {
        /// Version stored in SQLite user_version.
        found: u32,
        /// Latest version supported by this runtime.
        supported: u32,
    },

    /// The world format itself is newer or otherwise incompatible.
    #[error("unsupported world format version {found}; supported version is {supported}")]
    UnsupportedWorldFormat {
        /// World format stored in metadata.
        found: u32,
        /// World format supported by the world crate.
        supported: u32,
    },

    /// A persisted Core mutation encoding is newer or otherwise unsupported.
    #[error("unsupported world mutation format version {found}; supported version is {supported}")]
    UnsupportedMutationFormat {
        /// Format version stored with the mutation entry.
        found: u32,
        /// Mutation format version supported by this runtime.
        supported: u32,
    },

    /// Persisted bytes violate a world-storage invariant.
    #[error("corrupt world storage: {0}")]
    CorruptData(String),

    /// A persisted sequence/index cannot fit the public world-model type.
    #[error("world storage index for {0} is out of range")]
    IndexOutOfRange(&'static str),

    /// A requested durable effect job does not exist.
    #[error("effect job {0} does not exist")]
    EffectJobNotFound(EffectJobId),

    /// An effect worker requested a lease that does not extend beyond the claim time.
    #[error("effect job lease must expire after the claim time")]
    InvalidEffectLease,

    /// A previous effect claim lost authority because the job was reclaimed or cancelled.
    #[error("effect job claim for {0} is no longer authoritative")]
    EffectJobClaimLost(EffectJobId),

    /// The effect job attempt counter cannot be incremented further.
    #[error("effect job {0} attempt counter overflowed")]
    EffectAttemptOverflow(EffectJobId),

    /// A completed effect job cannot transition back to a worker-controlled state.
    #[error("effect job {0} is already terminal")]
    EffectJobTerminal(EffectJobId),

    /// Worker diagnostic text exceeds the bounded durable error field.
    #[error("effect job error is {actual_bytes} bytes, exceeding the {maximum_bytes}-byte limit")]
    EffectErrorTooLarge {
        /// Supplied UTF-8 byte length.
        actual_bytes: usize,
        /// Maximum persisted error size.
        maximum_bytes: usize,
    },

    /// The storage mutex became poisoned after an internal panic.
    #[error("world storage connection is unavailable")]
    ConnectionUnavailable,
}

/// Result type used by persistent world storage operations.
pub type StorageResult<T> = Result<T, StorageError>;
