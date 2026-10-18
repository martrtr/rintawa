//! Generic request/response service transport.

use thiserror::Error;

/// Opaque host-issued selection for one provider of a `Multiple` service contract.
///
/// The numeric value is transport identity only. Constructing or guessing a value
/// grants no authority: the Extension Engine binds every live handle to the exact
/// consumer principal, contract, scope, provider, and current topology/policy state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceProviderHandle(u64);

impl ServiceProviderHandle {
    /// Reconstructs an opaque handle received through a host transport.
    ///
    /// This does not validate or grant access. The host must validate the handle
    /// again when it is used.
    #[doc(hidden)]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the transport representation of this opaque handle.
    #[doc(hidden)]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

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
