use rintawa_extension_engine::{
    EngineResult, ExtensionEngine, ExtensionLoader, ExtensionState, ExtensionsStateConfig,
    STATE_FILE_NAME,
};
use std::fs;

#[test]
fn test_extension_loader_and_state_persistence() -> EngineResult<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let ext_dir = temp_dir.path().join("rintawa/extensions/chat-ext");
    fs::create_dir_all(&ext_dir)?;

    // 1. Create a dummy manifest for a native component
    let manifest_content = r#"
        id = "chat-ext"
        name = "Chat Extension"
        version = "0.1.0"
        sdk = "^0.0"

        [[components]]
        id = "chat-native"
        kind = "runtime"
        target = "native"
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
    let loader = ExtensionLoader::default();

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
