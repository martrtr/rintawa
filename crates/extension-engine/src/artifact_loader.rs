//! Canonical loading of `rintawa.extension@1` content from stored RTW artifacts.

use rintawa_artifacts::{ArtifactDigest, ArtifactPath, ArtifactStore, RtwArchive, RtwError};
use rintawa_sdk::{
    manifest::{ExtensionManifest, WASM_COMPONENT_TARGET_V1},
    traits::Component,
    types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};

use crate::{
    engine::ExtensionEngine,
    errors::{EngineError, EngineResult},
};

const EXTENSION_CONTENT_ID: &str = "rintawa.extension";
const EXTENSION_CONTENT_MAJOR: u32 = 1;
const DEFAULT_WASM_ENTRY: &str = "runtime.wasm";
const MAX_EXTENSION_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Loads built-in RTW extension content into the Extension Engine.
///
/// The canonical path starts from an exact artifact in [`ArtifactStore`]. The
/// loader reads component bytes directly from the validated ZIP and never
/// extracts package files into a mutable extension directory. Repository
/// discovery, version selection, updates, and development-source policy remain
/// outside the Extension Engine.
#[derive(Debug, Default, Clone, Copy)]
pub struct RtwExtensionLoader;

impl RtwExtensionLoader {
    /// Creates the built-in RTW extension loader.
    pub const fn new() -> Self {
        Self
    }

    /// Opens one exact stored artifact and registers its extension instance.
    ///
    /// A WASM runtime is derived from the same `engine` supplied for
    /// registration so secrets, services, UI, and lifecycle policy cannot be
    /// accidentally split across independent runtime topologies. Unsupported
    /// optional component targets remain declarative manifest metadata; an
    /// unsupported required target rejects the load.
    ///
    /// This method registers the instance but does not start it. The host may
    /// grant requested capabilities or validate composition before calling
    /// [`ExtensionEngine::start_extension_instance`].
    ///
    /// # Errors
    ///
    /// Returns an artifact validation error, unsupported-content or required-
    /// target error, manifest error, WASM loading error, or extension
    /// registration error.
    pub fn load_stored_extension(
        &self,
        engine: &mut ExtensionEngine,
        store: &ArtifactStore,
        digest: &ArtifactDigest,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> EngineResult<ExtensionId> {
        let archive = store.open_artifact(digest)?;
        self.load_archive(engine, archive, instance_id, scope_id)
    }

    fn load_archive(
        &self,
        engine: &mut ExtensionEngine,
        mut archive: RtwArchive,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> EngineResult<ExtensionId> {
        let content = &archive.manifest().content;
        if content.id() != EXTENSION_CONTENT_ID || content.major() != EXTENSION_CONTENT_MAJOR {
            return Err(EngineError::UnsupportedExtensionArtifactContent(
                content.to_string(),
            ));
        }

        let manifest_path = archive.manifest().entry.clone();
        let manifest_size = archive
            .entries()
            .find(|entry| entry.path == manifest_path)
            .map(|entry| entry.uncompressed_size)
            .ok_or_else(|| RtwError::EntryNotFound(manifest_path.to_string()))?;
        if manifest_size > MAX_EXTENSION_MANIFEST_BYTES {
            return Err(EngineError::ExtensionManifestTooLarge {
                path: manifest_path.to_string(),
                actual: manifest_size,
                maximum: MAX_EXTENSION_MANIFEST_BYTES,
            });
        }
        let manifest_bytes = archive.read(&manifest_path)?;
        let raw_manifest = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| EngineError::ExtensionManifestEncoding(manifest_path.to_string()))?;
        let manifest: ExtensionManifest = engine.parse_manifest(raw_manifest)?;
        let extension_id = manifest.id.clone();

        for descriptor in &manifest.components {
            if descriptor.required && descriptor.target.as_str() != WASM_COMPONENT_TARGET_V1 {
                return Err(EngineError::UnsupportedRequiredComponentTarget {
                    component_id: descriptor.id.to_string(),
                    target: descriptor.target.to_string(),
                });
            }
        }

        let mut components: Vec<Box<dyn Component>> = Vec::new();
        if manifest
            .components
            .iter()
            .any(|descriptor| descriptor.target.as_str() == WASM_COMPONENT_TARGET_V1)
        {
            let wasm_engine = engine.wasm_runtime_engine()?;
            for descriptor in manifest
                .components
                .iter()
                .filter(|descriptor| descriptor.target.as_str() == WASM_COMPONENT_TARGET_V1)
            {
                let entry = descriptor.entry.as_deref().unwrap_or(DEFAULT_WASM_ENTRY);
                let artifact_path = resolve_component_entry(&manifest_path, entry)?;
                let bytes = archive.read(&artifact_path)?;
                let component =
                    wasm_engine.load_component_from_bytes(descriptor.id.clone(), &bytes)?;
                components.push(Box::new(component));
            }
        }

        engine.register_extension_instance(instance_id, scope_id, manifest, components)?;
        Ok(extension_id)
    }
}

fn resolve_component_entry(
    manifest_path: &ArtifactPath,
    entry: &str,
) -> Result<ArtifactPath, RtwError> {
    let entry = ArtifactPath::parse(entry)?;
    match manifest_path.as_str().rsplit_once('/') {
        Some((parent, _)) => ArtifactPath::parse(format!("{parent}/{}", entry.as_str())),
        None => Ok(entry),
    }
}
