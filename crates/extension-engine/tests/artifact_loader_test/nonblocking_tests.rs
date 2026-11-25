//! Two Worlds exchange polled queries between hosted tasks sharing one real Store.
use super::*;
use rintawa_extension_engine::{HostAccessResult, RuntimeContextAccess, TransportError};
use rintawa_sdk::{
    contracts::ComponentPrincipal, runtime_permissions::RuntimePermission, world::WorldId,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
struct Worlds(HashMap<RuntimeScopeId, WorldId>);
impl RuntimeContextAccess for Worlds {
    fn world_id_for_scope(&self, scope: &RuntimeScopeId) -> HostAccessResult<Option<WorldId>> {
        Ok(self.0.get(scope).copied())
    }
}
fn observer(
    engine: &mut ExtensionEngine,
    id: &str,
    scope: &str,
) -> anyhow::Result<ComponentPrincipal> {
    let instance: ExtensionInstanceId = id.into();
    engine.register_extension_instance(
        instance.clone(),
        scope.into(),
        ExtensionManifest {
            id: ExtensionId::new(id),
            name: id.into(),
            version: "0.0.1".into(),
            sdk: "^0.0".into(),
            components: Vec::new(),
        },
        vec![Box::new(TestHostedComponent {
            id: "observer".into(),
        })],
    )?;
    engine.start_extension_instance(&instance)?;
    Ok(engine.active_component_principal(&ComponentRef::new(instance, "observer"))?)
}
fn load_client(
    engine: &mut ExtensionEngine,
    root: &Path,
    id: &str,
    role: &str,
    scope: &str,
) -> anyhow::Result<ComponentRef> {
    let source = root.join(format!("{id}-source"));
    write_rtw_source(
        &source,
        "rintawa.extension@1",
        format!(
            r#"id = "fixture.d8"
name = "D8 hosted task"
version = "0.0.1"
sdk = "^0.0"
[[components]]
id = "{role}"
kind = "runtime"
target = "test.wasm-target@1"
entry = "payload.bin"
[components.permissions]
runtime = ["background-task"]
"#
        )
        .as_bytes(),
    )?;
    fs::write(source.join("payload.bin"), b"hosted payload")?;
    let artifacts = root.join(format!("{id}-artifacts"));
    fs::create_dir(&artifacts)?;
    let (store, digest) = import_source(&source, &artifacts)?;
    RtwExtensionLoader::new().load_stored_extension(
        engine,
        &store,
        &digest,
        id.into(),
        scope.into(),
    )?;
    let owner = ComponentRef::new(id, role);
    engine.grant_requested_runtime_permission_for_instance(
        &owner.instance_id,
        &owner.component_id,
        RuntimePermission::BackgroundTask,
    )?;
    engine.start_extension_instance(&owner.instance_id)?;
    Ok(owner)
}
fn collect(
    engine: &mut ExtensionEngine,
    observer: &ComponentPrincipal,
    subscription: u64,
    until: impl Fn(&[String]) -> bool,
) -> anyhow::Result<Vec<String>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut events = Vec::new();
    loop {
        engine.poll_runtime()?;
        for event in engine.poll_transport_notifications(observer, subscription)? {
            events.push(String::from_utf8(event)?);
        }
        if until(&events) {
            return Ok(events);
        }
        anyhow::ensure!(Instant::now() < deadline, "missing events: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn test_hosted_same_store_query_reply_is_scope_bound_and_keeps_pump_available() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let mut engine = ExtensionEngine::new();
    engine.attach_runtime_context_access(Arc::new(Worlds(HashMap::from([
        ("world-a".into(), WorldId::new()),
        ("world-b".into(), WorldId::new()),
    ]))));
    let shared = load_wasm_target_provider(&mut engine, root.path(), "shared-provider")?;
    engine.grant_requested_runtime_permission_for_instance(
        &shared,
        &"runtime".into(),
        RuntimePermission::BackgroundTask,
    )?;
    engine.start_extension_instance(&shared)?;
    let a = observer(&mut engine, "observer-a", "world-a")?;
    let b = observer(&mut engine, "observer-b", "world-b")?;
    let sa = engine.subscribe_transport_notifications(&a, "fixture/d8-events")?;
    let sb = engine.subscribe_transport_notifications(&b, "fixture/d8-events")?;
    let pa = load_client(
        &mut engine,
        root.path(),
        "a-provider",
        "d8-provider",
        "world-a",
    )?;
    let ca = load_client(&mut engine, root.path(), "a-caller", "d8-caller", "world-a")?;
    let pb = load_client(
        &mut engine,
        root.path(),
        "b-provider",
        "d8-provider",
        "world-b",
    )?;
    let cb = load_client(&mut engine, root.path(), "b-caller", "d8-caller", "world-b")?;
    collect(&mut engine, &a, sa, |events| {
        events.iter().filter(|e| e.starts_with("ready|")).count() == 2
    })?;
    collect(&mut engine, &b, sb, |events| {
        events.iter().filter(|e| e.starts_with("ready|")).count() == 2
    })?;
    engine.publish_transport_notification(&a, "fixture/d8-start", b"alpha")?;
    let events = collect(&mut engine, &a, sa, |events| {
        events.iter().any(|e| e.starts_with("done|"))
    })?;
    assert!(
        events
            .iter()
            .any(|e| e.contains("handled|world-a|a-provider|") && e.ends_with("a-caller:alpha"))
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("done|world-a|a-caller|") && e.ends_with("|alpha"))
    );
    assert!(
        engine.poll_transport_notifications(&b, sb)?.is_empty(),
        "World A request leaked to World B"
    );
    engine.stop_extension_instance(&ca.instance_id)?;
    engine.publish_transport_notification(&b, "fixture/d8-start", b"beta")?;
    let events = collect(&mut engine, &b, sb, |events| {
        events.iter().any(|e| e.starts_with("done|"))
    })?;
    assert!(
        events
            .iter()
            .any(|e| e.contains("handled|world-b|b-provider|") && e.ends_with("b-caller:beta"))
    );
    engine.start_extension_instance(&ca.instance_id)?;
    collect(&mut engine, &a, sa, |events| {
        events.iter().any(|e| e.contains("ready|world-a|a-caller|"))
    })?;
    engine.publish_transport_notification(&a, "fixture/d8-start", b"replacement")?;
    collect(&mut engine, &a, sa, |events| {
        events
            .iter()
            .any(|e| e.contains("done|world-a|a-caller|") && e.ends_with("|replacement"))
    })?;
    let independent = ComponentRef::new("independent-backend", "runtime");
    let component = engine.wasm_runtime_engine()?.load_component_from_bytes(
        "runtime".into(),
        include_bytes!("../fixtures/transport/provider.wasm"),
    )?;
    engine.register_extension_instance(
        independent.instance_id.clone(),
        "independent".into(),
        ExtensionManifest {
            id: "independent".into(),
            name: "independent".into(),
            version: "0.0.1".into(),
            sdk: "^0.0".into(),
            components: Vec::new(),
        },
        vec![Box::new(component)],
    )?;
    engine.start_extension_instance(&independent.instance_id)?;
    let old = engine.active_component_principal(&ca)?;
    let trap = load_client(&mut engine, root.path(), "trap-client", "trap", "world-a")?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while engine.active_component_principal(&ca).is_ok() {
        engine.poll_runtime()?;
        anyhow::ensure!(
            Instant::now() < deadline,
            "shared Store was not quarantined"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        engine.publish_transport_notification(&old, "fixture/d8-start", b"stale"),
        Err(TransportError::Inactive)
    ));
    let independent_principal = engine.active_component_principal(&independent)?;
    assert_eq!(
        engine.query_transport_contributions(&independent_principal, "fixture/caller", b"ping")?,
        vec![b"wasm:ping".to_vec()]
    );
    for owner in [ca, pa, pb, cb, trap] {
        engine.unregister_extension_instance(&owner.instance_id)?;
    }
    engine.unregister_extension_instance(&shared)?;
    engine.stop_extension_instance(&independent.instance_id)?;
    engine.unregister_extension_instance(&independent.instance_id)?;
    Ok(())
}
