//! Global state persistence for installed extensions.

use std::{
    collections::HashMap,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::errors::EngineResult;

/// Persistent file name for extension state inside the extensions directory.
pub const STATE_FILE_NAME: &str = "state.toml";

/// Audit record tracking status and modifications of an extension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionStateRecord {
    /// Indicates whether the extension is enabled by the user or host.
    pub enabled: bool,
    /// ISO-8601 timestamp of the last modification.
    pub updated_at: String,
    /// Actor (user, system, or updater) that performed the last state change.
    pub updated_by: String,
}

impl Default for ExtensionStateRecord {
    fn default() -> Self {
        Self {
            enabled: true,
            updated_at: String::new(),
            updated_by: "system".to_string(),
        }
    }
}

/// Persistent configuration stored in `extensions/state.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionsStateConfig {
    /// Map of extension IDs to their audit records.
    #[serde(default)]
    pub extensions: HashMap<String, ExtensionStateRecord>,
}

fn persistence_parent(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn persist_atomically_with(
    path: &Path,
    content: &str,
    commit: impl FnOnce(NamedTempFile, &Path) -> io::Result<()>,
) -> EngineResult<()> {
    let parent = persistence_parent(path);
    std::fs::create_dir_all(&parent)?;
    let mut temporary = NamedTempFile::new_in(&parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file_mut().sync_all()?;
    commit(temporary, path)?;
    Ok(())
}

impl ExtensionsStateConfig {
    /// Loads state configuration from a file, returning a default empty config if missing.
    pub fn load_from_file(path: &Path) -> EngineResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Saves state configuration atomically.
    ///
    /// The replacement file is fully written and synced before it becomes
    /// visible at `path`, so an interrupted write cannot leave a truncated
    /// `state.toml`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or serialization error when the state cannot be persisted.
    pub fn save_to_file(&self, path: &Path) -> EngineResult<()> {
        let content = toml::to_string_pretty(self)?;
        persist_atomically_with(path, &content, |temporary, destination| {
            temporary
                .persist(destination)
                .map_err(|error| error.error)?;
            Ok(())
        })
    }

    /// Returns `true` if an extension is enabled or untracked.
    pub fn is_enabled(&self, extension_id: &str) -> bool {
        self.extensions
            .get(extension_id)
            .is_none_or(|record| record.enabled)
    }

    /// Sets the enabled state of an extension with audit details.
    pub fn set_enabled(&mut self, extension_id: &str, enabled: bool, actor: &str, timestamp: &str) {
        self.extensions.insert(
            extension_id.to_string(),
            ExtensionStateRecord {
                enabled,
                updated_at: timestamp.to_string(),
                updated_by: actor.to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_state_write_replaces_complete_document() -> EngineResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(STATE_FILE_NAME);
        let mut state = ExtensionsStateConfig::default();
        state.set_enabled("alpha", false, "tester", "2026-09-20T00:00:00Z");

        state.save_to_file(&path)?;

        assert_eq!(ExtensionsStateConfig::load_from_file(&path)?, state);
        Ok(())
    }

    #[test]
    fn test_interrupted_atomic_state_write_preserves_previous_document() -> EngineResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(STATE_FILE_NAME);
        let mut original = ExtensionsStateConfig::default();
        original.set_enabled("alpha", true, "tester", "2026-09-20T00:00:00Z");
        original.save_to_file(&path)?;

        let mut replacement = original.clone();
        replacement.set_enabled("alpha", false, "tester", "2026-09-20T00:01:00Z");
        let replacement_content = toml::to_string_pretty(&replacement)?;
        let error = persist_atomically_with(&path, &replacement_content, |_temporary, _path| {
            Err(io::Error::other(
                "simulated interruption before atomic commit",
            ))
        })
        .expect_err("simulated interruption must fail");

        assert!(matches!(error, crate::EngineError::Io(_)));
        assert_eq!(ExtensionsStateConfig::load_from_file(&path)?, original);
        Ok(())
    }
}
