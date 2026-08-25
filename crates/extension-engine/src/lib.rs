//! Taverna Extension Engine — lifecycle management, directory scanning, and runtime host.
//!
//! Provides core abstractions for:
//! - Parsing manifests and managing component lifecycles (`register`, `start`, `stop`).
//! - Dynamic filesystem discovery and state persistence via [`ExtensionLoader`] and [`ExtensionsStateConfig`].
//! - WASM runtime isolation and contribution side-effect tracking.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod context;
pub mod engine;
pub mod errors;
pub mod loader;
pub mod runtime;
pub mod state;

pub use engine::{ExtensionEngine, ExtensionState};
pub use errors::{EngineError, EngineResult};
pub use loader::ExtensionLoader;
pub use runtime::{WasmComponent, WasmRuntimeEngine};
pub use state::{ExtensionStateRecord, ExtensionsStateConfig, STATE_FILE_NAME};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use taverna_sdk::prelude::*;
    use tempfile::tempdir;

    struct DummyRuntime {
        id: ComponentId,
    }

    impl DummyRuntime {
        fn new() -> Self {
            Self {
                id: ComponentId::from("runtime"),
            }
        }
    }

    impl Component for DummyRuntime {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            ctx.register(ContributionDescriptor::new(
                "chat.system",
                ContributionKind::system(),
            ))?;
            Ok(())
        }
    }

    #[test]
    fn test_full_extension_lifecycle_with_restart_and_unregister() -> EngineResult<()> {
        let mut engine = ExtensionEngine::new();

        let manifest_toml = r#"
            id = "chat"
            name = "Chat Extension"
            version = "0.0.1"
            sdk = "^0.0"

            [[components]]
            id = "runtime"
            kind = "runtime"
            target = "native"
        "#;

        let manifest = engine.parse_manifest(manifest_toml)?;
        let component = Box::new(DummyRuntime::new());

        engine.register_extension(manifest.clone(), vec![component])?;
        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Registered)
        );
        assert_eq!(engine.active_contributions().len(), 1);

        engine.start_extension(&manifest.id)?;
        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Active)
        );

        // Test stopping and contribution cleanup
        engine.stop_extension(&manifest.id)?;
        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Stopped)
        );
        assert_eq!(engine.active_contributions().len(), 0);

        // Test re-starting from stopped state
        engine.start_extension(&manifest.id)?;
        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Active)
        );
        assert_eq!(engine.active_contributions().len(), 1);

        // Test complete unregistration
        engine.unregister_extension(&manifest.id)?;
        assert_eq!(engine.extension_state(&manifest.id), None);
        assert_eq!(engine.active_contributions().len(), 0);

        Ok(())
    }

    #[test]
    fn test_loader_and_state_config_integration() -> EngineResult<()> {
        let dir = tempdir().map_err(EngineError::Io)?;
        let ext_dir = dir.path().join("chat-ext");
        fs::create_dir_all(&ext_dir).map_err(EngineError::Io)?;

        let manifest_toml = r#"
            id = "chat-ext"
            name = "Chat Extension"
            version = "0.1.0"
            sdk = "^0.0"
        "#;
        fs::write(ext_dir.join("manifest.toml"), manifest_toml).map_err(EngineError::Io)?;

        let mut engine = ExtensionEngine::new();
        let loader = ExtensionLoader::default();
        let mut state_config = ExtensionsStateConfig::default();

        // 1. Extension is enabled by default
        let loaded = loader.load_single_extension(&mut engine, &ext_dir, &state_config)?;
        assert_eq!(loaded, Some(ExtensionId::from("chat-ext")));
        assert_eq!(
            engine.extension_state(&ExtensionId::from("chat-ext")),
            Some(ExtensionState::Registered)
        );

        // 2. Disable extension via state configuration
        state_config.set_enabled("chat-ext", false, "user", "2026-08-25T12:00:00Z");
        let mut new_engine = ExtensionEngine::new();
        let loaded_disabled =
            loader.load_single_extension(&mut new_engine, &ext_dir, &state_config)?;
        assert_eq!(loaded_disabled, None);
        assert_eq!(
            new_engine.extension_state(&ExtensionId::from("chat-ext")),
            None
        );

        Ok(())
    }
}
