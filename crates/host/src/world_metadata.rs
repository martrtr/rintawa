//! Validated human-facing metadata embedded in the authoritative World manifest.

use rintawa_artifacts::AssetRef;
use rintawa_world::WorldManifest;
use serde::{Deserialize, Serialize};

use crate::{HostError, HostResult};

/// Maximum UTF-8 bytes allowed in a world title.
pub const MAX_WORLD_TITLE_BYTES: usize = 256;
/// Maximum UTF-8 bytes allowed in a world description.
pub const MAX_WORLD_DESCRIPTION_BYTES: usize = 4 * 1024;

/// Host-owned metadata in the World manifest, not a separate persistence layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorldMetadata {
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover: Option<AssetRef>,
}

impl WorldMetadata {
    pub(crate) fn new(
        title: impl Into<String>,
        description: Option<String>,
        cover: Option<AssetRef>,
    ) -> HostResult<Self> {
        let title = title.into().trim().to_string();
        if title.is_empty() || title.len() > MAX_WORLD_TITLE_BYTES {
            return Err(HostError::InvalidWorldTitle {
                actual_bytes: title.len(),
                maximum_bytes: MAX_WORLD_TITLE_BYTES,
            });
        }
        let description = description.map(|value| value.trim().to_string());
        let description = description.filter(|value| !value.is_empty());
        if let Some(value) = &description
            && value.len() > MAX_WORLD_DESCRIPTION_BYTES
        {
            return Err(HostError::InvalidWorldDescription {
                actual_bytes: value.len(),
                maximum_bytes: MAX_WORLD_DESCRIPTION_BYTES,
            });
        }
        Ok(Self {
            title,
            description,
            cover,
        })
    }

    pub(crate) fn from_manifest(manifest: &WorldManifest) -> HostResult<Self> {
        let metadata = manifest
            .as_value()
            .get("metadata")
            .ok_or_else(|| HostError::InvalidWorldMetadata("missing manifest.metadata".into()))?;
        let decoded: Self = serde_json::from_value(metadata.clone())
            .map_err(|error| HostError::InvalidWorldMetadata(error.to_string()))?;
        Self::new(decoded.title, decoded.description, decoded.cover)
    }

    pub(crate) fn write_to_manifest(&self, manifest: &WorldManifest) -> HostResult<WorldManifest> {
        let mut document = manifest
            .as_value()
            .as_object()
            .ok_or_else(|| HostError::InvalidWorldMetadata("manifest must be an object".into()))?
            .clone();
        let metadata = serde_json::to_value(self)
            .map_err(|error| HostError::InvalidWorldMetadata(error.to_string()))?;
        document.insert("metadata".to_string(), metadata);
        Ok(WorldManifest::new(serde_json::Value::Object(document)))
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) const fn cover(&self) -> Option<&AssetRef> {
        self.cover.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_validate_and_round_trip_manifest_metadata() -> anyhow::Result<()> {
        let metadata =
            WorldMetadata::new("  My World  ", Some(" A quiet place ".to_string()), None)?;
        let original = WorldManifest::new(serde_json::json!({"custom": {"keep": true}}));
        let manifest = metadata.write_to_manifest(&original)?;
        assert_eq!(WorldMetadata::from_manifest(&manifest)?, metadata);
        assert_eq!(metadata.title(), "My World");
        assert_eq!(metadata.description(), Some("A quiet place"));
        assert_eq!(manifest.as_value()["custom"]["keep"], true);
        Ok(())
    }

    #[test]
    fn test_should_reject_invalid_metadata() {
        assert!(matches!(
            WorldMetadata::new("   ", None, None),
            Err(HostError::InvalidWorldTitle { .. })
        ));
        assert!(matches!(
            WorldMetadata::new("x".repeat(MAX_WORLD_TITLE_BYTES + 1), None, None),
            Err(HostError::InvalidWorldTitle { .. })
        ));
        assert!(matches!(
            WorldMetadata::new(
                "Valid",
                Some("x".repeat(MAX_WORLD_DESCRIPTION_BYTES + 1)),
                None
            ),
            Err(HostError::InvalidWorldDescription { .. })
        ));
        assert!(matches!(
            WorldMetadata::from_manifest(&WorldManifest::default()),
            Err(HostError::InvalidWorldMetadata(_))
        ));
    }
}
