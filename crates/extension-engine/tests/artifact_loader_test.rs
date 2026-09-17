use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use rintawa_artifacts::{ArtifactStore, RtwLimits, pack_directory};
use rintawa_extension_engine::{
    EngineError, EngineResult, ExtensionEngine, ExtensionState, RtwComponentHost,
    RtwComponentHostResult, RtwComponentSource, RtwExtensionLoader,
};
use rintawa_sdk::{
    contracts::ComponentRef,
    manifest::{ComponentDescriptor, ExtensionManifest},
    traits::Component,
    types::{ComponentId, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};

struct TestComponentHost {
    observed: Arc<Mutex<Vec<u8>>>,
}

struct TestHostedComponent {
    id: ComponentId,
}

impl Component for TestHostedComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }
}

impl RtwComponentHost for TestComponentHost {
    fn target(&self) -> &str {
        "test.runtime@1"
    }

    fn load_component(
        &self,
        source: &mut RtwComponentSource<'_>,
        descriptor: &ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        let entry = descriptor.entry.as_deref().ok_or_else(|| {
            rintawa_extension_engine::RtwComponentHostError::InvalidDescriptor(String::from(
                "entry is required",
            ))
        })?;
        let path = source.resolve_component_entry(entry)?;
        let bytes = source.read(&path)?;
        *self.observed.lock().expect("test mutex should be valid") = bytes;
        Ok(Box::new(TestHostedComponent {
            id: descriptor.id.clone(),
        }))
    }
}

const TEST_WASM_COMPONENT: &str = include_str!("fixtures/stateful_component.wat");
const TEST_TARGET_PROVIDER_COMPONENT: &[u8] =
    include_bytes!("fixtures/target-provider/component.wasm");

fn register_active_target_owner(
    engine: &mut ExtensionEngine,
    instance: &str,
    component: &str,
) -> EngineResult<ComponentRef> {
    let instance_id = ExtensionInstanceId::new(instance);
    engine.register_extension_instance(
        instance_id.clone(),
        RuntimeScopeId::new("bootstrap"),
        ExtensionManifest {
            id: ExtensionId::new(format!("{instance}.extension")),
            name: format!("{instance} extension"),
            version: String::from("0.1.0"),
            sdk: String::from("^0.0"),
            components: Vec::new(),
        },
        vec![Box::new(TestHostedComponent {
            id: ComponentId::new(component),
        })],
    )?;
    engine.start_extension_instance(&instance_id)?;
    Ok(ComponentRef::new(instance_id, component))
}

fn write_rtw_source(root: &Path, content: &str, manifest: &[u8]) -> EngineResult<()> {
    fs::create_dir_all(root)?;
    fs::write(
        root.join("rtw.toml"),
        format!("format = 1\ncontent = \"{content}\"\nentry = \"manifest.toml\"\n"),
    )?;
    fs::write(root.join("manifest.toml"), manifest)?;
    Ok(())
}

fn write_wasm_extension_source(root: &Path, content: &str) -> EngineResult<()> {
    write_rtw_source(
        root,
        content,
        br#"
id = "e2e-extension"
name = "E2E Extension"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "runtime.wasm"
"#,
    )?;
    fs::write(root.join("runtime.wasm"), TEST_WASM_COMPONENT)?;
    Ok(())
}

fn import_source(
    source: &Path,
    root: &Path,
) -> EngineResult<(ArtifactStore, rintawa_artifacts::ArtifactDigest)> {
    let artifact = root.join("package.rtw");
    pack_directory(source, &artifact, RtwLimits::default())?;
    let store = ArtifactStore::open(root.join("store"), RtwLimits::default())?;
    let digest = store.import(&artifact)?.digest().clone();
    Ok((store, digest))
}

