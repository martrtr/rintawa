//! Context traits supplied by the Extension Engine to a component.
//!
//! Contexts expose SDK interfaces only. Engine-owned runtime, storage, and UI
//! implementation details must never escape through this public boundary.

use crate::{
    api::LoggerApi,
    contributions::ContributionDescriptor,
    errors::ExtensionResult,
    types::{ComponentId, ExtensionId},
};

/// The context available to a component during execution.
///
/// This trait provides access to the component's identity and the APIs
/// it can use during execution.
pub trait ComponentContext {
    /// Returns the ID of the extension that owns this component.
    fn extension_id(&self) -> &ExtensionId;

    /// Returns the ID of this component.
    fn component_id(&self) -> &ComponentId;

    /// Returns the logger API for this component.
    fn logger(&self) -> &dyn LoggerApi;
}

/// The context available during component registration.
///
/// This trait extends [`ComponentContext`] with registration capabilities,
/// allowing components to register their contributions.
pub trait RegistrationContext: ComponentContext {
    /// Registers a contribution with the system.
    ///
    /// # Errors
    ///
    /// Returns [`crate::errors::ExtensionError::DuplicateContribution`] if another active
    /// registration already owns the contribution identifier.
    fn register(&mut self, contribution: ContributionDescriptor) -> ExtensionResult<()>;
}
