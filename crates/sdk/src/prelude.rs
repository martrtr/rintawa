//! Prelude — a convenience module for common imports.
//!
//! This module re-exports the most commonly used types and traits
//! from the Rintawa SDK, making it easy to import everything
//! needed for extension development with a single `use` statement.
//!
//! # Example
//!
//! ```rust
//! use rintawa_sdk::prelude::*;
//! ```

pub use crate::api::{LogLevel, LoggerApi};

pub use crate::context::{ComponentContext, RegistrationContext};

pub use crate::contracts::{
    ComponentRef, ContractConsumer, ContractDefinition, ContractGrantRequirement, ContractKey,
    ContractProtocol, ContractProvider, ContractResolutionPolicy, ContractVersion,
    HOST_SHELL_CONTRACT_ID, HOST_SHELL_CONTRACT_VERSION, host_shell_contract_key,
};
pub use crate::contributions::{ContributionDescriptor, ContributionKind};

pub use crate::errors::{ExtensionError, ExtensionResult};

pub use crate::manifest::{
    ComponentDescriptor, ComponentKind, ComponentPermissions, ExtensionManifest,
    ManifestValidationError, WASM_COMPONENT_TARGET_V1,
};

pub use crate::runtime_effects::RuntimeEffect;

pub use crate::secrets::{SecretAccessError, SecretPath, SecretPathPattern, SecretValue};

pub use crate::services::{ServiceCallError, ServiceCallResult};

pub use crate::traits::Component;

pub use crate::types::{
    ComponentId, ComponentTarget, ContractId, ContributionId, ExtensionId, ExtensionInstanceId,
    RuntimeEffectId, RuntimeScopeId,
};

pub use crate::ui::{
    PORTABLE_UI_PROTOCOL_MAJOR, UiActionEvent, UiActionId, UiActionPayload, UiButtonNode,
    UiCapabilityId, UiContainerNode, UiError, UiLayerDescriptor, UiMarkdownNode, UiNode, UiNodeId,
    UiNodeKind, UiPatch, UiPatchBatch, UiPlacementHint, UiResult, UiSurfaceContribution,
    UiSurfaceId, UiSurfaceSnapshot, UiTextAreaNode, UiTextInputNode, UiTextNode,
};