fn load_wasm_target_provider(
    engine: &mut ExtensionEngine,
    root: &Path,
    instance: &str,
) -> EngineResult<ExtensionInstanceId> {
    let source = root.join(format!("{instance}-source"));
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "wasm-target-provider"
name = "WASM Target Provider"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "provider.wasm"
"#,
    )?;
    fs::write(source.join("provider.wasm"), TEST_TARGET_PROVIDER_COMPONENT)?;
    let artifacts = root.join(format!("{instance}-artifacts"));
    fs::create_dir_all(&artifacts)?;
    let (store, digest) = import_source(&source, &artifacts)?;
    let instance_id = ExtensionInstanceId::new(instance);
    RtwExtensionLoader::new().load_stored_extension(
        engine,
        &store,
        &digest,
        instance_id.clone(),
        RuntimeScopeId::new("baseline"),
    )?;
    Ok(instance_id)
}

#[test]
fn test_should_run_stored_rtw_extension_through_full_lifecycle() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_wasm_extension_source(&source, "rintawa.extension@1")?;
    let (store, digest) = import_source(&source, temp.path())?;

    let mut engine = ExtensionEngine::new();
    let loader = RtwExtensionLoader::new();
    let instance_id = ExtensionInstanceId::new("e2e-instance");

    let extension_id = loader.load_stored_extension(
        &mut engine,
        &store,
        &digest,
        instance_id.clone(),
        RuntimeScopeId::new("e2e-scope"),
    )?;
    assert_eq!(extension_id.as_str(), "e2e-extension");
    assert_eq!(
        engine.extension_instance_state(&instance_id),
        Some(ExtensionState::Registered)
    );

    engine.start_extension_instance(&instance_id)?;
    assert_eq!(
        engine.extension_instance_state(&instance_id),
        Some(ExtensionState::Active)
    );
    engine.stop_extension_instance(&instance_id)?;
    assert_eq!(
        engine.extension_instance_state(&instance_id),
        Some(ExtensionState::Stopped)
    );
    engine.unregister_extension_instance(&instance_id)?;
    assert_eq!(engine.extension_instance_state(&instance_id), None);
    Ok(())
}

#[test]
fn test_should_reject_non_extension_rtw_content() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_wasm_extension_source(&source, "rtwkit.pack@1")?;
    let (store, digest) = import_source(&source, temp.path())?;
    let mut engine = ExtensionEngine::new();
    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            ExtensionInstanceId::new("wrong-content"),
            RuntimeScopeId::new("default"),
        )
        .expect_err("non-extension RTW content must not be loaded as an extension");

    assert!(matches!(
        error,
        EngineError::UnsupportedExtensionArtifactContent(content) if content == "rtwkit.pack@1"
    ));
    Ok(())
}

#[test]
fn test_should_reject_unsupported_required_component_target() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "native-only"
name = "Native Only"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "example.runtime.native@1"
required = true
"#,
    )?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("native-only-instance");
    let mut engine = ExtensionEngine::new();

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            instance_id.clone(),
            RuntimeScopeId::new("default"),
        )
        .expect_err("required unsupported target must reject the artifact");

    assert!(matches!(
        error,
        EngineError::UnsupportedRequiredComponentTarget { component_id, target }
            if component_id == "runtime" && target == "example.runtime.native@1"
    ));
    assert_eq!(engine.extension_instance_state(&instance_id), None);
    Ok(())
}

#[test]
fn test_should_allow_unsupported_optional_component_target_as_metadata() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "optional-target"
name = "Optional Target"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "optional-ui"
kind = "ui"
target = "example.runtime.ui@1"
required = false
entry = "web/index.html"
"#,
    )?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("optional-target-instance");
    let mut engine = ExtensionEngine::new();

    RtwExtensionLoader::new().load_stored_extension(
        &mut engine,
        &store,
        &digest,
        instance_id.clone(),
        RuntimeScopeId::new("default"),
    )?;
    assert_eq!(
        engine.extension_instance_state(&instance_id),
        Some(ExtensionState::Registered)
    );
    engine.start_extension_instance(&instance_id)?;
    engine.unregister_extension_instance(&instance_id)?;
    Ok(())
}

