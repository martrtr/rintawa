//! Component lifecycle contract implemented by extension runtime code.
//!
//! The Extension Engine owns component creation, lifecycle ordering, rollback,
//! and removal of registered contributions. Implementations only describe their
//! identity and registration/start/stop behavior.

use crate::{
    context::{ComponentContext, RegistrationContext},
    contracts::ContractKey,
    errors::{ExtensionError, ExtensionResult},
    types::ComponentId,
};

/// The trait that all components must implement.
///
/// This trait defines the lifecycle methods that all components must provide.
/// Components are the building blocks of Rintawa extensions.
pub trait Component: Send {
    /// Returns the unique identifier of this component.
    fn id(&self) -> &ComponentId;

    /// Registers this component with the system.
    ///
    /// This method is called once during component initialization.
    /// Override this to perform custom registration logic.
    ///
    /// # Errors
    ///
    /// Returns an error when the component cannot register one of its
    /// contributions.
    fn register(&mut self, _ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        Ok(())
    }

    /// Starts this component.
    ///
    /// This method is called after all components have been registered.
    /// Override this to perform custom startup logic.
    ///
    /// # Errors
    ///
    /// Returns an error when the component cannot start.
    fn start(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        Ok(())
    }

    /// Stops this component.
    ///
    /// This method is called during component shutdown.
    /// Override this to perform custom cleanup logic.
    ///
    /// # Errors
    ///
    /// Returns an error when the component cannot stop cleanly.
    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        Ok(())
    }

    /// Returns a provider-specific service message limit, if one is stricter than the host limit.
    fn service_message_limit(&self) -> Option<usize> {
        None
    }

    /// Handles one generic service request routed to this component.
    ///
    /// Service-specific success and domain failure payloads belong to the
    /// versioned contract. This method only returns an SDK error when the
    /// provider itself cannot execute the request.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::ServiceHandlerUnavailable`] by default or a
    /// component-specific execution failure from an implementation.
    fn handle_service(
        &mut self,
        _ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        _request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        Err(ExtensionError::ServiceHandlerUnavailable(
            contract.to_string(),
        ))
    }
}
