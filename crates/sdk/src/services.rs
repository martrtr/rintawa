//! Generic request/response service transport.

use thiserror::Error;

/// Transport-level failure while routing a service request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceCallError {
    /// The requested contract has no currently usable binding.
    #[error("service unavailable")]
    Unavailable,
    /// The caller did not declare itself as a consumer of this contract.
    #[error("component is not a consumer of this service contract")]
    NotConsumer,
    /// The resolved contract is not declared as a service protocol.
    #[error("contract is not a service contract")]
    NotServiceContract,
    /// The contract resolves to a provider composition unsupported by unary calls.
    #[error("service contract does not resolve to exactly one provider")]
    UnsupportedResolution,
    /// The selected provider is already present in the current nested call chain.
    #[error("cyclic service call detected")]
    CyclicCall,
    /// The selected provider is currently executing another independent callback.
    #[error("service provider is busy")]
    ProviderBusy,
    /// The selected provider failed while handling the request.
    #[error("service provider failed")]
    ProviderFailed,
    /// The request exceeds the host transport limit.
    #[error("service request is too large")]
    RequestTooLarge,
    /// The provider response exceeds the host transport limit.
    #[error("service response is too large")]
    ResponseTooLarge,
}

/// Result returned by the generic service transport.
pub type ServiceCallResult<T> = Result<T, ServiceCallError>;
