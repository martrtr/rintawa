//! Types for Rintawa extension manifests.
//!
//! This module provides types for describing the structure of extension manifests
//! and the components that extensions declare.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    runtime_permissions::RuntimePermission,
    secrets::SecretPathPattern,
    types::{ComponentId, ComponentTarget, ExtensionId},
};

/// Versioned execution target for a WebAssembly Component hosted by Rintawa.
pub const WASM_COMPONENT_TARGET_V1: &str = "rintawa.runtime.wasm-component@1";

/// Coarse package classification for a component.
///
/// Product roles are expressed through versioned contracts, not through this
/// enum. [`ComponentDescriptor::target`] independently selects the execution ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    /// Executable extension logic.
    Runtime,
    /// A host-specific user interface component.
    Ui,
}

fn required_by_default() -> bool {
    true
}

/// Validation failure for one versioned component execution target.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid component target `{value}`: {reason}")]
pub struct ComponentTargetValidationError {
    value: String,
    reason: &'static str,
}

impl ComponentTargetValidationError {
    /// Returns the rejected target string.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the stable validation reason.
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Validates a canonical versioned component execution target.
///
/// Targets use `<namespace>.<name>@<major>` identity. Namespace segments use
/// lowercase ASCII letters, digits, `_`, or `-`, and each segment starts and
/// ends with an ASCII alphanumeric character.
///
/// # Errors
///
/// Returns [`ComponentTargetValidationError`] when the target is malformed or
/// its major version is not canonical unsigned decimal notation.
pub fn validate_component_target(value: &str) -> Result<(), ComponentTargetValidationError> {
    let invalid = |reason| ComponentTargetValidationError {
        value: value.to_string(),
        reason,
    };
    let (id, major) = value
        .rsplit_once('@')
        .ok_or_else(|| invalid("expected `<namespace>.<name>@<major>`"))?;
    if id.len() > 128 {
        return Err(invalid("identifier must not exceed 128 bytes"));
    }
    let segments: Vec<_> = id.split('.').collect();
    if segments.len() < 2 {
        return Err(invalid(
            "identifier must contain at least one namespace separator `.`",
        ));
    }
    for segment in segments {
        if segment.is_empty() {
            return Err(invalid("namespace segments cannot be empty"));
        }
        if !segment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        }) {
            return Err(invalid(
                "identifier uses characters outside `[a-z0-9_-]` and `.` separators",
            ));
        }
        let first = segment.as_bytes().first();
        let last = segment.as_bytes().last();
        if !first.is_some_and(u8::is_ascii_alphanumeric)
            || !last.is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err(invalid(
                "namespace segments must start and end with an ASCII letter or digit",
            ));
        }
    }
    if major.len() > 1 && major.starts_with('0') {
        return Err(invalid("major version must use canonical decimal notation"));
    }
    major
        .parse::<u32>()
        .map_err(|_| invalid("major version must be an unsigned integer"))?;
    Ok(())
}

/// Permissions a component may request from a Rintawa host.
///
/// A request is only metadata. It does not grant access: Rintawa must approve
/// an exact requested pattern for a specific component before activation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentPermissions {
    /// Secret domains the extension may ask the host to grant for reading.
    #[serde(default, rename = "secret-read")]
    pub secret_read: Vec<SecretPathPattern>,
    /// Host runtime capabilities this component may ask policy to approve.
    #[serde(default, rename = "runtime")]
    pub runtime: Vec<RuntimePermission>,
}

