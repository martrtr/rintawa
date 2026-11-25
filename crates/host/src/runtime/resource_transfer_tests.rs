//! Real WASM transfer cleanup through production Host world lifecycle.
use super::*;
use serde_json::{Value, json};
use std::time::Instant;

const BACKEND: &[u8] =
    include_bytes!("../../../extension-engine/tests/fixtures/world-system/component.wasm");
const CONSUMER: &[u8] =
    include_bytes!("../../../extension-engine/tests/fixtures/transport/provider.wasm");

fn install(
    root: &std::path::Path,
    home: &HostHome,
    slug: &str,
    bytes: &[u8],
) -> anyhow::Result<crate::InstallResult> {
    let source = root.join(slug);
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    std::fs::write(
        source.join("manifest.toml"),
        format!(
            "id = \"fixture.{slug}\"\nname = \"Transfer fixture\"\nversion = \"0.0.1\"\nsdk = \"^0.0\"\n[[components]]\nid = \"runtime\"\nkind = \"runtime\"\ntarget = \"rintawa.runtime.wasm-component@1\"\nentry = \"component.wasm\"\n[components.permissions]\nruntime = [\"background-task\", \"asset-import\", \"asset-read\", \"user-resource-import\", \"user-resource-read\"]\n"
        ),
    )?;
    std::fs::write(source.join("component.wasm"), bytes)?;
    let archive = root.join(format!("{slug}.rtw"));
    rintawa_artifacts::pack_directory(&source, &archive, rintawa_artifacts::RtwLimits::default())?;
    Ok(home.install_local_rtw(archive, Some(false))?)
}
fn query(host: &HostRuntime, consumer: &ComponentRef, request: Value) -> anyhow::Result<Value> {
    let principal = host.engine.active_component_principal(consumer)?;
    let mut payload = b"forward:fixture/commands\n".to_vec();
    payload.extend(serde_json::to_vec(&request)?);
    let replies =
        host.engine
            .query_transport_contributions(&principal, "fixture/caller", &payload)?;
    anyhow::ensure!(replies.len() == 1, "expected one response");
    Ok(serde_json::from_slice(&replies[0])?)
}
fn wait_for(
    host: &mut HostRuntime,
    consumer: &ComponentRef,
    count: usize,
) -> anyhow::Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        host.poll_runtime()?;
        let value = query(host, consumer, json!({"op":"resource-state"}))?;
        if value["count"] == count {
            return Ok(value);
        }
        anyhow::ensure!(Instant::now() < deadline, "resource task deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn reference(value: &Value) -> UserResourceRef {
    UserResourceRef {
        id: value["id"].as_str().unwrap().into(),
        size: value["size"].as_u64().unwrap(),
        media_type: value["mediaType"].as_str().unwrap().into(),
        name: value["name"].as_str().map(str::to_string),
    }
}
#[test]
fn test_user_resource_transfer_world_stop_preserves_other_world_and_unclaimed() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let backend = install(root.path(), &home, "transfer-backend", BACKEND)?;
    let consumer = install(root.path(), &home, "transfer-consumer", CONSUMER)?;
    let default_backend =
        home.select_default_world_stored_rtw(&backend.activation.digest, Some(true))?;
    home.select_default_world_stored_rtw(&consumer.activation.digest, Some(true))?;
    let scope = RuntimeScopeId::new(crate::DEFAULT_WORLD_SCOPE);
    let owner = ComponentRef::new(default_backend.instance_id, "runtime");
    for permission in [
        RuntimePermission::BackgroundTask,
        RuntimePermission::UserResourceImport,
        RuntimePermission::UserResourceRead,
        RuntimePermission::AssetImport,
        RuntimePermission::AssetRead,
    ] {
        home.grant_runtime_permission(scope.clone(), owner.clone(), permission)?;
    }
    let a = home.create_world()?.id;
    let b = home.create_world()?.id;
    let refs = |world| -> anyhow::Result<(ComponentRef, ComponentRef)> {
        let activations = home.list_world_activations(world)?;
        let find = |subject: &str| {
            activations
                .iter()
                .find(|v| v.subject == subject)
                .unwrap()
                .instance_id
                .clone()
        };
        Ok((
            ComponentRef::new(find(&backend.activation.subject), "runtime"),
            ComponentRef::new(find(&consumer.activation.subject), "runtime"),
        ))
    };
    let (_backend_a, consumer_a) = refs(a)?;
    let (backend_b, consumer_b) = refs(b)?;
    let mut host = HostRuntime::start(&home)?;
    host.activate_world(&home, a)?;
    host.activate_world(&home, b)?;
    query(
        &host,
        &consumer_a,
        json!({"op":"resource-fill","bytes":16*1024*1024,"count":1}),
    )?;
    let state_a = wait_for(&mut host, &consumer_a, 1)?;
    query(
        &host,
        &consumer_b,
        json!({"op":"resource-fill","bytes":8*1024*1024,"count":1}),
    )?;
    let state_b = wait_for(&mut host, &consumer_b, 1)?;
    let broker = host.host_access.as_ref().unwrap().user_resources.clone();
    let unclaimed = broker
        .import(&vec![0; 8 * 1024 * 1024], "application/octet-stream", None)
        .unwrap();
    assert_eq!(
        broker.import(b"x", "text/plain", None),
        Err(HostAccessError::QueueFull)
    );
    let old_a = reference(&state_a["originals"][0]);
    let owned_b = reference(&state_b["owned"][0]);
    let principal_b = host.engine.active_component_principal(&backend_b)?;
    host.deactivate_world(a)?;
    let replacement = broker
        .import(&vec![0; 16 * 1024 * 1024], "application/octet-stream", None)
        .unwrap();
    assert_eq!(
        broker
            .read_for(&owned_b, 8 * 1024 * 1024, Some(&principal_b))
            .unwrap()
            .len(),
        8 * 1024 * 1024
    );
    assert_eq!(
        broker.read(&unclaimed, 8 * 1024 * 1024).unwrap().len(),
        8 * 1024 * 1024
    );
    assert_eq!(broker.read(&old_a, 1), Err(HostAccessError::NotFound));
    broker.release(&replacement).unwrap();
    host.shutdown()?;
    Ok(())
}

