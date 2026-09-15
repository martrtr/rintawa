use std::{fs, path::Path};

use rintawa_artifacts::{ArtifactStore, RtwLimits, pack_directory};
use rintawa_extension_engine::{
    EngineError, EngineResult, ExtensionEngine, ExtensionState, RtwExtensionLoader,
};
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

const TEST_WASM_COMPONENT: &str = include_str!("fixtures/stateful_component.wat");

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
target = "native"
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
            if component_id == "runtime" && target == "native"
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
id = "optional-web"
name = "Optional Web"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "web-ui"
kind = "ui"
target = "web"
required = false
entry = "web/index.html"
"#,
    )?;
    let (store, digest) = import_source(&source, temp.path())?;
    let instance_id = ExtensionInstanceId::new("optional-web-instance");
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
