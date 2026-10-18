//! Integration tests for runtime event delivery across extension boundaries.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rintawa_extension_engine::{EngineError, ExtensionEngine, ExtensionState};
use rintawa_sdk::prelude::*;

#[derive(Clone)]
struct RecordingState {
    deliveries: Arc<Mutex<Vec<String>>>,
}

impl RecordingState {
    fn new() -> Self {
        Self {
            deliveries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn values(&self) -> Vec<String> {
        self.deliveries
            .lock()
            .map(|values| values.clone())
            .unwrap_or_default()
    }
}

struct EventSubscriber {
    id: ComponentId,
    topic: String,
    label: String,
    state: RecordingState,
    duplicate_subscription: bool,
    signal_subscription: bool,
    fail_on_event: bool,
    unsubscribe_on_event: bool,
    subscription: Option<RuntimeEffectId>,
}

impl EventSubscriber {
    fn new(id: &str, topic: &str, label: &str, state: RecordingState) -> Self {
        Self {
            id: ComponentId::new(id),
            topic: topic.to_string(),
            label: label.to_string(),
            state,
            duplicate_subscription: false,
            signal_subscription: false,
            fail_on_event: false,
            unsubscribe_on_event: false,
            subscription: None,
        }
    }

    fn with_duplicate_subscription(mut self) -> Self {
        self.duplicate_subscription = true;
        self
    }

    fn signal(mut self) -> Self {
        self.signal_subscription = true;
        self
    }

    fn failing(mut self) -> Self {
        self.fail_on_event = true;
        self
    }

    fn unsubscribing(mut self) -> Self {
        self.unsubscribe_on_event = true;
        self
    }
}

impl Component for EventSubscriber {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let runtime_effect = if self.signal_subscription {
            RuntimeEffect::signal_subscription(self.topic.clone())
        } else {
            RuntimeEffect::event_subscription(self.topic.clone())
        };
        let effect = ctx.register_runtime_effect(runtime_effect.clone())?;
        self.subscription = Some(effect);
        if self.duplicate_subscription {
            ctx.register_runtime_effect(runtime_effect)?;
        }
        Ok(())
    }

    fn handle_event(
        &mut self,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        let payload = std::str::from_utf8(payload)
            .map_err(|error| ExtensionError::Message(error.to_string()))?;
        self.state
            .deliveries
            .lock()
            .map_err(|_| ExtensionError::Message(String::from("recording state lock poisoned")))?
            .push(format!("{}:{topic}:{payload}", self.label));

        if self.unsubscribe_on_event
            && let Some(effect) = self.subscription.take()
        {
            ctx.revoke_runtime_effect(&effect)?;
        }

        if self.fail_on_event {
            return Err(ExtensionError::Message(String::from(
                "intentional event callback failure",
            )));
        }
        Ok(())
    }
}

fn manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_string(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: Vec::new(),
    }
}

fn register_subscriber(
    engine: &mut ExtensionEngine,
    instance: &str,
    scope: &RuntimeScopeId,
    extension: &str,
    subscriber: EventSubscriber,
) -> Result<ExtensionInstanceId> {
    let instance = ExtensionInstanceId::new(instance);
    engine.register_extension_instance(
        instance.clone(),
        scope.clone(),
        manifest(extension),
        vec![Box::new(subscriber)],
    )?;
    engine.start_extension_instance(&instance)?;
    Ok(instance)
}

#[test]
fn test_should_route_exact_topic_once_in_deterministic_owner_order() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");
    let state = RecordingState::new();

    register_subscriber(
        &mut engine,
        "b-instance",
        &scope,
        "example.b",
        EventSubscriber::new("runtime", "dialogue.message@1", "b", state.clone()),
    )?;
    register_subscriber(
        &mut engine,
        "a-instance",
        &scope,
        "example.a",
        EventSubscriber::new("runtime", "dialogue.message@1", "a", state.clone())
            .with_duplicate_subscription(),
    )?;

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "dialogue.message@1", b"hello")?,
        2
    );
    assert_eq!(
        state.values(),
        vec![
            String::from("a:dialogue.message@1:hello"),
            String::from("b:dialogue.message@1:hello")
        ]
    );

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "dialogue.other@1", b"ignored")?,
        0
    );
    Ok(())
}

