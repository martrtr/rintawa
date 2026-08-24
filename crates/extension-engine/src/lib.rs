//! Taverna Extension Engine — lifecycle management and runtime host.
//!
//! Owns parsing manifests, component lifecycle invocation (`register`, `start`, `stop`),
//! tracking contributions, and rolling back side-effects on deactivation.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod context;
pub mod engine;
pub mod errors;

pub use engine::{ExtensionEngine, ExtensionState};
pub use errors::{EngineError, EngineResult};

#[cfg(test)]
mod tests {
    use super::*;
    use taverna_sdk::prelude::*;

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
}
