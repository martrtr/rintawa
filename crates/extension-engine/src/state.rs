//! Global state persistence for installed extensions.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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

    /// Saves state configuration back to a file.
    pub fn save_to_file(&self, path: &Path) -> EngineResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
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
