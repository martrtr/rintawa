//! Declarative configuration for local RTW extension development projects.

use std::path::PathBuf;

use serde::Deserialize;

/// Optional developer configuration file.
pub const DEV_CONFIG_FILE: &str = "rintawa-dev.toml";

fn default_artifact_root() -> PathBuf {
    PathBuf::from(".")
}

/// One Rust guest library compiled and wrapped as a WebAssembly Component.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RustComponentBuild {
    /// Cargo manifest of the standalone guest crate, relative to the project root.
    pub manifest_path: PathBuf,
    /// Core-WASM cdylib artifact stem produced by Cargo, such as `world_manager_runtime`.
    pub artifact: String,
    /// Generated Component Model binary path relative to the configured RTW artifact root.
    pub output: PathBuf,
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
    /// Rust guest crates compiled to core WASM then wrapped as Component Model binaries.
    #[serde(default, rename = "rust-components")]
    pub rust_components: Vec<RustComponentBuild>,
    /// Gitignore-style patterns excluded from source change detection.
    #[serde(default, rename = "watch-ignore")]
    pub watch_ignore_patterns: Vec<String>,
}
