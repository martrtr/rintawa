//! Errors returned by world storage implementations.

use std::path::PathBuf;

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

    /// The storage schema version is not supported by this runtime.
    #[error("unsupported world storage schema version {found}; supported version is {supported}")]
    UnsupportedStorageSchema {
        /// Version stored in SQLite user_version.
        found: u32,
        /// Latest version supported by this runtime.
        supported: u32,
    },

    /// A persisted value or index cannot fit the public world-model type.
    #[error("world storage value for {0} is out of range")]
    IndexOutOfRange(&'static str),

    /// Persisted bytes violate a world-storage invariant.
    #[error("corrupt world storage: {0}")]
    CorruptData(String),

    /// A property value was stored under a different version than requested.
    #[error("property {name} is stored at version {found}, not the requested version {expected}")]
    StalePropertyVersion {
        /// Unversioned property name.
        name: String,
        /// Version requested by the caller.
        expected: u32,
        /// Version actually stored.
        found: u32,
    },

    /// A requested read page exceeds the bounded query policy.
    #[error("world query page size {requested} must be between 1 and {maximum}")]
    InvalidQueryPageSize {
        /// Requested maximum number of results.
        requested: usize,
        /// Largest permitted page.
        maximum: usize,
    },

    /// A property search page contains more JSON bytes than the read budget.
    #[error("world property query page has {bytes} JSON bytes; maximum is {maximum}")]
    QueryPageTooLarge {
        /// Total encoded JSON bytes encountered in the page.
        bytes: usize,
        /// Maximum encoded JSON bytes permitted in one page.
        maximum: usize,
    },

    /// A batch query supplied too many target item IDs.
    #[error("world query batch size {requested} exceeds {maximum}")]
    InvalidQueryBatchSize {
        /// Number of supplied item IDs.
        requested: usize,
        /// Largest permitted batch.
        maximum: usize,
    },

    /// A batched property read requested too many distinct exact keys.
    #[error("world property read requested {requested} keys; maximum is {maximum}")]
    InvalidPropertyReadKeyCount {
        /// Number of requested property keys.
        requested: usize,
        /// Largest supported number of property keys.
        maximum: usize,
    },

    /// The storage mutex became poisoned after an internal panic.
    #[error("world storage connection is unavailable")]
    ConnectionUnavailable,
}

/// Result type used by persistent world storage operations.
pub type StorageResult<T> = Result<T, StorageError>;
