//! Public types for versioned component contracts.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    secrets::SecretPathPattern,
    types::{ComponentId, ContractId, ExtensionId},
};

/// A major version of a public contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContractVersion(u32);

impl ContractVersion {
    /// Creates a contract version.
    pub const fn new(major: u32) -> Self {
        Self(major)
    }

    /// Returns the major version number.
    pub const fn major(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Identifies one major version of a contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContractKey {
    /// Stable contract identifier.
    pub id: ContractId,
    /// Major contract version.
    pub version: ContractVersion,
}

impl ContractKey {
    /// Creates a contract key.
    pub fn new(id: impl Into<ContractId>, version: ContractVersion) -> Self {
        Self {
            id: id.into(),
            version,
        }
    }
}

impl fmt::Display for ContractKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

/// Identifies one component across extension packages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComponentRef {
    /// Extension package identifier.
    pub extension_id: ExtensionId,
    /// Component identifier within the package.
    pub component_id: ComponentId,
}

impl ComponentRef {
    /// Creates a component reference.
    pub fn new(extension_id: impl Into<ExtensionId>, component_id: impl Into<ComponentId>) -> Self {
        Self {
            extension_id: extension_id.into(),
            component_id: component_id.into(),
        }
    }
}

/// Determines how providers of one contract are composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContractResolutionPolicy {
    /// Exactly one provider is bound to each consumer.
    Single,
    /// Every eligible provider is bound to each consumer.
    Multiple,
}

impl fmt::Display for ContractResolutionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single => f.write_str("single"),
            Self::Multiple => f.write_str("multiple"),
        }
    }
}

/// Defines a contract and its provider resolution policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractDefinition {
    /// Contract being defined.
    pub contract: ContractKey,
    /// Provider composition policy.
    pub resolution: ContractResolutionPolicy,
}

impl ContractDefinition {
    /// Creates a contract definition.
    pub fn new(contract: ContractKey, resolution: ContractResolutionPolicy) -> Self {
        Self {
            contract,
            resolution,
        }
    }
}

/// A host grant that must exist before a contract endpoint is eligible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ContractGrantRequirement {
    /// The component must hold a host-approved secret-read grant.
    SecretRead {
        /// Secret path or domain required by the endpoint.
        pattern: SecretPathPattern,
    },
}

/// Declares that a component provides a contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractProvider {
    /// Contract implemented by the component.
    pub contract: ContractKey,
    /// Host grants required before this provider can be selected.
    #[serde(default)]
    pub required_grants: Vec<ContractGrantRequirement>,
}

impl ContractProvider {
    /// Creates a provider without host-grant requirements.
    pub fn new(contract: ContractKey) -> Self {
        Self {
            contract,
            required_grants: Vec::new(),
        }
    }

    /// Requires one host grant before this provider is eligible.
    pub fn requiring(mut self, requirement: ContractGrantRequirement) -> Self {
        self.required_grants.push(requirement);
        self
    }
}

/// Declares that a component consumes a contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractConsumer {
    /// Contract required by the component.
    pub contract: ContractKey,
    /// Whether missing resolution is a required dependency for this consumer.
    pub required: bool,
    /// Host grants required before this consumer endpoint is eligible.
    #[serde(default)]
    pub required_grants: Vec<ContractGrantRequirement>,
}

impl ContractConsumer {
    /// Creates a consumer declaration.
    pub fn new(contract: ContractKey, required: bool) -> Self {
        Self {
            contract,
            required,
            required_grants: Vec::new(),
        }
    }

    /// Requires one host grant before this consumer is eligible.
    pub fn requiring(mut self, requirement: ContractGrantRequirement) -> Self {
        self.required_grants.push(requirement);
        self
    }
}
