//! Component lifecycle contract implemented by extension runtime code.
//!
//! The Extension Engine owns component creation, lifecycle ordering, rollback,
//! and removal of registered contributions. Implementations only describe their
//! identity and registration/start/stop behavior.

use crate::{
    context::{ComponentContext, RegistrationContext},
    errors::ExtensionResult,
    types::ComponentId,
};

/// The trait that all components must implement.
///
/// This trait defines the lifecycle methods that all components must provide.
/// Components are the building blocks of Taverna extensions.
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
}
