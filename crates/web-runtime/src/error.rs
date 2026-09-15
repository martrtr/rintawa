//! Web runtime protocol errors.

use thiserror::Error;

/// Errors while parsing Web bundle metadata or bridge messages.
#[derive(Debug, Error)]
pub enum WebRuntimeError {
    /// The Web bundle descriptor is larger than the host metadata limit.
    #[error("web bundle descriptor is {actual} bytes, exceeding the {maximum}-byte limit")]
    DescriptorTooLarge {
        /// Observed descriptor size.
        actual: usize,
        /// Maximum accepted descriptor size.
        maximum: usize,
    },

    /// The Web bundle descriptor is not valid UTF-8.
    #[error("web bundle descriptor must be valid UTF-8")]
    DescriptorEncoding,

    /// The Web bundle descriptor is invalid TOML.
    #[error("failed to parse web bundle descriptor: {0}")]
    DescriptorParse(#[from] toml::de::Error),

    /// The descriptor schema version is not supported.
    #[error("unsupported web bundle descriptor schema {0}")]
    UnsupportedDescriptorSchema(u32),

    /// The Web bridge protocol version is not supported.
    #[error("unsupported web UI bridge protocol major {0}")]
    UnsupportedBridgeProtocol(u32),

    /// The portable UI protocol version is not supported.
    #[error("unsupported portable UI protocol major {0}")]
    UnsupportedPortableUiProtocol(u32),

    /// A capability appears more than once in the layer descriptor.
    #[error("duplicate UI capability `{0}`")]
    DuplicateCapability(String),

    /// Renderer capabilities do not match the packaged UI Layer descriptor.
    #[error("renderer capabilities do not match the packaged UI Layer descriptor")]
    CapabilityMismatch,

    /// A Web action contains an invalid exact surface revision.
    #[error("invalid surface revision `{0}`")]
    InvalidSurfaceRevision(String),
}

/// Result returned by Web runtime protocol operations.
pub type WebRuntimeResult<T> = Result<T, WebRuntimeError>;
