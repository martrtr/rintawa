//! Integration tests for extension lifecycle transitions.

use rintawa_extension_engine::{EngineResult, ExtensionEngine, ExtensionState};
use rintawa_sdk::prelude::*;

/// Mock runtime component used exclusively for integration lifecycle tests.
struct MockChatComponent {
    id: ComponentId,
}

impl MockChatComponent {
    fn new() -> Self {
        Self {
            id: ComponentId::new("chat-runtime"),
        }
    }
}

impl Component for MockChatComponent {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        ctx.logger().info("Registering chat component...");
        ctx.register(ContributionDescriptor::new(
            "chat.send_message",
            ContributionKind::command(),
        ))?;
        Ok(())
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        ctx.logger().info("Starting chat component...");
        Ok(())
    }

    fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        ctx.logger().info("Stopping chat component...");
        Ok(())
    }
}

#[test]
fn test_sdk_and_engine_integration_lifecycle() -> EngineResult<()> {
    let mut engine = ExtensionEngine::new();

    // 1. Parse a valid extension manifest containing a native component declaration.
    let raw_manifest = r#"
        id = "rintawa-chat"
        name = "Rintawa Chat"
        version = "0.1.0"
        sdk = "^0.0"

        [[components]]
        id = "chat-runtime"
        kind = "runtime"
        target = "example.runtime.native@1"
    "#;

    let manifest = engine.parse_manifest(raw_manifest)?;
    let component = Box::new(MockChatComponent::new());

    // 2. Register the extension and verify that side-effect contributions are indexed.
    engine.register_extension(manifest.clone(), vec![component])?;
    assert_eq!(
        engine.extension_state(&manifest.id),
        Some(ExtensionState::Registered)
    );
    assert_eq!(engine.active_contributions().len(), 1);
    assert_eq!(
        engine.active_contributions()[0].id.as_str(),
        "chat.send_message"
    );
    let (extension_id, component_id) = engine
        .active_contribution_owner(&ContributionId::new("chat.send_message"))
        .expect("registered contribution should retain its component owner");
    assert_eq!(extension_id.as_str(), "rintawa-chat");
    assert_eq!(component_id.as_str(), "chat-runtime");

    // 3. Start the extension and ensure its state transitions to Active.
    engine.start_extension(&manifest.id)?;
    assert_eq!(
        engine.extension_state(&manifest.id),
        Some(ExtensionState::Active)
    );

    // 4. Stop the extension and verify that active contributions are removed.
    engine.stop_extension(&manifest.id)?;
    assert_eq!(
        engine.extension_state(&manifest.id),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(engine.active_contributions().len(), 0);

    // 5. Fully unregister the extension from the engine runtime.
    engine.unregister_extension(&manifest.id)?;
    assert_eq!(engine.extension_state(&manifest.id), None);

    Ok(())
}

struct AdmissionComponent {
    id: ComponentId,
    starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Component for AdmissionComponent {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn id(&self) -> &ComponentId {
        &self.id
    }
    fn register(&mut self, _: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        Ok(())
    }
    fn start(&mut self, _: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn stop(&mut self, _: &mut dyn ComponentContext) -> ExtensionResult<()> {
        Ok(())
    }
}
#[test]
fn test_component_admission_refuses_before_partial_start_and_releases_slots() -> EngineResult<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let starts = Arc::new(AtomicUsize::new(0));
    let mut engine = ExtensionEngine::new();
    let manifest = |name: &str| ExtensionManifest {
        id: ExtensionId::new(name),
        name: name.into(),
        version: "0.0.1".into(),
        sdk: "^0.0".into(),
        components: vec![],
    };
    let components = (0..128)
        .map(|index| {
            Box::new(AdmissionComponent {
                id: ComponentId::new(format!("component-{index}")),
                starts: starts.clone(),
            }) as Box<dyn Component>
        })
        .collect();
    engine.register_extension(manifest("full"), components)?;
    engine.start_extension(&ExtensionId::new("full"))?;
    engine.register_extension(
        manifest("overflow"),
        vec![Box::new(AdmissionComponent {
            id: ComponentId::new("overflow"),
            starts: starts.clone(),
        })],
    )?;
    assert!(
        engine
            .start_extension(&ExtensionId::new("overflow"))
            .is_err()
    );
    assert_eq!(starts.load(Ordering::SeqCst), 128);
    engine.stop_extension(&ExtensionId::new("full"))?;
    engine.start_extension(&ExtensionId::new("overflow"))?;
    assert_eq!(starts.load(Ordering::SeqCst), 129);
    Ok(())
}