#[test]
fn test_should_reject_missing_wasm_component_entry() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "missing-wasm"
name = "Missing WASM"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "missing.wasm"
"#,
    )?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("missing-wasm-instance");
    let mut engine = ExtensionEngine::new();

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            instance_id.clone(),
            RuntimeScopeId::new("default"),
        )
        .expect_err("declared WASM entry must exist in the artifact");

    assert!(matches!(
        error,
        EngineError::Artifact(rintawa_artifacts::RtwError::EntryNotFound(path))
            if path == "missing.wasm"
    ));
    assert_eq!(engine.extension_instance_state(&instance_id), None);
    Ok(())
}

#[test]
fn test_should_reject_non_utf8_extension_manifest() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(&source, "rintawa.extension@1", &[0xff, 0xfe, 0xfd])?;
    let (store, digest) = import_source(&source, temp.path())?;
    let mut engine = ExtensionEngine::new();

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            ExtensionInstanceId::new("invalid-manifest"),
            RuntimeScopeId::new("default"),
        )
        .expect_err("extension manifest must be UTF-8");

    assert!(matches!(
        error,
        EngineError::ExtensionManifestEncoding(path) if path == "manifest.toml"
    ));
    Ok(())
}

#[test]
fn test_should_reject_component_entry_path_traversal() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "path-traversal"
name = "Path Traversal"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "../escape.wasm"
"#,
    )?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("path-traversal-instance");
    let mut engine = ExtensionEngine::new();

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            instance_id.clone(),
            RuntimeScopeId::new("default"),
        )
        .expect_err("component entry traversal must be rejected");

    assert!(matches!(
        error,
        EngineError::Artifact(rintawa_artifacts::RtwError::InvalidPath { path, .. })
            if path == "../escape.wasm"
    ));
    assert_eq!(engine.extension_instance_state(&instance_id), None);
    Ok(())
}

#[test]
fn test_should_resolve_component_entry_relative_to_nested_manifest() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let package = source.join("extension");
    fs::create_dir_all(&package)?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"extension/manifest.toml\"\n",
    )?;
    fs::write(
        package.join("manifest.toml"),
        br#"
id = "nested-extension"
name = "Nested Extension"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "runtime.wasm"
"#,
    )?;
    fs::write(package.join("runtime.wasm"), TEST_WASM_COMPONENT)?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("nested-instance");
    let mut engine = ExtensionEngine::new();

    let extension_id = RtwExtensionLoader::new().load_stored_extension(
        &mut engine,
        &store,
        &digest,
        instance_id.clone(),
        RuntimeScopeId::new("nested-scope"),
    )?;

    assert_eq!(extension_id.as_str(), "nested-extension");
    engine.start_extension_instance(&instance_id)?;
    engine.unregister_extension_instance(&instance_id)?;
    Ok(())
}

#[test]
fn test_should_reject_extension_manifest_above_loader_limit() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let oversized_manifest = vec![b' '; 1024 * 1024 + 1];
    write_rtw_source(&source, "rintawa.extension@1", &oversized_manifest)?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("oversized-manifest");
    let mut engine = ExtensionEngine::new();

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &store,
            &digest,
            instance_id.clone(),
            RuntimeScopeId::new("default"),
        )
        .expect_err("oversized extension manifest must be rejected before parsing");

    assert!(matches!(
        error,
        EngineError::ExtensionManifestTooLarge { path, actual, maximum }
            if path == "manifest.toml" && actual == 1024 * 1024 + 1 && maximum == 1024 * 1024
    ));
    assert_eq!(engine.extension_instance_state(&instance_id), None);
    Ok(())
}

