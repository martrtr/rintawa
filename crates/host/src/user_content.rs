//! Persistent generic user-content library index.

use std::{collections::HashSet, fmt, io::Write, path::Path, str::FromStr};

use rintawa_artifacts::{ArtifactDigest, ContentType};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::{HostError, HostHome, HostResult};

/// Current persistence schema of the generic user-content library index.
pub const USER_CONTENT_LIBRARY_SCHEMA: u32 = 1;

/// Stable host-local identity of one logical user-content item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserContentId(Uuid);

impl UserContentId {
    /// Allocates a time-ordered random user-content identifier.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for UserContentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for UserContentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for UserContentId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// One logical user-content item pointing at its current immutable RTW revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct UserContentEntry {
    /// Stable logical library identity retained across edits/revisions.
    pub id: UserContentId,
    /// Exact versioned RTW content type interpreted by an extension handler.
    pub content: ContentType,
    /// Current immutable RTW artifact revision in the shared artifact CAS.
    pub revision: ArtifactDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserContentLibrary {
    schema: u32,
    #[serde(default)]
    entries: Vec<UserContentEntry>,
}

impl Default for UserContentLibrary {
    fn default() -> Self {
        Self {
            schema: USER_CONTENT_LIBRARY_SCHEMA,
            entries: Vec::new(),
        }
    }
}

impl UserContentLibrary {
    fn load(path: &Path) -> HostResult<Self> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(HostError::InvalidUserContentLibraryFile(path.to_path_buf()));
        }
        let source = std::fs::read_to_string(path)?;
        let mut library: Self = toml::from_str(&source)
            .map_err(|source| HostError::UserContentLibraryDecode { source })?;
        if library.schema != USER_CONTENT_LIBRARY_SCHEMA {
            return Err(HostError::UnsupportedUserContentLibrarySchema(
                library.schema,
            ));
        }
        library.entries.sort_by_key(|entry| entry.id);
        let mut seen = HashSet::with_capacity(library.entries.len());
        if let Some(duplicate) = library.entries.iter().find(|entry| !seen.insert(entry.id)) {
            return Err(HostError::DuplicateUserContentId(duplicate.id));
        }
        Ok(library)
    }

    fn save(&self, path: &Path) -> HostResult<()> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "user-content library path has no parent",
            )
        })?;
        std::fs::create_dir_all(parent)?;
        let source = toml::to_string_pretty(self)
            .map_err(|source| HostError::UserContentLibraryEncode { source })?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(source.as_bytes())?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| HostError::Io(error.error))?;
        Ok(())
    }
}

impl HostHome {
    /// Lists logical user-content items in deterministic identifier order.
    ///
    /// # Errors
    ///
    /// Returns an I/O, decoding, schema, or duplicate-ID error if the persisted
    /// library index is malformed.
    pub fn list_user_content(&self) -> HostResult<Vec<UserContentEntry>> {
        Ok(self.load_user_content_library()?.entries)
    }

    /// Removes one logical user-content item from the index without deleting CAS bytes.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::UserContentNotFound`] when `id` is not indexed, or a
    /// persistence error if the updated library cannot be stored atomically.
    pub fn remove_user_content(&self, id: UserContentId) -> HostResult<UserContentEntry> {
        let mut library = UserContentLibrary::load(&self.user_content_path)?;
        let index = library
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(HostError::UserContentNotFound(id))?;
        let removed = library.entries.remove(index);
        library.save(&self.user_content_path)?;
        Ok(removed)
    }

    fn load_user_content_library(&self) -> HostResult<UserContentLibrary> {
        let library = UserContentLibrary::load(&self.user_content_path)?;
        for entry in &library.entries {
            self.verify_user_content_revision(entry.id, &entry.content, &entry.revision)?;
        }
        Ok(library)
    }

    pub(crate) fn indexed_user_content(&self, id: UserContentId) -> HostResult<UserContentEntry> {
        UserContentLibrary::load(&self.user_content_path)?
            .entries
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(HostError::UserContentNotFound(id))
    }