#[test]
fn test_user_resource_transfer_task_trap_reclaims_quota() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let backend = install(root.path(), &home, "trap-backend", BACKEND)?;
    let consumer = install(root.path(), &home, "trap-consumer", CONSUMER)?;
    let default_backend =
        home.select_default_world_stored_rtw(&backend.activation.digest, Some(true))?;
    home.select_default_world_stored_rtw(&consumer.activation.digest, Some(true))?;
    let scope = RuntimeScopeId::new(crate::DEFAULT_WORLD_SCOPE);
    let owner = ComponentRef::new(default_backend.instance_id, "runtime");
    for permission in [
        RuntimePermission::BackgroundTask,
        RuntimePermission::UserResourceImport,
        RuntimePermission::UserResourceRead,
        RuntimePermission::AssetImport,
        RuntimePermission::AssetRead,
    ] {
        home.grant_runtime_permission(scope.clone(), owner.clone(), permission)?;
    }
    let world = home.create_world()?.id;
    let activations = home.list_world_activations(world)?;
    let consumer = ComponentRef::new(
        activations
            .iter()
            .find(|v| v.subject == consumer.activation.subject)
            .unwrap()
            .instance_id
            .clone(),
        "runtime",
    );
    let mut host = HostRuntime::start(&home)?;
    host.activate_world(&home, world)?;
    query(
        &host,
        &consumer,
        json!({"op":"resource-fill","bytes":16*1024*1024,"count":1}),
    )?;
    let state = wait_for(&mut host, &consumer, 1)?;
    let owned = reference(&state["owned"][0]);
    let broker = host.host_access.as_ref().unwrap().user_resources.clone();
    let remaining = broker
        .import(&vec![0; 16 * 1024 * 1024], "application/octet-stream", None)
        .unwrap();
    assert_eq!(
        broker.import(b"x", "text/plain", None),
        Err(HostAccessError::QueueFull)
    );
    query(&host, &consumer, json!({"op":"resource-trap"}))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let _ = host.poll_runtime();
        if !broker
            .state
            .lock()
            .unwrap()
            .resources
            .contains_key(&owned.id)
        {
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "trap cleanup deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    broker
        .import(&vec![0; 16 * 1024 * 1024], "application/octet-stream", None)
        .unwrap();
    assert!(broker.read(&remaining, 16 * 1024 * 1024).is_ok());
    let _ = host.shutdown();
    Ok(())
}