#[test]
fn test_should_publish_wasm_execution_target_and_load_dependent_through_bounded_artifact()
-> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let mut engine = ExtensionEngine::new();
    let provider_instance =
        load_wasm_target_provider(&mut engine, temp.path(), "wasm-target-provider-instance")?;
    engine.start_extension_instance(&provider_instance)?;

    assert_eq!(
        engine.execution_target_owner("test.wasm-target@1"),
        Some(ComponentRef::new(provider_instance.clone(), "runtime"))
    );

    let dependent_source = temp.path().join("dependent-source");
    write_rtw_source(
        &dependent_source,
        "rintawa.extension@1",
        br#"
id = "wasm-target-dependent"
name = "WASM Target Dependent"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "hosted"
kind = "runtime"
target = "test.wasm-target@1"
entry = "payload.bin"
"#,
    )?;
    fs::write(dependent_source.join("payload.bin"), b"hosted payload")?;
    let dependent_artifacts = temp.path().join("dependent-artifacts");
    fs::create_dir_all(&dependent_artifacts)?;
    let (dependent_store, dependent_digest) =
        import_source(&dependent_source, &dependent_artifacts)?;
    let dependent_instance = ExtensionInstanceId::new("wasm-target-dependent-instance");

    RtwExtensionLoader::new().load_stored_extension(
        &mut engine,
        &dependent_store,
        &dependent_digest,
        dependent_instance.clone(),
        RuntimeScopeId::new("baseline"),
    )?;
    engine.start_extension_instance(&dependent_instance)?;
    assert_eq!(
        engine.extension_instance_state(&dependent_instance),
        Some(ExtensionState::Active)
    );

    engine.unregister_extension_instance(&dependent_instance)?;
    engine.stop_extension_instance(&provider_instance)?;
    assert_eq!(engine.execution_target_owner("test.wasm-target@1"), None);
    engine.unregister_extension_instance(&provider_instance)?;
    Ok(())
}

#[test]
fn test_should_preserve_existing_target_when_wasm_provider_publication_conflicts()
-> EngineResult<()> {
    struct ExistingTargetHost;

    impl RtwComponentHost for ExistingTargetHost {
        fn target(&self) -> &str {
            "test.wasm-target@1"
        }

        fn load_component(
            &self,
            _source: &mut RtwComponentSource<'_>,
            _descriptor: &ComponentDescriptor,
        ) -> RtwComponentHostResult<Box<dyn Component>> {
            unreachable!("conflicting provider must fail before component loading")
        }
    }

    let temp = tempfile::tempdir()?;
    let mut engine = ExtensionEngine::new();
    let existing_owner =
        register_active_target_owner(&mut engine, "existing-target-provider", "runtime")?;
    engine.register_execution_target_host(&existing_owner, Arc::new(ExistingTargetHost))?;

    let provider_instance =
        load_wasm_target_provider(&mut engine, temp.path(), "conflicting-wasm-provider")?;
    let error = engine
        .start_extension_instance(&provider_instance)
        .expect_err("duplicate target publication must fail provider startup");

    assert!(matches!(
        error,
        EngineError::LifecycleFailed {
            component_id,
            reason,
            ..
        } if component_id == "runtime" && reason.contains("could not publish execution target")
    ));
    assert_eq!(
        engine.execution_target_owner("test.wasm-target@1"),
        Some(existing_owner.clone())
    );
    assert_eq!(
        engine.extension_instance_state(&provider_instance),
        Some(ExtensionState::Registered)
    );

    engine.unregister_extension_instance(&provider_instance)?;
    engine.stop_extension_instance(&existing_owner.instance_id)?;
    engine.unregister_extension_instance(&existing_owner.instance_id)?;
    Ok(())
}

