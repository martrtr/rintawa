//! Error types for Rintawa extensions.

use thiserror::Error;

use crate::secrets::SecretAccessError;

/// The error type for extension operations.
#[derive(Debug, Error)]
pub enum ExtensionError {
    /// A contribution identifier is already active in the current scope.
    #[error("duplicate contribution `{0}`")]
    DuplicateContribution(String),

    /// A runtime adapter discarded the component instance after a fatal callback failure.
    #[error("component runtime was invalidated during `{operation}`: {reason}")]
    ComponentRuntimeInvalidated {
        /// Callback after which the component runtime can no longer execute.
        operation: &'static str,
        /// Original failure that caused the runtime to be discarded.
        reason: String,
    },

    /// Stop cannot release state still used by a callback; retry after it finishes.
    #[error("component cleanup is busy; ownership retained for retry")]
    CleanupBusy,

    /// An execution budget enforced by the host was exhausted.
    #[error("execution budget for `{resource}` was exhausted during `{operation}`")]
    ExecutionBudgetExceeded {
        /// The bounded resource, for example `fuel`.
        resource: &'static str,
        /// The lifecycle operation receiving the limit.
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
    #[error("duplicate contract definition `{0}")]
    DuplicateContractDefinition(String),

    /// The component registered the same contract provider more than once.
    #[error("duplicate contract provider `{0}")]
    DuplicateContractProvider(String),

    /// The component registered the same contract consumer more than once.
    #[error("duplicate contract consumer `{0}")]
    DuplicateContractConsumer(String),

    /// The host cannot register contract metadata in the current context.
    #[error("contract registration is unavailable in this component context")]
    ContractRegistrationUnavailable,

    /// The extension declared the same world schema key more than once.
    #[error("duplicate world schema declaration `{0}")]
    DuplicateWorldSchema(String),

    /// The host cannot register world-schema metadata in the current context.
    #[error("world schema registration is unavailable in this component context")]
    WorldSchemaRegistrationUnavailable,

    /// The component does not implement a handler for the requested service contract.
    #[error("service handler is unavailable for contract `{0}`")]
    ServiceHandlerUnavailable(String),

    /// A generic error with a message.
    #[error("{0}")]
    Message(String),
}

/// Result type used by extension code.
pub type ExtensionResult<T = ()> = Result<T, ExtensionError>;
