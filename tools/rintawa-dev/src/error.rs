use std::io;

use rintawa_artifacts::RtwError;
use thiserror::Error;

use crate::DEV_CONFIG_FILE;

/// Errors returned by local project preparation.
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
    /// The build command has no executable.
    #[error("build command must contain an executable")]
    EmptyBuildCommand,
    /// The build command failed.
    #[error("build command failed with status {0}")]
    BuildFailed(String),
    /// RTW packing or store import failed.
    #[error("RTW artifact error: {0}")]
    Artifact(#[from] RtwError),
}

/// Result type used by `rintawa-dev`.
pub type DevResult<T> = Result<T, DevError>;