/// Describes a single component declared by an extension.
///
/// Components represent the building blocks of an extension's functionality
/// and are declared in the extension's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    /// The unique identifier of this component.
    pub id: ComponentId,

    /// The kind of component this is.
    pub kind: ComponentKind,

    /// The versioned execution target that runs this component.
    ///
    /// Published WASM packages use [`WASM_COMPONENT_TARGET_V1`]. Other targets
    /// are provider-defined ABI identities registered through the host execution-
    /// target registry; presentation roles remain separate versioned contracts.
    pub target: ComponentTarget,

    /// The target-specific entry point for this component, when one is needed.
    ///
    /// For `rintawa.extension@1` RTW content, relative entries are resolved from
    /// the directory containing the extension manifest selected by `rtw.toml`.
    /// Interpretation beyond that path boundary belongs to the selected target host.
    #[serde(default)]
    pub entry: Option<String>,

    /// Whether activation requires a compatible host for this component.
    #[serde(default = "required_by_default")]
    pub required: bool,

    /// Permissions requested by this component.
    #[serde(default)]
    pub permissions: ComponentPermissions,
}

/// Semantic validation errors for an extension manifest.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ManifestValidationError {
    /// Two or more components use the same component identifier.
    #[error("duplicate component ID `{component_id}` in extension `{extension_id}`")]
    DuplicateComponentId {
        /// Extension containing the duplicate component declaration.
        extension_id: ExtensionId,
        /// Component identifier declared more than once.
        component_id: ComponentId,
    },
    /// A component declares a malformed or unversioned execution target.
    #[error("component `{component_id}` in extension `{extension_id}` has {source}")]
    InvalidComponentTarget {
        /// Extension containing the invalid target declaration.
        extension_id: ExtensionId,
        /// Component containing the invalid target declaration.
        component_id: ComponentId,
        /// Exact target validation failure.
        #[source]
        source: ComponentTargetValidationError,
    },
}

/// The minimal manifest for a Rintawa extension package.
///
/// The manifest is the primary way to declare an extension's metadata and
/// components. Structural validity is enforced during deserialization, while
/// [`ExtensionManifest::validate`] enforces semantic package invariants such as
/// unique component IDs.
///
/// SDK 0.0.1 does not yet define validation or runtime semantics for
/// inter-extension dependencies or declarative contributions. Component
/// permission requests are parsed, but their grants remain an explicit host
/// policy decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    /// The unique identifier of the extension.
    pub id: ExtensionId,

    /// The human-readable name of the extension.
    pub name: String,

    /// The version of the extension following semantic versioning.
    pub version: String,

    /// The version of the SDK this extension targets.
    pub sdk: String,

    /// Components defined by the extension.
    #[serde(default)]
    pub components: Vec<ComponentDescriptor>,
}

impl ExtensionManifest {
    /// Validates semantic invariants that cannot be enforced by TOML parsing.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestValidationError::DuplicateComponentId`] when multiple
    /// component descriptors declare the same component ID, or
    /// [`ManifestValidationError::InvalidComponentTarget`] when a component
    /// target is not a canonical versioned execution identity.
    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        let mut component_ids = HashSet::with_capacity(self.components.len());
        for component in &self.components {
            if !component_ids.insert(component.id.as_str()) {
                return Err(ManifestValidationError::DuplicateComponentId {
                    extension_id: self.id.clone(),
                    component_id: component.id.clone(),
                });
            }
            validate_component_target(component.target.as_str()).map_err(|source| {
                ManifestValidationError::InvalidComponentTarget {
                    extension_id: self.id.clone(),
                    component_id: component.id.clone(),
                    source,
                }
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_parse_minimal_provider_runtime_manifest() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
                id = "chat"
                name = "Chat"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "runtime"
                kind = "runtime"
                target = "example.runtime.test@1"
            "#,
        )?;

        let component = &manifest.components[0];
        assert_eq!(manifest.id.as_str(), "chat");
        assert_eq!(component.kind, ComponentKind::Runtime);
        assert_eq!(component.target.as_str(), "example.runtime.test@1");
        assert_eq!(component.entry, None);
        assert!(component.required);
        assert!(component.permissions.secret_read.is_empty());

        Ok(())
    }

    #[test]
    fn test_should_parse_versioned_wasm_runtime_manifest_with_entry() -> Result<(), toml::de::Error>
    {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
                id = "chat"
                name = "Chat"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "runtime"
                kind = "runtime"
                target = "rintawa.runtime.wasm-component@1"
                entry = "runtime.wasm"
                required = false
            "#,
        )?;

        let component = &manifest.components[0];
        assert_eq!(component.target.as_str(), WASM_COMPONENT_TARGET_V1);
        assert_eq!(component.entry.as_deref(), Some("runtime.wasm"));
        assert!(!component.required);

        Ok(())
    }

