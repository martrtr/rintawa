//! Types for Taverna extension development.
//!
//! This module provides identifier types and primitives
//! used throughout the Taverna extension system.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Generates a documented string identifier type with common conversions.
///
/// This macro creates a `#[serde(transparent)]` newtype wrapper
/// around `String` with standard conversions.
///
/// # Generated API
///
/// - `new(value: impl Into<String>) -> Self`
/// - `as_str(&self) -> &str`
/// - `impl Display for Self`
/// - `impl From<&str> for Self`
/// - `impl From<String> for Self`
macro_rules! string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// This transparent `String` wrapper prevents identifiers from being
        /// accidentally exchanged across distinct Taverna concepts.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new identifier from a string-like value.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(ExtensionId, "An identifier for an extension package.");
string_id!(
    ComponentId,
    "An identifier for a component in an extension."
);
string_id!(
    ContributionId,
    "An identifier for an extension contribution."
);
string_id!(
    RuntimeEffectId,
    "An identifier for one reversible runtime effect owned by a component."
);
string_id!(
    ComponentTarget,
    "An identifier for the host contract that runs a component."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_id_creation() {
        let id = ExtensionId::new("test-extension");
        assert_eq!(id.as_str(), "test-extension");
    }

    #[test]
    fn test_extension_id_from_str() {
        let id: ExtensionId = "test".into();
        assert_eq!(id.as_str(), "test");
    }

    #[test]
    fn test_component_id_from_string() {
        let id: ComponentId = String::from("component-123").into();
        assert_eq!(id.as_str(), "component-123");
    }

    #[test]
    fn test_contribution_id_new() {
        let id = ContributionId::new("my-contribution");
        assert_eq!(id.as_str(), "my-contribution");
    }

    #[test]
    fn test_display_impl() {
        let id = ExtensionId::new("display-test");
        assert_eq!(format!("{id}"), "display-test");
    }

    #[test]
    fn test_extension_id_json_round_trip() -> serde_json::Result<()> {
        let original = ExtensionId::new("taverna.chat");
        let encoded = serde_json::to_string(&original)?;
        let decoded: ExtensionId = serde_json::from_str(&encoded)?;

        assert_eq!(encoded, "\"taverna.chat\"");
        assert_eq!(decoded, original);

        Ok(())
    }

    #[test]
    fn test_component_target_json_round_trip() -> serde_json::Result<()> {
        let original = ComponentTarget::new("wasm");
        let encoded = serde_json::to_string(&original)?;
        let decoded: ComponentTarget = serde_json::from_str(&encoded)?;

        assert_eq!(decoded, original);

        Ok(())
    }
}