#[test]
fn test_should_reject_wasm_target_artifact_read_above_runtime_budget() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let mut engine = ExtensionEngine::new();
    let provider_instance =
        load_wasm_target_provider(&mut engine, temp.path(), "bounded-read-provider")?;
    engine.start_extension_instance(&provider_instance)?;

    let dependent_source = temp.path().join("oversized-dependent-source");
    write_rtw_source(
        &dependent_source,
        "rintawa.extension@1",
        br#"
id = "oversized-target-dependent"
name = "Oversized Target Dependent"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "hosted"
kind = "runtime"
target = "test.wasm-target@1"
entry = "payload.bin"
"#,
    )?;
    fs::write(
        dependent_source.join("payload.bin"),
        vec![b'x'; 8 * 1024 * 1024 + 1],
    )?;
    let dependent_artifacts = temp.path().join("oversized-dependent-artifacts");
    fs::create_dir_all(&dependent_artifacts)?;
    let (dependent_store, dependent_digest) =
        import_source(&dependent_source, &dependent_artifacts)?;
    let dependent_instance = ExtensionInstanceId::new("oversized-target-dependent-instance");

    let error = RtwExtensionLoader::new()
        .load_stored_extension(
            &mut engine,
            &dependent_store,
            &dependent_digest,
            dependent_instance.clone(),
            RuntimeScopeId::new("baseline"),
        )
        .expect_err("target provider must not read an artifact entry above its WASM budget");
    assert!(matches!(
        error,
        EngineError::ComponentHostFailed {
            component_id,
            target,
            ..
        } if component_id == "hosted" && target == "test.wasm-target@1"
    ));
    assert_eq!(engine.extension_instance_state(&dependent_instance), None);
    assert_eq!(
        engine.execution_target_owner("test.wasm-target@1"),
        Some(ComponentRef::new(provider_instance.clone(), "runtime"))
    );

    engine.stop_extension_instance(&provider_instance)?;
    engine.unregister_extension_instance(&provider_instance)?;
    Ok(())
}

#[test]
fn test_should_load_required_component_through_engine_owned_target_registry() -> EngineResult<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        br#"
id = "hosted-extension"
name = "Hosted Extension"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "hosted"
kind = "runtime"
target = "test.runtime@1"
entry = "payload.bin"
"#,
    )?;
    fs::write(source.join("payload.bin"), b"hosted payload")?;
    let (store, digest) = import_source(&source, temp.path())?;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let instance_id = ExtensionInstanceId::new("hosted-instance");
    let mut engine = ExtensionEngine::new();
    let owner = register_active_target_owner(&mut engine, "target-provider", "runtime")?;
    engine.register_execution_target_host(
        &owner,
        Arc::new(TestComponentHost {
            observed: observed.clone(),
        }),
    )?;

    assert_eq!(
        engine.execution_target_owner("test.runtime@1"),
        Some(owner.clone())
    );
    RtwExtensionLoader::new().load_stored_extension(
        &mut engine,
        &store,
        &digest,
        instance_id.clone(),
        RuntimeScopeId::new("hosted-scope"),
    )?;
    engine.start_extension_instance(&instance_id)?;

    assert_eq!(
        observed
            .lock()
            .expect("test mutex should be valid")
            .as_slice(),
        b"hosted payload"
    );
    assert_eq!(
        engine.extension_instance_state(&instance_id),
        Some(ExtensionState::Active)
    );

    let unregister_error = engine
        .unregister_extension_instance(&owner.instance_id)
        .expect_err("target provider must not unregister while a dependent remains registered");
    assert!(matches!(
        unregister_error,
        EngineError::ExecutionTargetProviderInUse {
            provider_instance_id,
            dependents,
        } if provider_instance_id == "target-provider"
            && dependents == vec![String::from("hosted-instance")]
    ));
    assert_eq!(
        engine.extension_instance_state(&owner.instance_id),
        Some(ExtensionState::Active)
    );
    assert_eq!(
        engine.execution_target_owner("test.runtime@1"),
        Some(owner.clone())
    );

    let error = engine
        .stop_extension_instance(&owner.instance_id)
        .expect_err("target provider must not stop while a dependent remains registered");
    assert!(matches!(
        error,
        EngineError::ExecutionTargetProviderInUse {
            provider_instance_id,
            dependents,
        } if provider_instance_id == "target-provider"
            && dependents == vec![String::from("hosted-instance")]
    ));

    engine.unregister_extension_instance(&instance_id)?;
    engine.stop_extension_instance(&owner.instance_id)?;
    assert_eq!(engine.execution_target_owner("test.runtime@1"), None);
    engine.unregister_extension_instance(&owner.instance_id)?;
    Ok(())
}

