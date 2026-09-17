//! Canonical loading of `rintawa.extension@1` content from stored RTW artifacts.

use std::collections::BTreeMap;

use rintawa_artifacts::{ArtifactDigest, ArtifactStore, RtwArchive, RtwError};
use rintawa_sdk::{
    manifest::{ExtensionManifest, WASM_COMPONENT_TARGET_V1},
    traits::Component,
    types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};

use crate::{
    artifact_host::RtwComponentSource,
    engine::ExtensionEngine,
    errors::{EngineError, EngineResult},
    execution_targets::dependency_from_descriptor,
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
/// outside the Extension Engine. Non-built-in execution targets are resolved
/// from the owner-scoped registry of the supplied [`ExtensionEngine`].
#[derive(Clone, Copy, Default)]
pub struct RtwExtensionLoader;

impl RtwExtensionLoader {
    /// Creates the built-in RTW extension loader.
    pub fn new() -> Self {
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

    /// Reads and validates the extension manifest from one exact stored artifact.
    ///
    /// This performs the same content-type, size, encoding, and manifest validation
    /// used by runtime loading without registering or starting the extension.
    pub fn read_stored_manifest(
        &self,
        engine: &ExtensionEngine,
        store: &ArtifactStore,
        digest: &ArtifactDigest,
    ) -> EngineResult<ExtensionManifest> {
        let mut archive = store.open_artifact(digest)?;
        self.read_manifest(engine, &mut archive)
    }

    fn read_manifest(
        &self,
        engine: &ExtensionEngine,
        archive: &mut RtwArchive,
    ) -> EngineResult<ExtensionManifest> {
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
        engine.parse_manifest(raw_manifest)
    }

    fn load_archive(
        &self,
        engine: &mut ExtensionEngine,
        mut archive: RtwArchive,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> EngineResult<ExtensionId> {
        let manifest_path = archive.manifest().entry.clone();
        let manifest = self.read_manifest(engine, &mut archive)?;
        let extension_id = manifest.id.clone();

        let mut resolved_hosts = BTreeMap::new();
        for descriptor in &manifest.components {
            let target = descriptor.target.as_str();
            if target == WASM_COMPONENT_TARGET_V1 {
                continue;
            }
            if let Some(registered) = engine.resolve_component_host(target) {
                resolved_hosts
                    .entry(target.to_string())
                    .or_insert(registered);
            } else if descriptor.required {
                return Err(EngineError::UnsupportedRequiredComponentTarget {
                    component_id: descriptor.id.to_string(),
                    target: descriptor.target.to_string(),
                });
            }
        }

        let wasm_engine = manifest
            .components
            .iter()
            .any(|descriptor| descriptor.target.as_str() == WASM_COMPONENT_TARGET_V1)
            .then(|| engine.wasm_runtime_engine())
            .transpose()?;
        let mut components: Vec<Box<dyn Component>> = Vec::new();
        let mut execution_target_dependencies = Vec::new();

        for descriptor in &manifest.components {
            let target = descriptor.target.as_str();
            if target == WASM_COMPONENT_TARGET_V1 {
                let runtime = wasm_engine.as_ref().ok_or_else(|| {
                    EngineError::UnsupportedRequiredComponentTarget {
                        component_id: descriptor.id.to_string(),
                        target: descriptor.target.to_string(),
                    }
                })?;
                let entry = descriptor.entry.as_deref().unwrap_or(DEFAULT_WASM_ENTRY);
                let mut source = RtwComponentSource::new(&mut archive, &manifest_path);
                let artifact_path = source.resolve_component_entry(entry)?;
                let bytes = source.read(&artifact_path)?;
                let component = runtime.load_component_from_bytes(descriptor.id.clone(), &bytes)?;
                components.push(Box::new(component));
                continue;
            }

            if let Some(registered) = resolved_hosts.get(target) {
                let mut source = RtwComponentSource::new(&mut archive, &manifest_path);
                let component = registered
                    .host
                    .load_component(&mut source, descriptor)
                    .map_err(|source| EngineError::ComponentHostFailed {
                        component_id: descriptor.id.to_string(),
                        target: descriptor.target.to_string(),
                        source: Box::new(source),
                    })?;
                execution_target_dependencies.push(dependency_from_descriptor(
                    descriptor,
                    registered.owner.clone(),
                ));
                components.push(component);
            }
        }

        engine.register_extension_instance_with_target_dependencies(
            instance_id,
            scope_id,
            manifest,
            components,
            execution_target_dependencies,
        )?;
        Ok(extension_id)
    }
}