    fn verify_user_content_revision(
        &self,
        id: UserContentId,
        content: &ContentType,
        revision: &ArtifactDigest,
    ) -> HostResult<()> {
        let archive = self.store.open_artifact(revision)?;
        if archive.manifest().content != *content {
            return Err(HostError::UserContentTypeMismatch {
                id,
                expected: content.to_string(),
                actual: archive.manifest().content.to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn insert_user_content_revision(
        &self,
        content: ContentType,
        revision: ArtifactDigest,
    ) -> HostResult<UserContentEntry> {
        let mut library = UserContentLibrary::load(&self.user_content_path)?;
        let id = UserContentId::new();
        self.verify_user_content_revision(id, &content, &revision)?;
        let entry = UserContentEntry {
            id,
            content,
            revision,
        };
        library.entries.push(entry.clone());
        library.entries.sort_by_key(|entry| entry.id);
        library.save(&self.user_content_path)?;
        Ok(entry)
    }

    pub(crate) fn replace_user_content_revision(
        &self,
        id: UserContentId,
        content: ContentType,
        revision: ArtifactDigest,
    ) -> HostResult<UserContentEntry> {
        let mut library = UserContentLibrary::load(&self.user_content_path)?;
        let index = library
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(HostError::UserContentNotFound(id))?;
        if library.entries[index].content != content {
            return Err(HostError::UserContentTypeMismatch {
                id,
                expected: library.entries[index].content.to_string(),
                actual: content.to_string(),
            });
        }
        self.verify_user_content_revision(id, &content, &revision)?;
        library.entries[index].revision = revision;
        let updated = library.entries[index].clone();
        library.save(&self.user_content_path)?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn import_test_revision(
        home: &HostHome,
        root: &Path,
        name: &str,
        content: &ContentType,
    ) -> anyhow::Result<ArtifactDigest> {
        let source = root.join(format!("{name}-source"));
        std::fs::create_dir(&source)?;
        std::fs::write(
            source.join("rtw.toml"),
            format!(
                "format = 1\ncontent = \"{}\"\nentry = \"content.json\"\n",
                content
            ),
        )?;
        std::fs::write(
            source.join("content.json"),
            format!(r#"{{"revision":"{name}"}}"#),
        )?;
        let output = root.join(format!("{name}.rtw"));
        rintawa_artifacts::pack_directory(
            &source,
            &output,
            rintawa_artifacts::RtwLimits::default(),
        )?;
        Ok(home.artifact_store().import(&output)?.digest().clone())
    }

    #[test]
    fn test_should_persist_revision_without_changing_logical_content_id() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let content = ContentType::parse("rintawa.test-content@1")?;
        let first_revision = import_test_revision(&home, root.path(), "revision-1", &content)?;
        let second_revision = import_test_revision(&home, root.path(), "revision-2", &content)?;
        let first = home.insert_user_content_revision(content.clone(), first_revision)?;
        let second = home.replace_user_content_revision(first.id, content, second_revision)?;

        assert_eq!(first.id, second.id);
        assert_ne!(first.revision, second.revision);
        let persisted = UserContentLibrary::load(&home.user_content_path)?;
        assert_eq!(persisted.entries, vec![second]);
        Ok(())
    }

    #[test]
    fn test_should_fail_closed_on_corrupt_revision_but_allow_repair() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let content = ContentType::parse("rintawa.test-content@1")?;
        let first_revision = import_test_revision(&home, root.path(), "corrupt-me", &content)?;
        let repair_revision = import_test_revision(&home, root.path(), "repair", &content)?;
        let entry = home.insert_user_content_revision(content.clone(), first_revision.clone())?;

        let stored = home
            .artifact_store()
            .root()
            .join("sha256")
            .join(format!("{}.rtw", first_revision.hex()));
        std::fs::write(stored, b"corrupted")?;
        assert!(matches!(
            home.list_user_content(),
            Err(HostError::Artifact(_))
        ));

        let repaired = home.replace_user_content_revision(entry.id, content, repair_revision)?;
        assert_eq!(repaired.id, entry.id);
        assert_eq!(home.list_user_content()?, vec![repaired]);
        Ok(())
    }

    #[test]
    fn test_should_reject_content_type_change_for_existing_library_item() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let home = HostHome::open(root.path().join("home"))?;
        let first_content = ContentType::parse("rintawa.first@1")?;
        let second_content = ContentType::parse("rintawa.second@1")?;
        let first_revision = import_test_revision(&home, root.path(), "first", &first_content)?;
        let second_revision = import_test_revision(&home, root.path(), "second", &second_content)?;
        let entry = home.insert_user_content_revision(first_content, first_revision)?;

        assert!(matches!(
            home.replace_user_content_revision(entry.id, second_content, second_revision),
            Err(HostError::UserContentTypeMismatch { .. })
        ));
        Ok(())
    }
}
