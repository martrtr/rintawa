//! Error types for Taverna extensions.
//!
//! This module provides the error types used throughout the SDK
//! for error handling and propagation.

use thiserror::Error;

/// The error type for Taverna extension operations.
///
/// This enum represents the various ways an extension operation can fail,
/// providing descriptive error messages for each failure case.
#[derive(Debug, Error)]
pub enum ExtensionError {
    /// An error indicating that a contribution with the given identifier already exists.
    #[error("duplicate contribution `{0}`")]
    DuplicateContribution(String),

    /// A generic error with a message.
    #[error("{0}")]
    Message(String),
}

/// A type alias for the result of extension operations.
///
/// This type alias provides a shorthand for returning `Result<T, ExtensionError>`
/// from extension functions and methods.
pub type ExtensionResult<T = ()> = Result<T, ExtensionError>;
