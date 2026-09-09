//! Error types for the Rintawa Extension Engine.

use rintawa_sdk::errors::ExtensionError;
use rintawa_sdk::secrets::SecretAccessError;
use std::io;
use thiserror::Error;

/// Errors that can occur during Extension Engine operations.
#[derive(Debug, Error)]
pub enum EngineError {
    /// An I/O error occurred during file operations.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The specified directory for extensions is invalid.
    #[error("invalid extensions directory: `{0}`")]
    InvalidDirectory(String),

    /// An error occurred while parsing an extension manifest.
    #[error("failed to parse manifest: {0}")]
    ManifestParse(#[from] toml::de::Error),

    /// An error occurred while serializing configuration state.
    #[error("failed to serialize state: {0}")]
    StateSerialize(#[from] toml::ser::Error),

    /// A host secret configuration or policy operation failed.
    #[error("secret operation failed: {0}")]
    SecretAccess(#[from] SecretAccessError),

    /// A requested secret grant does not correspond to the extension manifest.
    #[error(
        "secret grant `{pattern}` was not requested by component `{component_id}` in extension `{extension_id}`"
    )]
    SecretPermissionNotRequested {
        /// ID of the extension requesting secret access.
        extension_id: String,
        /// ID of the component requesting secret access.
        component_id: String,
        /// Requested grant pattern that policy attempted to approve.
        pattern: String,
    },

    /// A host attempted to grant access to an undeclared component.
    #[error("component `{component_id}` was not found in extension `{extension_id}`")]
    SecretComponentNotFound {
        /// ID of the extension containing the component.
        extension_id: String,
        /// ID of the missing component.
        component_id: String,
    },

    /// An SDK-level lifecycle or registration error.
    #[error("extension SDK error: {0}")]
    Sdk(#[from] ExtensionError),

    /// An error occurred during Wasmtime runtime operations.
    #[error("WASM runtime error: {0}")]
    WasmRuntime(#[from] wasmtime::Error),

    /// The specified extension was not found in the engine.
    #[error("extension `{0}` not found")]
    ExtensionNotFound(String),

    /// The specified extension is already registered in the engine.
    #[error("extension `{0}` is already registered")]
    ExtensionAlreadyExists(String),

    /// A component failed during a lifecycle transition.
    #[error("component `{component_id}` in extension `{extension_id}` failed: {reason}")]
    LifecycleFailed {
        /// ID of the extension owning the component.
        extension_id: String,
        /// ID of the failed component.
        component_id: String,
        /// Failure message detailing the cause.
        reason: String,
    },

    /// The extension is in an invalid lifecycle state for the operation.
    #[error("extension `{0}` is in an invalid state for this operation")]
    InvalidState(String),
}

/// A specialized [`Result`] type for Extension Engine operations.
pub type EngineResult<T = ()> = Result<T, EngineError>;
