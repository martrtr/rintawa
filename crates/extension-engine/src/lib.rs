//! Rintawa Extension Engine — lifecycle management, directory scanning, and runtime host.
//!
//! Provides core abstractions for:
//! - Parsing manifests and managing component lifecycles (`register`, `start`, `stop`).
//! - Dynamic filesystem discovery and state persistence via [`ExtensionLoader`] and [`ExtensionsStateConfig`].
//! - WASM runtime isolation and contribution side-effect tracking.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod composition;
pub mod context;
pub mod engine;
pub mod errors;
pub mod loader;
pub mod runtime;
pub mod secrets;
pub mod state;

mod runtime_effects;

pub use composition::{
    CompositionSnapshot, ContractBinding, UnresolvedContract, UnresolvedContractReason,
};
pub use engine::{ExtensionEngine, ExtensionState};
pub use errors::{ComponentStopFailure, EngineError, EngineResult};
pub use loader::ExtensionLoader;
pub use runtime::{WasmComponent, WasmExecutionBudget, WasmRuntimeEngine};
pub use secrets::SecretManager;
pub use state::{ExtensionStateRecord, ExtensionsStateConfig, STATE_FILE_NAME};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::InMemorySecretVault;
    use rintawa_sdk::prelude::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    struct DummyRuntime {
        id: ComponentId,
    }

    struct SubscriptionRuntime {
        id: ComponentId,
        should_fail_after_registering: bool,
    }

    struct SecretReaderRuntime {
        id: ComponentId,
        path: SecretPath,
        observed_value: Arc<Mutex<Option<String>>>,
        stop_read_denied: Arc<Mutex<bool>>,
    }

    struct LifecycleFailureRuntime {
        id: ComponentId,
        contribution_id: Option<&'static str>,
        fail_start: bool,
        fail_stop: bool,
    }

    impl LifecycleFailureRuntime {
        fn new(
            id: &str,
            contribution_id: Option<&'static str>,
            fail_start: bool,
            fail_stop: bool,
        ) -> Self {
            Self {
                id: ComponentId::new(id),
                contribution_id,
                fail_start,
                fail_stop,
            }
        }
    }

    impl SecretReaderRuntime {
        fn new(
            id: &str,
            path: SecretPath,
            observed_value: Arc<Mutex<Option<String>>>,
            stop_read_denied: Arc<Mutex<bool>>,
        ) -> Self {
            Self {
                id: ComponentId::new(id),
                path,
                observed_value,
                stop_read_denied,
            }
        }
    }

    impl SubscriptionRuntime {
        fn new(id: &str, should_fail_after_registering: bool) -> Self {
            Self {
                id: ComponentId::new(id),
                should_fail_after_registering,
            }
        }
    }

    impl Component for LifecycleFailureRuntime {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            if let Some(contribution_id) = self.contribution_id {
                ctx.register(ContributionDescriptor::new(
                    contribution_id,
                    ContributionKind::capability(),
                ))?;
            }
            Ok(())
        }

        fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            ctx.register_runtime_effect(RuntimeEffect::event_subscription(format!(
                "test.{}",
                self.id
            )))?;
            if self.fail_start {
                return Err(ExtensionError::Message(String::from(
                    "simulated start failure",
                )));
            }
            Ok(())
        }

        fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            if self.fail_stop {
                ctx.register_runtime_effect(RuntimeEffect::event_subscription(format!(
                    "stop.{}",
                    self.id
                )))?;
                return Err(ExtensionError::Message(String::from(
                    "simulated stop failure",
                )));
            }
            Ok(())
        }
    }

    impl Component for SubscriptionRuntime {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            ctx.register_runtime_effect(RuntimeEffect::event_subscription("dialogue.message"))?;

            if self.should_fail_after_registering {
                return Err(ExtensionError::Message(String::from(
                    "simulated startup failure",
                )));
            }

            Ok(())
        }
    }

    impl Component for SecretReaderRuntime {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            let value = ctx.read_secret(&self.path)?;
            *self.observed_value.lock().unwrap() = Some(value.expose_secret().to_string());
            Ok(())
        }

        fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
            *self.stop_read_denied.lock().unwrap() = ctx.read_secret(&self.path).is_err();
            Ok(())
        }
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
        let loader = ExtensionLoader::new(engine.wasm_runtime_engine()?);
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

    #[test]
    fn test_runtime_effects_are_owner_scoped_and_cleaned_on_stop_and_failed_start()
    -> EngineResult<()> {
        let manifest_toml = r#"
            id = "runtime-effects"
            name = "Runtime Effects"
            version = "0.0.1"
            sdk = "^0.0"
        "#;
        let mut engine = ExtensionEngine::new();
        let manifest = engine.parse_manifest(manifest_toml)?;

        engine.register_extension(
            manifest.clone(),
            vec![Box::new(SubscriptionRuntime::new("subscriber", false))],
        )?;
        engine.start_extension(&manifest.id)?;

        let effects = engine.active_runtime_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].1, &manifest.id);
        assert_eq!(effects[0].2.as_str(), "subscriber");
        assert_eq!(
            effects[0].3,
            &RuntimeEffect::event_subscription("dialogue.message")
        );

        engine.stop_extension(&manifest.id)?;
        assert!(engine.active_runtime_effects().is_empty());
        engine.start_extension(&manifest.id)?;
        assert_eq!(engine.active_runtime_effects().len(), 1);

        let failed_manifest = engine.parse_manifest(
            r#"
                id = "failing-runtime-effects"
                name = "Failing Runtime Effects"
                version = "0.0.1"
                sdk = "^0.0"
            "#,
        )?;
        engine.register_extension(
            failed_manifest.clone(),
            vec![Box::new(SubscriptionRuntime::new(
                "failing-subscriber",
                true,
            ))],
        )?;

        assert!(engine.start_extension(&failed_manifest.id).is_err());
        assert_eq!(engine.active_runtime_effects().len(), 1);

        Ok(())
    }

    #[test]
    fn test_stop_failures_are_returned_after_host_cleanup() -> EngineResult<()> {
        let mut engine = ExtensionEngine::new();
        let manifest = engine.parse_manifest(
            r#"
                id = "stop-failures"
                name = "Stop Failures"
                version = "0.0.1"
                sdk = "^0.0"
            "#,
        )?;

        engine.register_extension(
            manifest.clone(),
            vec![
                Box::new(LifecycleFailureRuntime::new(
                    "first",
                    Some("stop.first"),
                    false,
                    true,
                )),
                Box::new(LifecycleFailureRuntime::new(
                    "second",
                    Some("stop.second"),
                    false,
                    true,
                )),
            ],
        )?;
        engine.start_extension(&manifest.id)?;
        assert_eq!(engine.active_contributions().len(), 2);
        assert_eq!(engine.active_runtime_effects().len(), 2);

        let error = engine.stop_extension(&manifest.id).unwrap_err();
        match error {
            EngineError::StopFailed {
                extension_id,
                failures,
            } => {
                assert_eq!(extension_id, "stop-failures");
                assert_eq!(
                    failures,
                    vec![
                        ComponentStopFailure {
                            component_id: String::from("second"),
                            reason: String::from("simulated stop failure"),
                        },
                        ComponentStopFailure {
                            component_id: String::from("first"),
                            reason: String::from("simulated stop failure"),
                        },
                    ]
                );
            }
            other => panic!("expected StopFailed, got {other:?}"),
        }

        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Stopped)
        );
        assert!(engine.active_contributions().is_empty());
        assert!(engine.active_runtime_effects().is_empty());

        Ok(())
    }

    #[test]
    fn test_unregister_propagates_stop_failure_after_host_cleanup() -> EngineResult<()> {
        let mut engine = ExtensionEngine::new();
        let manifest = engine.parse_manifest(
            r#"
                id = "unregister-stop-failure"
                name = "Unregister Stop Failure"
                version = "0.0.1"
                sdk = "^0.0"
            "#,
        )?;

        engine.register_extension(
            manifest.clone(),
            vec![Box::new(LifecycleFailureRuntime::new(
                "runtime",
                Some("unregister.runtime"),
                false,
                true,
            ))],
        )?;
        engine.start_extension(&manifest.id)?;

        assert!(matches!(
            engine.unregister_extension(&manifest.id),
            Err(EngineError::StopFailed { .. })
        ));
        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Stopped)
        );
        assert!(engine.active_contributions().is_empty());
        assert!(engine.active_runtime_effects().is_empty());

        engine.unregister_extension(&manifest.id)?;
        assert_eq!(engine.extension_state(&manifest.id), None);

        Ok(())
    }

    #[test]
    fn test_startup_rollback_failures_are_aggregated_and_effects_revoked() -> EngineResult<()> {
        let mut engine = ExtensionEngine::new();
        let manifest = engine.parse_manifest(
            r#"
                id = "rollback-failures"
                name = "Rollback Failures"
                version = "0.0.1"
                sdk = "^0.0"
            "#,
        )?;

        engine.register_extension(
            manifest.clone(),
            vec![
                Box::new(LifecycleFailureRuntime::new("first", None, false, true)),
                Box::new(LifecycleFailureRuntime::new("second", None, true, true)),
            ],
        )?;

        let error = engine.start_extension(&manifest.id).unwrap_err();
        match error {
            EngineError::StartupRollbackFailed {
                extension_id,
                component_id,
                start_reason,
                rollback_failures,
            } => {
                assert_eq!(extension_id, "rollback-failures");
                assert_eq!(component_id, "second");
                assert_eq!(start_reason, "simulated start failure");
                assert_eq!(
                    rollback_failures,
                    vec![
                        ComponentStopFailure {
                            component_id: String::from("second"),
                            reason: String::from("simulated stop failure"),
                        },
                        ComponentStopFailure {
                            component_id: String::from("first"),
                            reason: String::from("simulated stop failure"),
                        },
                    ]
                );
            }
            other => panic!("expected StartupRollbackFailed, got {other:?}"),
        }

        assert_eq!(
            engine.extension_state(&manifest.id),
            Some(ExtensionState::Registered)
        );
        assert!(engine.active_runtime_effects().is_empty());

        Ok(())
    }

    #[test]
    fn test_should_grant_requested_secret_domain_only_to_active_component() -> EngineResult<()> {
        let secret_manager = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let secret_path = SecretPath::parse("ai.api_keys.openai").unwrap();
        secret_manager.store(&secret_path, &SecretValue::new("test-key"))?;

        let observed_value = Arc::new(Mutex::new(None));
        let stop_read_denied = Arc::new(Mutex::new(false));
        let component = Box::new(SecretReaderRuntime::new(
            "provider",
            secret_path.clone(),
            observed_value.clone(),
            stop_read_denied.clone(),
        ));
        let mut engine = ExtensionEngine::with_secret_manager(secret_manager);
        let manifest = engine.parse_manifest(
            r#"
                id = "official_ai"
                name = "Official AI"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "provider"
                kind = "runtime"
                target = "native"

                [components.permissions]
                secret-read = ["ai.api_keys.*"]
            "#,
        )?;
        let extension_id = manifest.id.clone();
        engine.register_extension(manifest, vec![component])?;

        assert!(engine.start_extension(&extension_id).is_err());
        assert!(observed_value.lock().unwrap().is_none());

        engine.grant_requested_secret_read(
            &extension_id,
            &ComponentId::new("provider"),
            SecretPathPattern::parse("ai.api_keys.openai").unwrap(),
        )?;
        engine.start_extension(&extension_id)?;
        assert_eq!(observed_value.lock().unwrap().as_deref(), Some("test-key"));

        engine.stop_extension(&extension_id)?;
        assert!(*stop_read_denied.lock().unwrap());

        assert!(matches!(
            engine.grant_requested_secret_read(
                &extension_id,
                &ComponentId::new("provider"),
                SecretPathPattern::parse("ai.*").unwrap(),
            ),
            Err(EngineError::SecretPermissionNotRequested { .. })
        ));

        engine.unregister_extension(&extension_id)?;
        assert!(matches!(
            engine.grant_requested_secret_read(
                &extension_id,
                &ComponentId::new("provider"),
                SecretPathPattern::parse("ai.api_keys.openai").unwrap(),
            ),
            Err(EngineError::ExtensionNotFound(_))
        ));

        let replacement_observed_value = Arc::new(Mutex::new(None));
        let replacement_stop_read_denied = Arc::new(Mutex::new(false));
        let replacement_manifest = engine.parse_manifest(
            r#"
                id = "official_ai"
                name = "Official AI"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "provider"
                kind = "runtime"
                target = "native"

                [components.permissions]
                secret-read = ["ai.api_keys.*"]
            "#,
        )?;
        engine.register_extension(
            replacement_manifest,
            vec![Box::new(SecretReaderRuntime::new(
                "provider",
                secret_path,
                replacement_observed_value.clone(),
                replacement_stop_read_denied,
            ))],
        )?;

        assert!(engine.start_extension(&extension_id).is_err());
        assert!(replacement_observed_value.lock().unwrap().is_none());

        Ok(())
    }

    #[test]
    fn test_should_revoke_secret_grants_for_manifest_only_components() -> EngineResult<()> {
        let secret_manager = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
        let secret_path = SecretPath::parse("ai.api_keys.openai").unwrap();
        secret_manager.store(&secret_path, &SecretValue::new("test-key"))?;

        let mut engine = ExtensionEngine::with_secret_manager(secret_manager);
        let manifest = engine.parse_manifest(
            r#"
                id = "optional_components"
                name = "Optional Components"
                version = "0.0.1"
                sdk = "^0.0"

                [[components]]
                id = "optional_provider"
                kind = "runtime"
                target = "wasm"
                required = false

                [components.permissions]
                secret-read = ["ai.api_keys.*"]
            "#,
        )?;
        let extension_id = manifest.id.clone();
        let component_id = ComponentId::new("optional_provider");
        engine.register_extension(manifest, Vec::new())?;
        engine.grant_requested_secret_read(
            &extension_id,
            &component_id,
            SecretPathPattern::parse("ai.api_keys.openai").unwrap(),
        )?;

        engine.unregister_extension(&extension_id)?;

        assert!(matches!(
            engine
                .secret_manager()
                .read_for_component(&extension_id, &component_id, &secret_path),
            Err(SecretAccessError::AccessDenied)
        ));

        Ok(())
    }
    #[test]
    fn test_should_reject_programmatic_manifest_with_duplicate_component_principals() {
        let mut engine = ExtensionEngine::new();
        let manifest = ExtensionManifest {
            id: ExtensionId::new("duplicate-principals"),
            name: String::from("Duplicate Principals"),
            version: String::from("0.0.1"),
            sdk: String::from("^0.0"),
            components: vec![
                ComponentDescriptor {
                    id: ComponentId::new("provider"),
                    kind: ComponentKind::Runtime,
                    target: ComponentTarget::new("native"),
                    entry: None,
                    required: true,
                    permissions: ComponentPermissions {
                        secret_read: vec![SecretPathPattern::parse("ai.api_keys.*").unwrap()],
                    },
                },
                ComponentDescriptor {
                    id: ComponentId::new("provider"),
                    kind: ComponentKind::Runtime,
                    target: ComponentTarget::new("native"),
                    entry: None,
                    required: true,
                    permissions: ComponentPermissions::default(),
                },
            ],
        };
        let error = engine.register_extension(manifest, Vec::new()).unwrap_err();
        assert!(matches!(error, EngineError::ManifestValidation(
            ManifestValidationError::DuplicateComponentId { extension_id, component_id }
        ) if extension_id.as_str() == "duplicate-principals" && component_id.as_str() == "provider"));
        assert_eq!(
            engine.extension_state(&ExtensionId::new("duplicate-principals")),
            None
        );
    }
}
