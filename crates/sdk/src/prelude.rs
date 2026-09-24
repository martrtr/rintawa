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
    HOST_SHELL_CONTRACT_ID, HOST_SHELL_CONTRACT_VERSION, UI_LAYER_CONTRACT_ID,
    UI_LAYER_CONTRACT_VERSION, host_shell_contract_key, ui_layer_contract_key,
};
pub use crate::contributions::{ContributionDescriptor, ContributionKind};

pub use crate::errors::{ExtensionError, ExtensionResult};

pub use crate::manifest::{
    ComponentDescriptor, ComponentKind, ComponentPermissions, ComponentTargetValidationError,
    ExtensionManifest, ManifestValidationError, WASM_COMPONENT_TARGET_V1,
    validate_component_target,
};

pub use crate::runtime_effects::RuntimeEffect;
pub use crate::runtime_permissions::{RuntimePermission, RuntimePermissionParseError};
pub use crate::runtime_signals::RuntimeSignal;

pub use crate::secrets::{SecretAccessError, SecretPath, SecretPathPattern, SecretValue};

pub use crate::services::{ServiceCallError, ServiceCallResult};

pub use crate::traits::Component;

pub use crate::types::{
    ComponentId, ComponentTarget, ContractId, ContributionId, ExtensionId, ExtensionInstanceId,
    RuntimeEffectId, RuntimeScopeId,
};

pub use crate::ui::{
    PORTABLE_UI_PROTOCOL_MAJOR, UiActionEvent, UiActionId, UiActionPayload, UiActivityContribution,
    UiActivityId, UiButtonAppearance, UiButtonNode, UiCapabilityId, UiCheckboxNode,
    UiContainerNode, UiDataGridColumn, UiDataGridNode, UiDataGridSortDirection, UiError,
    UiIconNode, UiIconSlotId, UiImageNode, UiLayerDescriptor, UiMarkdownNode, UiNode, UiNodeId,
    UiNodeKind, UiNodeSemanticTraitId, UiPatch, UiPatchBatch, UiPlacementHint, UiResult,
    UiSelectNode, UiSelectOption, UiSplitAxis, UiSplitNode, UiSurfaceContribution, UiSurfaceId,
    UiSurfaceSnapshot, UiSurfaceTraitId, UiTextAreaNode, UiTextInputNode, UiTextNode,
};

pub use crate::world::{
    CommandId, ControlGrantId, CorrelationId, EffectJobId, EntityId, PrincipalId, RelationId,
    SchemaId, SchemaIdentifierError, SchemaKey, SchemaKeyParseError, SchemaVersion,
    SchemaVersionError, UnixTimeMillis, WorldEventId, WorldId,
};
