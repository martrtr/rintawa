use taverna_extension_engine::{EngineResult, ExtensionEngine, ExtensionState};
use taverna_sdk::prelude::*;

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
        id = "taverna-chat"
        name = "Taverna Chat"
        version = "0.1.0"
        sdk = "^0.0"

        [[components]]
        id = "chat-runtime"
        kind = "runtime"
        target = "native"
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
