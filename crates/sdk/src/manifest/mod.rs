//! Types for Rintawa extension manifests.
//!
//! This module provides types for describing the structure of extension manifests
//! and the components that extensions declare.

use serde::{Deserialize, Serialize};

use crate::{
    secrets::SecretPathPattern,
    types::{ComponentId, ComponentTarget, ExtensionId},
};

/// Represents the role of a component in an extension.
///
/// A runtime component can run on different hosts. Its execution model is
/// selected by [`ComponentDescriptor::target`], for example `"native"` or
/// `"wasm"`. A UI component uses a host-specific target such as `"web"`.
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

/// Permissions a component may request from a Rintawa host.
///
/// A request is only metadata. It does not grant access: Rintawa must approve
/// an exact requested pattern for a specific component before activation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentPermissions {
    /// Secret domains the extension may ask the host to grant for reading.
    #[serde(default, rename = "secret-read")]
    pub secret_read: Vec<SecretPathPattern>,
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

    /// The host contract that runs this component.
    ///
    /// Initial runtime targets are `"native"` and `"wasm"`; `"web"` is a
    /// UI target. The target names are stable SDK data, while the host
    /// implementations remain outside this crate.
    pub target: ComponentTarget,

    /// The host-specific entry point for this component, when one is needed.
    ///
    /// A statically registered native component may omit this field. A WASM or
    /// web component normally supplies an entry such as `"runtime.wasm"`.
    #[serde(default)]
    pub entry: Option<String>,

    /// Whether activation requires a compatible host for this component.
    #[serde(default = "required_by_default")]
    pub required: bool,

    /// Permissions requested by this component.
    #[serde(default)]
    pub permissions: ComponentPermissions,
}

/// The minimal manifest for a Rintawa extension package.
///
/// The manifest is the primary way to declare an extension's metadata and
/// components. It serves as the entry point for Extension Engine parsing and
/// validation. SDK 0.0.1 does not yet define validation or runtime semantics
/// for inter-extension dependencies or declarative contributions. Component
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_parse_minimal_native_runtime_manifest() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
                id = "chat"
                name = "Chat"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "runtime"
                kind = "runtime"
                target = "native"
            "#,
        )?;

        let component = &manifest.components[0];
        assert_eq!(manifest.id.as_str(), "chat");
        assert_eq!(component.kind, ComponentKind::Runtime);
        assert_eq!(component.target.as_str(), "native");
        assert_eq!(component.entry, None);
        assert!(component.required);
        assert!(component.permissions.secret_read.is_empty());

        Ok(())
    }

    #[test]
    fn test_should_parse_wasm_runtime_manifest_with_entry() -> Result<(), toml::de::Error> {
        let manifest: ExtensionManifest = toml::from_str(
            r#"
                id = "chat"
                name = "Chat"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "runtime"
                kind = "runtime"
                target = "wasm"
                entry = "runtime.wasm"
                required = false
            "#,
        )?;

        let component = &manifest.components[0];
        assert_eq!(component.target.as_str(), "wasm");
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
                target: ComponentTarget::new("wasm"),
                entry: Some(String::from("runtime.wasm")),
                required: true,
                permissions: ComponentPermissions {
                    secret_read: vec![SecretPathPattern::parse("ai.api_keys.*")?],
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
                target = "native"

                [components.permissions]
                secret-read = ["ai.api_keys.*"]
            "#,
        )?;

        assert_eq!(
            manifest.components[0].permissions.secret_read,
            vec![SecretPathPattern::parse("ai.api_keys.*").unwrap()]
        );

        Ok(())
    }
}
