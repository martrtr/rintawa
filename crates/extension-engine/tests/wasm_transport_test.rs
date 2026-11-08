//! Production Host/WIT integration: real independent WASM providers and consumer.
use anyhow::Result;
use rintawa_extension_engine::{ExtensionEngine, TransportError};
use rintawa_sdk::{
    contracts::ComponentRef,
    manifest::{
        ComponentDescriptor, ComponentKind, ComponentPermissions, ExtensionManifest,
        WASM_COMPONENT_TARGET_V1,
    },
    types::{ComponentId, ComponentTarget, ExtensionId, ExtensionInstanceId, RuntimeScopeId},
};

const PROVIDER: &[u8] = include_bytes!("fixtures/transport/provider.wasm");
const CONSUMER: &[u8] = include_bytes!("fixtures/transport/consumer.wasm");

fn manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_owned(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: vec![ComponentDescriptor {
            id: ComponentId::new("runtime"),
            kind: ComponentKind::Runtime,
            target: ComponentTarget::new(WASM_COMPONENT_TARGET_V1),
            entry: None,
            required: true,
            permissions: ComponentPermissions {
                secret_read: Vec::new(),
                runtime: Vec::new(),
            },
        }],
    }
}

#[test]
fn test_should_exchange_wasm_fanout_and_revoke_on_stop_across_world_scopes() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    for (id, scope, bytes) in [
        ("one", "world-a", PROVIDER),
        ("two", "world-a", PROVIDER),
        ("consumer", "world-a", CONSUMER),
        ("foreign", "world-b", PROVIDER),
    ] {
        let component = engine
            .wasm_runtime_engine()?
            .load_component_from_bytes(ComponentId::new("runtime"), bytes)?;
        engine.register_extension_instance(
            ExtensionInstanceId::new(id),
            RuntimeScopeId::new(scope),
            manifest(id),
            vec![Box::new(component)],
        )?;
    }

    let caller = ComponentRef::new("consumer", "runtime");
    let foreign = ComponentRef::new("foreign", "runtime");
    let missing = ComponentRef::new("unknown", "runtime");
    let point = "chat/send-controls/v1";
    assert!(matches!(
        engine.query_transport_contributions(&caller, point, b"ping"),
        Err(TransportError::Inactive)
    ));
    engine.start_extension_instance(&ExtensionInstanceId::new("one"))?;
    engine.start_extension_instance(&ExtensionInstanceId::new("two"))?;
    engine.start_extension_instance(&ExtensionInstanceId::new("foreign"))?;
    // The WASM consumer's start imports rintawa:engine/transport.query
    // and verifies both answers through real WASM provider callbacks.
    engine.start_extension_instance(&ExtensionInstanceId::new("consumer"))?;

    assert_eq!(
        engine.query_transport_contributions(&caller, point, b"ping")?,
        vec![b"wasm:ping".to_vec(), b"wasm:ping".to_vec()]
    );
    assert_eq!(
        engine.query_transport_contributions(&foreign, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    // Two WASM providers subscribe on start. Both publish through their
    // query callbacks, and a native caller publishes under the same scope.
    let updates = "chat/updates/v1";
    let handle = engine.subscribe_transport_notifications(&caller, updates)?;
    let other_scope_handle = engine.subscribe_transport_notifications(&foreign, updates)?;
    assert!(matches!(
        engine.poll_transport_notifications(&foreign, handle),
        Err(TransportError::UnknownSubscription)
    ));
    let sent = engine.query_transport_contributions(&caller, point, b"send-event")?;
    assert_eq!(sent, vec![b"published".to_vec(), b"published".to_vec()]);
    engine.publish_transport_notification(&caller, updates, b"from-native")?;

    let mut received = Vec::new();
    for _ in 0..60 {
        received.extend(engine.poll_transport_notifications(&caller, handle)?);
        if received.len() == 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    received.sort();
    assert_eq!(
        received,
        vec![
            b"from-native".to_vec(),
            b"from-wasm".to_vec(),
            b"from-wasm".to_vec()
        ]
    );
    assert!(
        engine
            .poll_transport_notifications(&foreign, other_scope_handle)?
            .is_empty()
    );

    let mut received_by_wasm = 0_usize;
    for _ in 0..60 {
        for reply in engine.query_transport_contributions(&caller, point, b"drain-events")? {
            let text = String::from_utf8(reply)?;
            let count = text
                .strip_prefix("events:")
                .ok_or_else(|| anyhow::anyhow!("unexpected WASM event response"))?
                .parse::<usize>()?;
            received_by_wasm += count;
        }
        if received_by_wasm == 6 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(received_by_wasm, 6);
    assert_eq!(
        engine.query_transport_contributions(&foreign, point, b"drain-events")?,
        vec![b"events:0".to_vec()]
    );

    engine.unsubscribe_transport_notifications(&caller, handle)?;
    assert!(matches!(
        engine.poll_transport_notifications(&caller, handle),
        Err(TransportError::UnknownSubscription)
    ));
    assert!(matches!(
        engine.query_transport_contributions(&caller, point, b"oversized"),
        Err(TransportError::Unavailable)
    ));
    assert!(matches!(
        engine.query_transport_contributions(&missing, point, b"ping"),
        Err(TransportError::Inactive)
    ));
    assert!(matches!(
        engine.query_transport_contributions(&caller, "../escape", b"ping"),
        Err(TransportError::InvalidName)
    ));

    let owner_one = ComponentRef::new("one", "runtime");
    let stale_handle = engine.subscribe_transport_notifications(&owner_one, updates)?;
    engine.stop_extension_instance(&ExtensionInstanceId::new("one"))?;
    assert!(matches!(
        engine.poll_transport_notifications(&owner_one, stale_handle),
        Err(TransportError::Inactive)
    ));
    assert_eq!(
        engine.query_transport_contributions(&caller, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    engine.stop_extension_instance(&ExtensionInstanceId::new("two"))?;
    assert!(
        engine
            .query_transport_contributions(&caller, point, b"ping")?
            .is_empty()
    );
    engine.start_extension_instance(&ExtensionInstanceId::new("one"))?;
    assert!(matches!(
        engine.poll_transport_notifications(&owner_one, stale_handle),
        Err(TransportError::UnknownSubscription)
    ));
    assert_eq!(
        engine.query_transport_contributions(&caller, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    engine.unregister_extension_instance(&ExtensionInstanceId::new("one"))?;
    assert!(
        engine
            .query_transport_contributions(&caller, point, b"ping")?
            .is_empty()
    );
    engine.stop_extension_instance(&ExtensionInstanceId::new("consumer"))?;
    assert!(matches!(
        engine.query_transport_contributions(&caller, point, b"ping"),
        Err(TransportError::Inactive)
    ));
    Ok(())
}

#[test]
fn test_should_quarantine_wasm_trap_and_recover_after_explicit_reload() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let component = engine
        .wasm_runtime_engine()?
        .load_component_from_bytes(ComponentId::new("runtime"), PROVIDER)?;
    engine.register_extension_instance(
        ExtensionInstanceId::new("budgeted"),
        RuntimeScopeId::new("world-budget"),
        manifest("budgeted"),
        vec![Box::new(component)],
    )?;
    engine.start_extension_instance(&ExtensionInstanceId::new("budgeted"))?;
    let caller = ComponentRef::new("budgeted", "runtime");
    let point = "chat/send-controls/v1";
    // A guest spin loop must exhaust Wasmtime fuel, not stall the transport.
    let started = std::time::Instant::now();
    assert!(matches!(
        engine.query_transport_contributions(&caller, point, b"fuel-burn"),
        Err(TransportError::Unavailable)
    ));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "untrusted WASM query was not bounded"
    );
    // An out-of-fuel trap permanently invalidates the Wasmtime Component
    // instance. It must be discarded, not re-entered.
    assert!(matches!(
        engine.query_transport_contributions(&caller, point, b"ping"),
        Err(TransportError::Unavailable)
    ));
    let id = ExtensionInstanceId::new("budgeted");
    engine.stop_extension_instance(&id)?;
    engine.unregister_extension_instance(&id)?;

    // Explicit reload is the MVP recovery policy; no hidden replay of effects.
    let fresh = engine
        .wasm_runtime_engine()?
        .load_component_from_bytes(ComponentId::new("runtime"), PROVIDER)?;
    engine.register_extension_instance(
        id.clone(),
        RuntimeScopeId::new("world-budget"),
        manifest("budgeted"),
        vec![Box::new(fresh)],
    )?;
    engine.start_extension_instance(&id)?;
    assert_eq!(
        engine.query_transport_contributions(&caller, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    engine.stop_extension_instance(&id)?;
    Ok(())
}

#[test]
fn test_should_enforce_scoped_authority_for_real_wasm_providers() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    for (id, scope) in [
        ("one", "world-a"),
        ("two", "world-a"),
        ("foreign", "world-b"),
    ] {
        let component = engine
            .wasm_runtime_engine()?
            .load_component_from_bytes(ComponentId::new("runtime"), PROVIDER)?;
        engine.register_extension_instance(
            ExtensionInstanceId::new(id),
            RuntimeScopeId::new(scope),
            manifest(id),
            vec![Box::new(component)],
        )?;
    }
    let scope_a = RuntimeScopeId::new("world-a");
    let scope_b = RuntimeScopeId::new("world-b");
    let owner_one = ComponentRef::new("one", "runtime");
    let owner_two = ComponentRef::new("two", "runtime");
    let foreign = ComponentRef::new("foreign", "runtime");
    let point = "chat/send-controls/v1";

    engine.set_authoritative_transport_provider_in_scope(&scope_a, point, &owner_one)?;
    engine.set_authoritative_transport_provider_in_scope(&scope_b, point, &foreign)?;

    // The unselected component still starts but cannot occupy this shared point.
    engine.start_extension_instance(&ExtensionInstanceId::new("two"))?;
    assert!(matches!(
        engine.query_transport_contributions(&owner_two, point, b"ping"),
        Err(TransportError::Unavailable)
    ));
    engine.start_extension_instance(&ExtensionInstanceId::new("one"))?;
    engine.start_extension_instance(&ExtensionInstanceId::new("foreign"))?;
    assert_eq!(
        engine.query_transport_contributions(&owner_two, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    assert_eq!(
        engine.query_transport_contributions(&foreign, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );

    engine.stop_extension_instance(&ExtensionInstanceId::new("one"))?;
    assert!(matches!(
        engine.query_transport_contributions(&owner_two, point, b"ping"),
        Err(TransportError::Unavailable)
    ));
    engine.stop_extension_instance(&ExtensionInstanceId::new("two"))?;
    engine.start_extension_instance(&ExtensionInstanceId::new("two"))?;
    assert!(matches!(
        engine.query_transport_contributions(&owner_two, point, b"ping"),
        Err(TransportError::Unavailable)
    ));
    assert_eq!(
        engine.query_transport_contributions(&foreign, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );

    engine.clear_authoritative_transport_providers_in_scope(&scope_a);
    engine.stop_extension_instance(&ExtensionInstanceId::new("two"))?;
    engine.start_extension_instance(&ExtensionInstanceId::new("two"))?;
    assert_eq!(
        engine.query_transport_contributions(&owner_two, point, b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    Ok(())
}
