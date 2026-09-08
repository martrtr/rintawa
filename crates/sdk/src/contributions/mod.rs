//! Types for describing extension contributions.
//!
//! This module provides types for representing different kinds of contributions
//! that extensions can make to the Rintawa system.

use serde::{Deserialize, Serialize};

use crate::types::ContributionId;

/// Represents the kind of contribution an extension can make.
///
/// Contribution kinds define what role an extension plays within the Rintawa system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContributionKind(String);

impl ContributionKind {
    /// Creates a new contribution kind from a string-like value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the contribution kind as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a system-level contribution kind.
    ///
    /// System contributions are made by the core Rintawa system itself.
    pub fn system() -> Self {
        Self::new("system")
    }

    /// Creates a service-level contribution kind.
    ///
    /// Service contributions provide functionality to other extensions.
    pub fn service() -> Self {
        Self::new("service")
    }

    /// Creates a command-level contribution kind.
    ///
    /// Command contributions add new commands to the system.
    pub fn command() -> Self {
        Self::new("command")
    }

    /// Creates a surface-level contribution kind.
    ///
    /// Surface contributions add UI elements to the system.
    pub fn surface() -> Self {
        Self::new("surface")
    }

    /// Creates an object-type contribution kind.
    pub fn object_type() -> Self {
        Self::new("object-type")
    }

    /// Creates a relation-type contribution kind.
    pub fn relation_type() -> Self {
        Self::new("relation-type")
    }

    /// Creates an event-type contribution kind.
    pub fn event_type() -> Self {
        Self::new("event-type")
    }

    /// Creates a static capability-contract contribution kind.
    pub fn capability() -> Self {
        Self::new("capability")
    }
}

/// A descriptor for an extension contribution.
///
/// This struct combines an identifier with a contribution kind
/// to fully describe what an extension contributes to the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionDescriptor {
    /// The unique identifier of the contribution.
    pub id: ContributionId,

    /// The kind of contribution this is.
    pub kind: ContributionKind,
}

impl ContributionDescriptor {
    /// Creates a new contribution descriptor.
    pub fn new(id: impl Into<ContributionId>, kind: ContributionKind) -> Self {
        Self {
            id: id.into(),
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contribution_kind_system() {
        let kind = ContributionKind::system();
        assert_eq!(kind.as_str(), "system");
    }

    #[test]
    fn test_contribution_kind_capability() {
        assert_eq!(ContributionKind::capability().as_str(), "capability");
    }

    #[test]
    fn test_contribution_descriptor_new() {
        let descriptor = ContributionDescriptor::new("chat.system", ContributionKind::system());

        assert_eq!(descriptor.id.as_str(), "chat.system");
        assert_eq!(descriptor.kind.as_str(), "system");
    }

    #[test]
    fn test_contribution_descriptor_json_round_trip() -> serde_json::Result<()> {
        let original = ContributionDescriptor::new("chat.send", ContributionKind::command());
        let encoded = serde_json::to_string(&original)?;
        let decoded: ContributionDescriptor = serde_json::from_str(&encoded)?;

        assert_eq!(decoded, original);

        Ok(())
    }
}
