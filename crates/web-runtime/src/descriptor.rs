//! Web bundle target descriptor.

use std::collections::HashSet;

use rintawa_artifacts::ArtifactPath;
use rintawa_sdk::ui::{PORTABLE_UI_PROTOCOL_MAJOR, UiCapabilityId, UiLayerDescriptor};
use serde::{Deserialize, Serialize};

use crate::{WebRuntimeError, WebRuntimeResult};

/// Versioned execution target for sandboxed Web bundle components.
pub const WEB_BUNDLE_TARGET_V1: &str = "rintawa.runtime.web-bundle@1";
/// Current `web-layer.toml` schema version.
pub const WEB_BUNDLE_DESCRIPTOR_SCHEMA: u32 = 1;
/// Current host/renderer bridge protocol major version.
pub const WEB_UI_BRIDGE_PROTOCOL_MAJOR: u32 = 1;
/// Maximum accepted uncompressed `web-layer.toml` size.
pub const WEB_BUNDLE_DESCRIPTOR_MAX_BYTES: usize = 64 * 1024;

/// Target-specific descriptor for one Web bundle component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebBundleDescriptor {
    /// Descriptor schema version.
    pub schema: u32,
    /// Host/renderer bridge protocol major version.
    #[serde(rename = "bridge-protocol-major")]
    pub bridge_protocol_major: u32,
    /// HTML entry point relative to this descriptor.
    pub entry: ArtifactPath,
    /// Preferred loopback TCP port for this packaged Web bundle.
    ///
    /// When omitted, the Web host asks the OS for an ephemeral port. Development
    /// tooling may also override this value to allow concurrent sessions.
    #[serde(default, rename = "listen-port")]
    pub listen_port: Option<u16>,
    /// Optional Portable UI Layer role exposed by this Web component.
    #[serde(default, rename = "ui-layer")]
    pub ui_layer: Option<WebUiLayerDescriptor>,
}

/// Portable UI capabilities exposed by the Web renderer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiLayerDescriptor {
    /// Portable UI protocol major version.
    #[serde(rename = "protocol-major")]
    pub protocol_major: u32,
    /// Renderer capabilities.
    pub capabilities: Vec<UiCapabilityId>,
}

impl WebBundleDescriptor {
    /// Parses and validates a Web bundle descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, malformed, or incompatible metadata.
    pub fn parse(bytes: &[u8]) -> WebRuntimeResult<Self> {
        if bytes.len() > WEB_BUNDLE_DESCRIPTOR_MAX_BYTES {
            return Err(WebRuntimeError::DescriptorTooLarge {
                actual: bytes.len(),
                maximum: WEB_BUNDLE_DESCRIPTOR_MAX_BYTES,
            });
        }
        let source = std::str::from_utf8(bytes).map_err(|_| WebRuntimeError::DescriptorEncoding)?;
        let descriptor: Self = toml::from_str(source)?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Validates protocol versions and capability uniqueness.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported versions or duplicate capabilities.
    pub fn validate(&self) -> WebRuntimeResult<()> {
        if self.schema != WEB_BUNDLE_DESCRIPTOR_SCHEMA {
            return Err(WebRuntimeError::UnsupportedDescriptorSchema(self.schema));
        }
        if self.listen_port == Some(0) {
            return Err(WebRuntimeError::InvalidListenPort);
        }
        if self.bridge_protocol_major != WEB_UI_BRIDGE_PROTOCOL_MAJOR {
            return Err(WebRuntimeError::UnsupportedBridgeProtocol(
                self.bridge_protocol_major,
            ));
        }
        if let Some(ui_layer) = &self.ui_layer {
            if ui_layer.protocol_major != PORTABLE_UI_PROTOCOL_MAJOR {
                return Err(WebRuntimeError::UnsupportedPortableUiProtocol(
                    ui_layer.protocol_major,
                ));
            }

            let mut capabilities = HashSet::new();
            for capability in &ui_layer.capabilities {
                if !capabilities.insert(capability.as_str()) {
                    return Err(WebRuntimeError::DuplicateCapability(capability.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Returns the renderer-neutral UI Layer descriptor when this component exposes that role.
    pub fn ui_layer_descriptor(&self) -> Option<UiLayerDescriptor> {
        self.ui_layer.as_ref().map(|ui_layer| UiLayerDescriptor {
            protocol_major: ui_layer.protocol_major,
            capabilities: ui_layer.capabilities.clone(),
        })
    }
}