    #[test]
    fn test_manifest_toml_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let original = ExtensionManifest {
            id: ExtensionId::new("chat"),
            name: String::from("Chat"),
            version: String::from("0.0.1"),
            sdk: String::from("^0.0"),
            components: vec![ComponentDescriptor {
                id: ComponentId::new("runtime"),
                kind: ComponentKind::Runtime,
                target: ComponentTarget::new("example.runtime.test@1"),
                entry: Some(String::from("runtime.wasm")),
                required: true,
                permissions: ComponentPermissions {
                    secret_read: vec![SecretPathPattern::parse("ai.api_keys.*")?],
                    runtime: vec![
                        RuntimePermission::BackgroundTask,
                        RuntimePermission::LoopbackListen,
                    ],
                },
            }],
        };

        let encoded = toml::to_string(&original)?;
        let decoded: ExtensionManifest = toml::from_str(&encoded)?;

        assert_eq!(decoded, original);

        Ok(())
    }

    #[test]
    fn test_should_parse_requested_secret_domain() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
                id = "official_ai"
                name = "Official AI"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "provider"
                kind = "runtime"
                target = "example.runtime.test@1"

                [components.permissions]
                secret-read = ["ai.api_keys.*"]
                runtime = ["background-task", "loopback-listen"]
            "#,
        )?;

        assert_eq!(
            manifest.components[0].permissions.secret_read,
            vec![SecretPathPattern::parse("ai.api_keys.*").unwrap()]
        );
        assert_eq!(
            manifest.components[0].permissions.runtime,
            vec![
                RuntimePermission::BackgroundTask,
                RuntimePermission::LoopbackListen
            ]
        );

        Ok(())
    }
    #[test]
    fn test_should_reject_duplicate_component_ids() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
            id = "duplicate-components"
            name = "Duplicate Components"
            version = "0.0.1"
            sdk = "^0.0"
            [[components]]
            id = "runtime"
            kind = "runtime"
            target = "example.runtime.test@1"
            [[components]]
            id = "runtime"
            kind = "runtime"
            target = "example.runtime.test@1"
        "#,
        )?;
        assert_eq!(
            manifest.validate(),
            Err(ManifestValidationError::DuplicateComponentId {
                extension_id: ExtensionId::new("duplicate-components"),
                component_id: ComponentId::new("runtime"),
            })
        );
        Ok(())
    }

    #[test]
    fn test_should_reject_unversioned_component_target() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
            id = "invalid-target"
            name = "Invalid Target"
            version = "0.0.1"
            sdk = "^0.0"
            [[components]]
            id = "runtime"
            kind = "runtime"
            target = "runtime"
        "#,
        )?;

        assert!(matches!(
            manifest.validate(),
            Err(ManifestValidationError::InvalidComponentTarget {
                component_id,
                source,
                ..
            }) if component_id == ComponentId::new("runtime")
                && source.reason() == "expected `<namespace>.<name>@<major>`"
        ));
        Ok(())
    }

    #[test]
    fn test_should_accept_unique_component_ids() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
            id = "unique-components"
            name = "Unique Components"
            version = "0.0.1"
            sdk = "^0.0"
            [[components]]
            id = "runtime"
            kind = "runtime"
            target = "rintawa.runtime.wasm-component@1"
            [[components]]
            id = "settings-ui"
            kind = "ui"
            target = "example.runtime.ui@1"
        "#,
        )?;
        assert_eq!(manifest.validate(), Ok(()));
        Ok(())
    }
}
