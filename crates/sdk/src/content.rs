//! Generic extension-provided RTW content-handler contracts.

use serde::{Deserialize, Serialize};

use crate::contracts::{ContractKey, ContractVersion};

/// Major version of the platform-owned RTW content-handler service protocol.
pub const CONTENT_HANDLER_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(1);
/// Maximum content descriptor bytes passed to one handler validation call.
pub const MAX_CONTENT_HANDLER_ENTRY_BYTES: usize = 64 * 1024;
/// Maximum human-readable rejection diagnostic accepted from a content handler.
pub const MAX_CONTENT_HANDLER_DIAGNOSTIC_BYTES: usize = 8 * 1024;

/// Returns the platform service contract used to validate one exact RTW content type.
///
/// `content_id` is the namespace-qualified identifier without its major version,
/// while `content_major` is the major version declared by `rtw.toml`.
pub fn content_handler_service_contract_key(content_id: &str, content_major: u32) -> ContractKey {
    ContractKey::new(
        format!("rintawa.content.handler.{content_id}.v{content_major}"),
        CONTENT_HANDLER_SERVICE_PROTOCOL_VERSION,
    )
}

/// Host request asking an extension handler to validate one RTW content descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentHandlerRequest {
    /// Canonical versioned content identity from `rtw.toml`.
    pub content: String,
    /// Exact bounded bytes stored at the root manifest's `entry` path.
    pub descriptor: Vec<u8>,
}

impl ContentHandlerRequest {
    /// Creates one descriptor-validation request.
    pub fn new(content: impl Into<String>, descriptor: Vec<u8>) -> Self {
        Self {
            content: content.into(),
            descriptor,
        }
    }
}

/// Extension handler decision for one immutable RTW content descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ContentHandlerResponse {
    /// The descriptor is valid for the exact versioned content type.
    Accepted,
    /// The descriptor is invalid or unsupported by this handler.
    Rejected {
        /// Bounded human-readable diagnostic suitable for logs/UI.
        diagnostic: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_derive_stable_content_handler_contract_key() {
        let key = content_handler_service_contract_key("rintawa.character-template", 1);
        assert_eq!(
            key.to_string(),
            "rintawa.content.handler.rintawa.character-template.v1@1"
        );
    }

    #[test]
    fn test_should_round_trip_content_handler_message() -> serde_json::Result<()> {
        let request = ContentHandlerRequest::new(
            "rintawa.character-template@1",
            br#"{"name":"Alice"}"#.to_vec(),
        );
        let encoded = serde_json::to_vec(&request)?;
        let decoded: ContentHandlerRequest = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, request);

        let response = ContentHandlerResponse::Rejected {
            diagnostic: String::from("missing required field"),
        };
        let encoded = serde_json::to_vec(&response)?;
        let decoded: ContentHandlerResponse = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, response);
        Ok(())
    }
}
