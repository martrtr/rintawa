//! Directory scanner and loader for Rintawa extensions.

use std::path::{Component as PathComponent, Path};

use rintawa_sdk::{traits::Component, types::ExtensionId};
use tracing::{info, warn};

use crate::{
    engine::ExtensionEngine,
    errors::{EngineError, EngineResult},
    runtime::WasmRuntimeEngine,
    state::{ExtensionsStateConfig, STATE_FILE_NAME},
};

/// Service responsible for discovering and loading extensions from disk.
pub struct ExtensionLoader {
    wasm_engine: WasmRuntimeEngine,
}

impl ExtensionLoader {
    /// Creates a new extension loader instance.
    pub fn new(wasm_engine: WasmRuntimeEngine) -> Self {
        Self { wasm_engine }
    }

    /// Scans a directory (e.g., `rintawa/extensions`), parses manifests, and registers enabled extensions.
    pub fn load_directory(
        &self,
        engine: &mut ExtensionEngine,
        dir_path: &Path,
    ) -> EngineResult<Vec<ExtensionId>> {
        if !dir_path.exists() || !dir_path.is_dir() {
            return Err(EngineError::InvalidDirectory(
                dir_path.to_string_lossy().to_string(),
            ));
        }

        let state_path = dir_path.join(STATE_FILE_NAME);
        let state_config = ExtensionsStateConfig::load_from_file(&state_path)?;

        let mut loaded_extensions = Vec::new();
        let entries = std::fs::read_dir(dir_path)?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    warn!(
                        "Failed to read directory entry in {}: {err}",
                        dir_path.display()
                    );
                    continue;
                }
            };
            let path = entry.path();

            if path.is_dir() {
                let manifest_path = path.join("manifest.toml");
                if manifest_path.exists() {
                    match self.load_single_extension(engine, &path, &state_config) {
                        Ok(Some(id)) => loaded_extensions.push(id),
                        Ok(None) => {}
                        Err(err) => {
                            warn!(path = %path.display(), "Failed to load extension: {err}");
                        }
                    }
                }
            }
        }

        Ok(loaded_extensions)
    }

    /// Loads a single extension directory if approved by `state_config`.
    pub fn load_single_extension(
        &self,
        engine: &mut ExtensionEngine,
        ext_dir: &Path,
        state_config: &ExtensionsStateConfig,
    ) -> EngineResult<Option<ExtensionId>> {
        let manifest_path = ext_dir.join("manifest.toml");
        let raw_manifest = std::fs::read_to_string(&manifest_path)?;
        let manifest = engine.parse_manifest(&raw_manifest)?;

        let ext_id_str = manifest.id.as_str();

        if !state_config.is_enabled(ext_id_str) {
            info!(extension = %ext_id_str, "Skipping disabled extension");
            return Ok(None);
        }

        let mut components: Vec<Box<dyn Component>> = Vec::new();

        for comp_desc in &manifest.components {
            if comp_desc.target.as_str() == "wasm" {
                let wasm_rel_str = comp_desc.entry.as_deref().unwrap_or("runtime.wasm");
                let wasm_rel_path = Path::new(wasm_rel_str);

                // Path traversal protection: forbid absolute paths and parent directory components ('..')
                if wasm_rel_path.is_absolute()
                    || wasm_rel_path
                        .components()
                        .any(|c| c == PathComponent::ParentDir)
                {
                    return Err(EngineError::InvalidDirectory(format!(
                        "Path traversal attempt detected in component entry path: {wasm_rel_str}"
                    )));
                }

                let wasm_path = ext_dir.join(wasm_rel_path);
                let wasm_component = self
                    .wasm_engine
                    .load_component_from_file(comp_desc.id.clone(), &wasm_path)?;

                components.push(Box::new(wasm_component));
            }
        }

        let ext_id = manifest.id.clone();
        engine.register_extension(manifest, components)?;
        Ok(Some(ext_id))
    }
}