#[test]
fn test_should_isolate_event_delivery_by_runtime_scope() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope_a = RuntimeScopeId::new("world:a");
    let scope_b = RuntimeScopeId::new("world:b");
    let state = RecordingState::new();

    register_subscriber(
        &mut engine,
        "subscriber-a",
        &scope_a,
        "example.a",
        EventSubscriber::new("runtime", "example.event@1", "a", state.clone()),
    )?;
    register_subscriber(
        &mut engine,
        "subscriber-b",
        &scope_b,
        "example.b",
        EventSubscriber::new("runtime", "example.event@1", "b", state.clone()),
    )?;

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope_a, "example.event@1", b"payload")?,
        1
    );
    assert_eq!(
        state.values(),
        vec![String::from("a:example.event@1:payload")]
    );
    Ok(())
}

#[test]
fn test_should_apply_unsubscribe_from_event_callback_before_next_dispatch() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");
    let state = RecordingState::new();

    register_subscriber(
        &mut engine,
        "subscriber",
        &scope,
        "example.subscriber",
        EventSubscriber::new("runtime", "example.event@1", "once", state.clone()).unsubscribing(),
    )?;

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "example.event@1", b"first")?,
        1
    );
    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "example.event@1", b"second")?,
        0
    );
    assert_eq!(
        state.values(),
        vec![String::from("once:example.event@1:first")]
    );
    Ok(())
}

#[test]
fn test_should_quarantine_failed_subscriber_and_continue_delivery() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");
    let state = RecordingState::new();

    let failed = register_subscriber(
        &mut engine,
        "a-failed",
        &scope,
        "example.failed",
        EventSubscriber::new("runtime", "example.event@1", "failed", state.clone()).failing(),
    )?;
    let healthy = register_subscriber(
        &mut engine,
        "b-healthy",
        &scope,
        "example.healthy",
        EventSubscriber::new("runtime", "example.event@1", "healthy", state.clone()),
    )?;

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "example.event@1", b"payload")?,
        1
    );
    assert_eq!(
        engine.extension_instance_state(&failed),
        Some(ExtensionState::Stopped)
    );
    assert_eq!(
        engine.extension_instance_state(&healthy),
        Some(ExtensionState::Active)
    );
    assert_eq!(
        state.values(),
        vec![
            String::from("failed:example.event@1:payload"),
            String::from("healthy:example.event@1:payload")
        ]
    );

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, "example.event@1", b"again")?,
        1
    );
    assert_eq!(
        state.values(),
        vec![
            String::from("failed:example.event@1:payload"),
            String::from("healthy:example.event@1:payload"),
            String::from("healthy:example.event@1:again")
        ]
    );
    Ok(())
}

#[test]
fn test_should_keep_event_and_signal_subscriptions_disjoint() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");
    let state = RecordingState::new();
    let topic = "example.shared@1";

    register_subscriber(
        &mut engine,
        "event-subscriber",
        &scope,
        "example.event",
        EventSubscriber::new("runtime", topic, "event", state.clone()),
    )?;
    register_subscriber(
        &mut engine,
        "signal-subscriber",
        &scope,
        "example.signal",
        EventSubscriber::new("runtime", topic, "signal", state.clone()).signal(),
    )?;

    assert_eq!(
        engine.dispatch_runtime_event_in_scope(&scope, topic, b"durable")?,
        1
    );
    assert_eq!(
        engine.dispatch_runtime_signal_in_scope(&scope, topic, b"ephemeral")?,
        1
    );
    assert_eq!(
        state.values(),
        vec![
            String::from("event:example.shared@1:durable"),
            String::from("signal:example.shared@1:ephemeral"),
        ]
    );
    Ok(())
}

#[test]
fn test_should_reject_empty_runtime_signal_topic() {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");

    assert!(matches!(
        engine.dispatch_runtime_signal_in_scope(&scope, "   ", b"payload"),
        Err(EngineError::InvalidRuntimeSignalTopic)
    ));
}

#[test]
fn test_should_reject_empty_runtime_event_topic() {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world:test");

    assert!(matches!(
        engine.dispatch_runtime_event_in_scope(&scope, "   ", b"payload"),
        Err(EngineError::InvalidRuntimeEventTopic)
    ));
}
