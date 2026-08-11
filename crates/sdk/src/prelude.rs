//! Prelude — a convenience module for common imports.
//!
//! This module re-exports the most commonly used types and traits
//! from the Taverna SDK, making it easy to import everything
//! needed for extension development with a single `use` statement.
//!
//! # Example
//!
//! ```rust
//! use taverna_sdk::prelude::*;
//! ```

pub use crate::api::{LogLevel, LoggerApi};

pub use crate::context::{ComponentContext, RegistrationContext};

pub use crate::contributions::{ContributionDescriptor, ContributionKind};

pub use crate::errors::{ExtensionError, ExtensionResult};

pub use crate::manifest::{ComponentDescriptor, ComponentKind, ExtensionManifest};

pub use crate::traits::Component;

pub use crate::types::{ComponentId, ComponentTarget, ContributionId, ExtensionId};
