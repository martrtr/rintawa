use rintawa_extension_engine::{
    EngineError, EngineResult, ExtensionEngine, ExtensionLoader, ExtensionState,
    ExtensionsStateConfig, STATE_FILE_NAME,
};
use rintawa_sdk::manifest::ManifestValidationError;
use std::fs;

#[test]
fn test_extension_loader_and_state_persistence() -> EngineResult<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let ext_dir = temp_dir.path().join("rintawa/extensions/chat-ext");
    fs::create_dir_all(&ext_dir)?;

    // 1. Create a dummy manifest for an optional external component.
    let manifest_content = r#"
        id = "chat-ext"
        name = "Chat Extension"
        version = "0.1.0"
        sdk = "^0.0"

        [[components]]
        id = "chat-native"
        kind = "runtime"
        target = "example.runtime.native@1"
        required = false
    "#;
    fs::write(ext_dir.join("manifest.toml"), manifest_content)?;

    // 2. Set extension state as disabled initially in state.toml
    let state_file = temp_dir
        .path()
        .join("rintawa/extensions")
        .join(STATE_FILE_NAME);
    let mut state_config = ExtensionsStateConfig::default();
    state_config.set_enabled("chat-ext", false, "user", "2026-08-25T11:45:00Z");
    state_config.save_to_file(&state_file)?;

    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);

    // 3. Load directory — extension should be skipped because it is disabled
    let loaded = loader.load_directory(
        &mut engine,
        temp_dir.path().join("rintawa/extensions").as_path(),
    )?;
    assert_eq!(loaded.len(), 0);

    // 4. Enable extension and load again
    state_config.set_enabled("chat-ext", true, "admin", "2026-08-25T11:50:00Z");
    state_config.save_to_file(&state_file)?;

    let loaded = loader.load_directory(
        &mut engine,
        temp_dir.path().join("rintawa/extensions").as_path(),
    )?;
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        engine.extension_state(&loaded[0]),
        Some(ExtensionState::Registered)
    );

    Ok(())
}

#[test]
fn test_loader_rejects_duplicate_component_ids_before_artifact_loading() -> EngineResult<()> {
    let temp_dir = tempfile::tempdir()?;
    let ext_dir = temp_dir.path().join("duplicate-extension");
    fs::create_dir_all(&ext_dir)?;
    fs::write(
        ext_dir.join("manifest.toml"),
        r#"
        id = "duplicate-extension"
        name = "Duplicate Extension"
        version = "0.0.1"
        sdk = "^0.0"
        [[components]]
        id = "runtime"
        kind = "runtime"
        target = "example.runtime.wasm@1"
        entry = "does-not-exist-a.wasm"
        [[components]]
        id = "runtime"
        kind = "runtime"
        target = "example.runtime.wasm@1"
        entry = "does-not-exist-b.wasm"
    "#,
    )?;
    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
    let error = loader
        .load_single_extension(&mut engine, &ext_dir, &ExtensionsStateConfig::default())
        .unwrap_err();
    assert!(matches!(error, EngineError::ManifestValidation(
        ManifestValidationError::DuplicateComponentId { extension_id, component_id }
    ) if extension_id.as_str() == "duplicate-extension" && component_id.as_str() == "runtime"));
    Ok(())
}

#[test]
fn test_loader_rejects_required_external_target_without_artifact_host() -> EngineResult<()> {
    let temp_dir = tempfile::tempdir()?;
    let ext_dir = temp_dir.path().join("required-external");
    fs::create_dir_all(&ext_dir)?;
    fs::write(
        ext_dir.join("manifest.toml"),
        r#"
        id = "required-external"
        name = "Required External"
        version = "0.0.1"
        sdk = "^0.0"
        [[components]]
        id = "runtime"
        kind = "runtime"
        target = "example.runtime.external@1"
        required = true
    "#,
    )?;

    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
    let error = loader
        .load_single_extension(&mut engine, &ext_dir, &ExtensionsStateConfig::default())
        .expect_err("required external target must not be silently ignored");

    assert!(matches!(
        error,
        EngineError::UnsupportedRequiredComponentTarget {
            component_id,
            target,
        } if component_id == "runtime" && target == "example.runtime.external@1"
    ));
    Ok(())
}

#[test]
fn test_loader_directory_order_is_deterministic() -> EngineResult<()> {
    let temp_dir = tempfile::tempdir()?;
    let extensions_dir = temp_dir.path().join("extensions");
    fs::create_dir_all(&extensions_dir)?;

    for (directory, extension_id) in [("z-last", "z-extension"), ("a-first", "a-extension")] {
        let ext_dir = extensions_dir.join(directory);
        fs::create_dir_all(&ext_dir)?;
        fs::write(
            ext_dir.join("manifest.toml"),
            format!(
                r#"
id = "{extension_id}"
name = "{extension_id}"
version = "0.0.1"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "example.runtime.optional@1"
required = false
"#
            ),
        )?;
    }

    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
    let loaded = loader.load_directory(&mut engine, &extensions_dir)?;
    let loaded_ids = loaded.iter().map(|id| id.as_str()).collect::<Vec<_>>();

    assert_eq!(loaded_ids, ["a-extension", "z-extension"]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_loader_rejects_component_symlink_escape() -> EngineResult<()> {
    use std::os::unix::fs::symlink;

    let temp_dir = tempfile::tempdir()?;
    let ext_dir = temp_dir.path().join("extension");
    fs::create_dir_all(&ext_dir)?;
    let outside = temp_dir.path().join("outside.wasm");
    fs::write(&outside, b"not a component")?;
    symlink(&outside, ext_dir.join("runtime.wasm"))?;
    fs::write(
        ext_dir.join("manifest.toml"),
        r#"
id = "symlink-component"
name = "Symlink Component"
version = "0.0.1"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "runtime.wasm"
"#,
    )?;

    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
    let error = loader
        .load_single_extension(&mut engine, &ext_dir, &ExtensionsStateConfig::default())
        .expect_err("component symlink escaping the extension root must be rejected");

    assert!(matches!(
        error,
        EngineError::ExtensionPathEscapesRoot { path, root }
            if path.ends_with("outside.wasm") && root.ends_with("extension")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_loader_rejects_manifest_symlink_escape() -> EngineResult<()> {
    use std::os::unix::fs::symlink;

    let temp_dir = tempfile::tempdir()?;
    let ext_dir = temp_dir.path().join("extension");
    fs::create_dir_all(&ext_dir)?;
    let outside = temp_dir.path().join("outside-manifest.toml");
    fs::write(
        &outside,
        r#"
id = "symlink-manifest"
name = "Symlink Manifest"
version = "0.0.1"
sdk = "^0.0"
"#,
    )?;
    symlink(&outside, ext_dir.join("manifest.toml"))?;

    let mut engine = ExtensionEngine::new();
    let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
    let error = loader
        .load_single_extension(&mut engine, &ext_dir, &ExtensionsStateConfig::default())
        .expect_err("manifest symlink escaping the extension root must be rejected");

    assert!(matches!(
        error,
        EngineError::ExtensionPathEscapesRoot { path, root }
            if path.ends_with("outside-manifest.toml") && root.ends_with("extension")
    ));
    Ok(())
}