#[test]
fn test_should_reject_component_host_for_reserved_wasm_target() {
    struct ReservedHost;

    impl RtwComponentHost for ReservedHost {
        fn target(&self) -> &str {
            "rintawa.runtime.wasm-component@1"
        }

        fn load_component(
            &self,
            _source: &mut RtwComponentSource<'_>,
            _descriptor: &ComponentDescriptor,
        ) -> RtwComponentHostResult<Box<dyn Component>> {
            unreachable!("reserved host must be rejected before use")
        }
    }

    let mut engine = ExtensionEngine::new();
    let owner = register_active_target_owner(&mut engine, "reserved-provider", "runtime")
        .expect("test target owner should start");
    let error = engine
        .register_execution_target_host(&owner, Arc::new(ReservedHost))
        .expect_err("built-in WASM target must not be replaceable");
    assert!(matches!(
        error,
        EngineError::DuplicateComponentHostTarget(target)
            if target == "rintawa.runtime.wasm-component@1"
    ));
}

#[test]
fn test_should_reject_noncanonical_component_host_target() {
    struct NonCanonicalHost;

    impl RtwComponentHost for NonCanonicalHost {
        fn target(&self) -> &str {
            " test.runtime@1 "
        }

        fn load_component(
            &self,
            _source: &mut RtwComponentSource<'_>,
            _descriptor: &ComponentDescriptor,
        ) -> RtwComponentHostResult<Box<dyn Component>> {
            unreachable!("invalid host target must be rejected before use")
        }
    }

    let mut engine = ExtensionEngine::new();
    let owner = register_active_target_owner(&mut engine, "invalid-provider", "runtime")
        .expect("test target owner should start");
    let error = engine
        .register_execution_target_host(&owner, Arc::new(NonCanonicalHost))
        .expect_err("target identifiers must not be normalized implicitly");
    assert!(matches!(error, EngineError::InvalidComponentHostTarget));
}

#[test]
fn test_should_reject_execution_target_registration_from_inactive_owner() -> EngineResult<()> {
    let mut engine = ExtensionEngine::new();
    let instance_id = ExtensionInstanceId::new("inactive-provider");
    engine.register_extension_instance(
        instance_id.clone(),
        RuntimeScopeId::new("bootstrap"),
        ExtensionManifest {
            id: ExtensionId::new("inactive-provider.extension"),
            name: String::from("Inactive Provider"),
            version: String::from("0.1.0"),
            sdk: String::from("^0.0"),
            components: Vec::new(),
        },
        vec![Box::new(TestHostedComponent {
            id: ComponentId::new("runtime"),
        })],
    )?;
    let owner = ComponentRef::new(instance_id, "runtime");
    let error = engine
        .register_execution_target_host(
            &owner,
            Arc::new(TestComponentHost {
                observed: Arc::new(Mutex::new(Vec::new())),
            }),
        )
        .expect_err("registered but inactive components must not publish execution targets");
    assert!(matches!(
        error,
        EngineError::ExecutionTargetOwnerInactive {
            instance_id,
            component_id,
        } if instance_id == "inactive-provider" && component_id == "runtime"
    ));
    Ok(())
}

#[test]
fn test_should_reject_duplicate_execution_target_from_another_owner() -> EngineResult<()> {
    let mut engine = ExtensionEngine::new();
    let first = register_active_target_owner(&mut engine, "first-provider", "runtime")?;
    let second = register_active_target_owner(&mut engine, "second-provider", "runtime")?;
    engine.register_execution_target_host(
        &first,
        Arc::new(TestComponentHost {
            observed: Arc::new(Mutex::new(Vec::new())),
        }),
    )?;

    let error = engine
        .register_execution_target_host(
            &second,
            Arc::new(TestComponentHost {
                observed: Arc::new(Mutex::new(Vec::new())),
            }),
        )
        .expect_err("one execution target must have one process-global owner");
    assert!(matches!(
        error,
        EngineError::DuplicateComponentHostTarget(target) if target == "test.runtime@1"
    ));
    assert_eq!(engine.execution_target_owner("test.runtime@1"), Some(first));
    Ok(())
}
