//! Error types for Rintawa extensions.
//!
//! This module provides the error types used throughout the SDK
//! for error handling and propagation.

use crate::secrets::SecretAccessError;

use thiserror::Error;

/// The error type for Rintawa extension operations.
///
/// This enum represents the various ways an extension operation can fail,
/// providing descriptive error messages for each failure case.
#[derive(Debug, Error)]
pub enum ExtensionError {
    /// An error indicating that a contribution with the given identifier already exists.
    #[error("duplicate contribution `{0}`")]
    DuplicateContribution(String),

    /// An error indicating that a runtime effect is invalid for this host.
    #[error("invalid runtime effect: {0}")]
    InvalidRuntimeEffect(String),

    /// An error indicating that an effect does not belong to the caller.
    #[error("runtime effect `{0}` is not owned by this component")]
    RuntimeEffectNotOwned(String),

    /// An error indicating that the current context cannot manage runtime effects.
    #[error("runtime effects are unavailable in this component context")]
    RuntimeEffectsUnavailable,

    /// An error indicating that a runtime-effect callback and its rollback both failed.
    #[error("runtime effect operation failed: {operation}; rollback failed: {rollback}")]
    RuntimeEffectRollbackFailed {
        /// The error from the requested runtime-effect operation.
        operation: String,
        /// The error that prevented a complete rollback.
        rollback: String,
    },

    /// An error indicating that an operation failed and owner-effect cleanup also failed.
    #[error("runtime effect operation failed: {operation}; cleanup failed: {cleanup}")]
    RuntimeEffectCleanupFailed {
        /// The error from the operation that required cleanup.
        operation: String,
        /// The error that prevented owner-effect cleanup.
        cleanup: String,
    },

    /// An execution budget enforced by the host was exhausted.
    #[error("execution budget for `{resource}` was exhausted during `{operation}`")]
    ExecutionBudgetExceeded {
        /// The bounded resource, for example `fuel`.
        resource: &'static str,
        /// The lifecycle operation that exceeded its budget.
        operation: &'static str,
    },

    /// A host-to-component message exceeds the host's configured size limit.
    #[error(
        "host message for `{operation}` is {actual_bytes} bytes, exceeding the {maximum_bytes}-byte limit"
    )]
    HostMessageTooLarge {
        /// The lifecycle operation receiving the message.
        operation: &'static str,
        /// Size of the supplied message in bytes.
        actual_bytes: usize,
        /// Maximum message size accepted by the host in bytes.
        maximum_bytes: usize,
    },

    /// A secret read failed because the active component lacks access or the host cannot serve it.
    #[error("secret access failed: {0}")]
    SecretAccess(#[from] SecretAccessError),

    /// The component defined the same contract more than once.
    #[error("duplicate contract definition `{0}`")]
    DuplicateContractDefinition(String),

    /// The component registered the same contract provider more than once.
    #[error("duplicate contract provider `{0}`")]
    DuplicateContractProvider(String),

    /// The component registered the same contract consumer more than once.
    #[error("duplicate contract consumer `{0}`")]
    DuplicateContractConsumer(String),

    /// The host cannot register contract metadata in the current context.
    #[error("contract registration is unavailable in this component context")]
    ContractRegistrationUnavailable,

    /// A generic error with a message.
    #[error("{0}")]
    Message(String),
}

/// A type alias for the result of extension operations.
///
/// This type alias provides a shorthand for returning `Result<T, ExtensionError>`
/// from extension functions and methods.
pub type ExtensionResult<T = ()> = Result<T, ExtensionError>;
