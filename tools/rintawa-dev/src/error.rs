use std::io;

use rintawa_artifacts::RtwError;
use rintawa_extension_engine::EngineError;
use thiserror::Error;

use crate::DEV_CONFIG_FILE;

/// Errors returned by local extension development tools.
#[derive(Debug, Error)]
pub enum DevError {
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The developer configuration is invalid TOML.
    #[error("invalid {DEV_CONFIG_FILE}: {0}")]
    ConfigParse(#[from] toml::de::Error),
    /// The configuration schema is unsupported.
    #[error("unsupported developer config schema {0}")]
    UnsupportedConfigSchema(u32),
    /// The configured artifact root is invalid.
    #[error("invalid artifact root `{0}`")]
    InvalidArtifactRoot(String),
    /// A watch-ignore pattern is invalid.
    #[error("invalid watch-ignore pattern: {0}")]
    InvalidWatchIgnore(String),
    /// The build command has no executable.
    #[error("build command must contain an executable")]
    EmptyBuildCommand,
    /// The build command failed.
    #[error("build command failed with status {0}")]
    BuildFailed(String),
    /// RTW packing or store import failed.
    #[error("RTW artifact error: {0}")]
    Artifact(#[from] RtwError),
    /// Extension loading or lifecycle failed.
    #[error("extension runtime error: {0}")]
    Engine(#[from] EngineError),
    /// The development Web host failed.
    #[error("Web host error: {0}")]
    WebHost(String),
    /// Shared Web host state is unavailable.
    #[error("Web host state is unavailable")]
    WebHostUnavailable,
    /// Runtime setup failed and cleanup also failed.
    #[error("runtime setup failed: {setup}; cleanup failed: {cleanup}")]
    RuntimeSetupCleanupFailed {
        /// Runtime setup failure.
        setup: Box<DevError>,
        /// Cleanup failure after setup failed.
        cleanup: Box<DevError>,
    },
    /// Reload state is unavailable.
    #[error("development session is unavailable")]
    SessionUnavailable,
    /// A new snapshot failed to start and the previous snapshot was restored.
    #[error("reload failed; previous snapshot restored: {source}")]
    ReloadFailed {
        /// New snapshot failure.
        #[source]
        source: Box<DevError>,
    },
    /// Reload and rollback both failed.
    #[error("reload failed: {reload}; rollback failed: {rollback}")]
    ReloadAndRollbackFailed {
        /// New snapshot failure.
        reload: Box<DevError>,
        /// Previous snapshot rollback failure.
        rollback: Box<DevError>,
    },
    /// Stop and unregister both failed during cleanup.
    #[error("extension shutdown failed: stop: {stop}; unregister: {unregister}")]
    ShutdownFailed {
        /// Stop failure.
        stop: Box<EngineError>,
        /// Unregister failure.
        unregister: Box<EngineError>,
    },
}

/// Result type used by `rintawa-dev`.
pub type DevResult<T> = Result<T, DevError>;
