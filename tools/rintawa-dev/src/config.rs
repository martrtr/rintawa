use std::path::PathBuf;

use serde::Deserialize;

/// Optional developer configuration file.
pub const DEV_CONFIG_FILE: &str = "rintawa-dev.toml";

fn default_artifact_root() -> PathBuf {
    PathBuf::from(".")
}

/// Local build and watch settings for an RTW source tree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevConfig {
    /// Configuration schema version.
    pub schema: u32,
    /// RTW root produced by the build command.
    #[serde(default = "default_artifact_root", rename = "artifact-root")]
    pub artifact_root: PathBuf,
    /// Build command as program and arguments.
    #[serde(default)]
    pub build: Option<Vec<String>>,
    /// Gitignore-style patterns excluded from source change detection.
    #[serde(default, rename = "watch-ignore")]
    pub watch_ignore_patterns: Vec<String>,
}
