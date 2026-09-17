//! Canonical loading of `rintawa.extension@1` content from stored RTW artifacts.

use std::collections::BTreeMap;

use rintawa_artifacts::{ArtifactDigest, ArtifactStore, RtwArchive, RtwError};
use rintawa_sdk::{
    manifest::{ExtensionManifest, WASM_COMPONENT_TARGET_V1},
    traits::Component,
    types::{ComponentId, ComponentTarget, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
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

/// One required component whose execution target is not available yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredExecutionTarget {
    /// Component waiting for a target host.
    pub component_id: ComponentId,
    /// Exact versioned execution target that is currently unavailable.
    pub target: ComponentTarget,
}

/// Side-effect-free deferral returned by staged RTW extension loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredExtensionLoad {
    /// Logical extension identity read from the validated manifest.
    pub extension_id: ExtensionId,
    first_missing_required_target: DeferredExecutionTarget,
    additional_missing_required_targets: Vec<DeferredExecutionTarget>,
}

impl DeferredExtensionLoad {
    /// Returns every required execution target that is unavailable in manifest order.
    pub fn missing_required_targets(&self) -> impl Iterator<Item = &DeferredExecutionTarget> {
        std::iter::once(&self.first_missing_required_target)
            .chain(self.additional_missing_required_targets.iter())
    }
}

/// Metadata returned after one extension artifact is fully registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredExtensionLoad {
    /// Logical extension identity read from the validated manifest.
    pub extension_id: ExtensionId,
    /// Whether at least one built-in WASM component exports the target-provider ABI.
    ///
    /// This marks an activation as eligible for early bootstrap-provider scheduling;
    /// it does not guarantee that `start()` will actually publish a target.
    pub can_publish_execution_targets: bool,
}

/// Outcome of one staged attempt to load an exact RTW extension artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtwExtensionLoadOutcome {
    /// The extension instance was fully registered in the engine.
    Loaded(RegisteredExtensionLoad),
    /// Loading made no side effects because required execution targets are unavailable.
    Deferred(DeferredExtensionLoad),
}

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
        match self.try_load_stored_extension(engine, store, digest, instance_id, scope_id)? {
            RtwExtensionLoadOutcome::Loaded(loaded) => Ok(loaded.extension_id),
            RtwExtensionLoadOutcome::Deferred(deferred) => {
                let missing = deferred.first_missing_required_target;
                Err(EngineError::UnsupportedRequiredComponentTarget {
                    component_id: missing.component_id.to_string(),
                    target: missing.target.to_string(),
                })
            }
        }
    }

    /// Attempts to register one exact artifact without treating missing targets as failure.
    ///
    /// Required non-root execution targets are preflighted before any component is
    /// instantiated or registered. If one or more are unavailable, the method returns
    /// [`RtwExtensionLoadOutcome::Deferred`] and leaves `engine` unchanged. This is the
    /// loader boundary used by fixed-point bootstrap.
    ///
    /// # Errors
    ///
    /// Returns artifact, manifest, component-host, WASM, or registration failures.
    /// Missing required execution targets are represented by a deferred outcome instead.
    pub fn try_load_stored_extension(
        &self,
        engine: &mut ExtensionEngine,
        store: &ArtifactStore,
        digest: &ArtifactDigest,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> EngineResult<RtwExtensionLoadOutcome> {
        let archive = store.open_artifact(digest)?;
        self.try_load_archive(engine, archive, instance_id, scope_id)
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

    fn try_load_archive(
        &self,
        engine: &mut ExtensionEngine,
        mut archive: RtwArchive,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> EngineResult<RtwExtensionLoadOutcome> {
        let manifest_path = archive.manifest().entry.clone();
        let manifest = self.read_manifest(engine, &mut archive)?;
        let extension_id = manifest.id.clone();

        let mut resolved_hosts = BTreeMap::new();
        let mut missing_required_targets = Vec::new();
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
                missing_required_targets.push(DeferredExecutionTarget {
                    component_id: descriptor.id.clone(),
                    target: descriptor.target.clone(),
                });
            }
        }
        if let Some(first_missing_required_target) = missing_required_targets.first().cloned() {
            return Ok(RtwExtensionLoadOutcome::Deferred(DeferredExtensionLoad {
                extension_id,
                first_missing_required_target,
                additional_missing_required_targets: missing_required_targets
                    .into_iter()
                    .skip(1)
                    .collect(),
            }));
        }

        let wasm_engine = manifest
            .components
            .iter()
            .any(|descriptor| descriptor.target.as_str() == WASM_COMPONENT_TARGET_V1)
            .then(|| engine.wasm_runtime_engine())
            .transpose()?;
        let mut components: Vec<Box<dyn Component>> = Vec::new();
        let mut execution_target_dependencies = Vec::new();
        let mut can_publish_execution_targets = false;

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
                can_publish_execution_targets |= component.supports_execution_target_provider();
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
        Ok(RtwExtensionLoadOutcome::Loaded(RegisteredExtensionLoad {
            extension_id,
            can_publish_execution_targets,
        }))
    }
}
