//! Host-owned human-facing metadata for persistent Worlds.
//!
//! Catalog metadata intentionally lives beside the authoritative World database rather
//! than inside World Manager state so every launcher observes the same title and cover.

use std::{fs, io::Write, path::Path};

use rintawa_artifacts::AssetRef;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{HostError, HostResult};

/// Current persisted World catalog metadata schema.
pub const WORLD_CATALOG_METADATA_SCHEMA: u32 = 1;
/// Maximum UTF-8 byte length accepted for a human-facing World title.
pub const MAX_WORLD_TITLE_BYTES: usize = 256;
/// Maximum UTF-8 byte length accepted for an optional human-facing World description.
pub const MAX_WORLD_DESCRIPTION_BYTES: usize = 4 * 1024;
/// File stored inside each host-owned World directory.
pub const WORLD_CATALOG_METADATA_FILE: &str = "catalog.toml";

/// Human-facing host-owned metadata available while a World is inactive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldCatalogMetadata {
    schema: u32,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cover: Option<AssetRef>,
}

impl WorldCatalogMetadata {
    /// Creates validated schema-v1 catalog metadata.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidWorldTitle`] when the title is blank or exceeds the bound.
    pub fn new(title: impl Into<String>, cover: Option<AssetRef>) -> HostResult<Self> {
        let title = normalize_title(title.into())?;
        Ok(Self {
            schema: WORLD_CATALOG_METADATA_SCHEMA,
            title,
            description: None,
            cover,
        })
    }

    /// Returns the normalized human-facing title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the optional normalized human-facing description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the immutable cover reference when one is configured.
    pub const fn cover(&self) -> Option<&AssetRef> {
        self.cover.as_ref()
    }

    /// Replaces the human-facing title while preserving the cover.
    pub fn with_title(mut self, title: impl Into<String>) -> HostResult<Self> {
        self.title = normalize_title(title.into())?;
        Ok(self)
    }

    /// Replaces the optional description while preserving other metadata.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidWorldDescription`] when a non-empty description exceeds the bound.
    pub fn with_description(mut self, description: Option<String>) -> HostResult<Self> {
        self.description = normalize_description(description)?;
        Ok(self)
    }

    /// Replaces the immutable cover reference while preserving the title and description.
    pub fn with_cover(mut self, cover: Option<AssetRef>) -> Self {
        self.cover = cover;
        self
    }

    pub(crate) fn load(path: &Path) -> HostResult<Self> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HostError::WorldCatalogMetadataMissing(path.to_path_buf())
            } else {
                HostError::Io(error)
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(HostError::InvalidWorldCatalogMetadataFile(
                path.to_path_buf(),
            ));
        }
        let source = fs::read_to_string(path)?;
        let decoded: Self = toml::from_str(&source)
            .map_err(|source| HostError::WorldCatalogMetadataDecode { source })?;
        if decoded.schema != WORLD_CATALOG_METADATA_SCHEMA {
            return Err(HostError::UnsupportedWorldCatalogMetadataSchema(
                decoded.schema,
            ));
        }
        Self::new(decoded.title, decoded.cover)?.with_description(decoded.description)
    }

    pub(crate) fn save(&self, path: &Path) -> HostResult<()> {
        let parent = path
            .parent()
            .ok_or_else(|| HostError::InvalidWorldCatalogMetadataFile(path.to_path_buf()))?;
        if path.exists() {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(HostError::InvalidWorldCatalogMetadataFile(
                    path.to_path_buf(),
                ));
            }
        }
        let source = toml::to_string_pretty(self)
            .map_err(|source| HostError::WorldCatalogMetadataEncode { source })?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(source.as_bytes())?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

fn normalize_description(description: Option<String>) -> HostResult<Option<String>> {
    let Some(description) = description else {
        return Ok(None);
    };
    let description = description.trim().to_string();
    if description.is_empty() {
        return Ok(None);
    }
    if description.len() > MAX_WORLD_DESCRIPTION_BYTES {
        return Err(HostError::InvalidWorldDescription {
            actual_bytes: description.len(),
            maximum_bytes: MAX_WORLD_DESCRIPTION_BYTES,
        });
    }
    Ok(Some(description))
}

pub(crate) fn normalize_title(title: String) -> HostResult<String> {
    let title = title.trim().to_string();
    if title.is_empty() || title.len() > MAX_WORLD_TITLE_BYTES {
        return Err(HostError::InvalidWorldTitle {
            actual_bytes: title.len(),
            maximum_bytes: MAX_WORLD_TITLE_BYTES,
        });
    }
    Ok(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_round_trip_catalog_metadata_atomically() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(WORLD_CATALOG_METADATA_FILE);
        let expected = WorldCatalogMetadata::new("  My World  ", None)?
            .with_description(Some(String::from("  A quiet place  ")))?;
        expected.save(&path)?;
        assert_eq!(WorldCatalogMetadata::load(&path)?, expected);
        assert_eq!(expected.title(), "My World");
        assert_eq!(expected.description(), Some("A quiet place"));
        Ok(())
    }

    #[test]
    fn test_should_normalize_empty_description_and_reject_oversized_description()
    -> anyhow::Result<()> {
        let empty = WorldCatalogMetadata::new("World", None)?
            .with_description(Some(String::from("   ")))?;
        assert_eq!(empty.description(), None);
        assert!(matches!(
            WorldCatalogMetadata::new("World", None)?
                .with_description(Some("x".repeat(MAX_WORLD_DESCRIPTION_BYTES + 1))),
            Err(HostError::InvalidWorldDescription { .. })
        ));
        Ok(())
    }

    #[test]
    fn test_should_reject_blank_and_oversized_titles() {
        assert!(matches!(
            WorldCatalogMetadata::new("   ", None),
            Err(HostError::InvalidWorldTitle { .. })
        ));
        assert!(matches!(
            WorldCatalogMetadata::new("x".repeat(MAX_WORLD_TITLE_BYTES + 1), None),
            Err(HostError::InvalidWorldTitle { .. })
        ));
    }
}
