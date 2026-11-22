//! P09 acceptance at the existing Host/WASM lifecycle boundary.
use super::*;

#[test]
fn test_system_endpoints_share_one_store_across_schemas_separate_from_started_guest()
-> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let ids = fixture.call(json!({"op":"ids","count":3}))?["ids"].clone();
    for (schema, expected) in [
        ("fixture.system-counter-a@1", 1),
        ("fixture.system-counter-b@1", 2),
    ] {
        // Ask Engine for a fresh endpoint each time, as Host does per schema.
        let endpoint = fixture.engine.world_system_endpoint(&fixture.backend)?;
        endpoint.evaluate(
            fixture
                .runtime
                .as_ref()
                .unwrap()
                .client()
                .snapshot_for_instance(fixture.backend.instance_id.clone())?,
            &WorldCommand::system(
                schema.parse()?,
                json!({"expected":expected,"item":ids[expected as usize-1]}),
            ),
        )?;
    }
    let reply=fixture.call(json!({"op":"submit","schema":"fixture.system-counter-a@1","payload":{"expected":3,"item":ids[2]}}))?;
    fixture.flush()?;
    assert_eq!(
        fixture.poll(reply["ticket"].as_u64().unwrap())?["state"],
        "committed"
    );
    Ok(())
}

#[test]
fn test_host_world_admission_refuses_ninth_before_activation_and_reuses_freed_slot()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let worlds = (0..9)
        .map(|_| home.create_world().map(|world| world.id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut host = crate::HostRuntime::start(&home)?;
    for world in &worlds[..8] {
        host.activate_world(&home, *world)?;
    }
    assert!(matches!(
        host.activate_world(&home, worlds[8]),
        Err(crate::HostError::ActiveWorldLimitExceeded)
    ));
    assert!(!host.is_world_active(worlds[8]));
    // Failure publishes neither worker/writer nor topology for the refused World.
    drop(home.open_world_storage(worlds[8])?);
    host.deactivate_world(worlds[0])?;
    host.activate_world(&home, worlds[8])?;
    host.shutdown()?;
    assert!(matches!(
        host.activate_world(&home, worlds[0]),
        Err(crate::HostError::HostStopping)
    ));
    host.shutdown()?; // Completed cleanup remains idempotent.
    Ok(())
}

#[test]
fn test_busy_world_shutdown_retains_writer_drains_guest_commits_and_preserves_other_world()
-> anyhow::Result<()> {
    use crate::HostError;
    use crate::runtime::ActiveWorld;
    let mut fixture = Fixture::new(128)?;
    let other = fixture.home.create_world()?.id;
    let ids = fixture.call(json!({"op":"ids","count":2}))?["ids"].clone();
    let pause = fixture.pause();
    let first = fixture.submit("fixture.create@1", ids[0].as_str().unwrap().parse()?)?;
    pause.wait()?;
    let second = fixture.submit("fixture.create@1", ids[1].as_str().unwrap().parse()?)?;
    assert_eq!(fixture.poll(first)?["state"], "pending");
    assert_eq!(fixture.poll(second)?["state"], "pending");
    let mut host = crate::HostRuntime::start(&fixture.home)?;
    host.engine = std::mem::take(&mut fixture.engine);
    host.world_data = fixture.data.clone();
    let client = fixture.runtime.as_ref().unwrap().client();
    let instances = vec![
        fixture.backend.instance_id.clone(),
        fixture.consumer.instance_id.clone(),
    ];
    host.active_worlds.insert(
        fixture.world,
        ActiveWorld {
            runtime: fixture.runtime.take().unwrap(),
            started_instances: instances.clone(),
            registered_instances: instances,
        },
    );
    host.activate_world(&fixture.home, other)?;
    assert!(matches!(
        host.deactivate_world_with_timeout(fixture.world, Duration::from_millis(10)),
        Err(HostError::WorldRuntime(
            rintawa_world_runtime::WorldRuntimeError::ShutdownBusy
        ))
    ));
    assert!(host.is_world_active(fixture.world));
    assert!(matches!(
        fixture.home.open_world_storage(fixture.world),
        Err(HostError::Storage(
            rintawa_storage::StorageError::WriterBusy(_)
        ))
    ));
    assert!(matches!(
        client.submit(WorldCommand::system(
            "fixture.create@1".parse()?,
            json!({"item":ItemId::new()})
        )),
        Err(rintawa_world_runtime::WorldRuntimeError::RuntimeStopped)
    ));
    assert!(matches!(
        host.engine
            .query_transport_contributions(&fixture.consumer, "fixture/caller", b"blocked"),
        Err(TransportError::Inactive)
    ));
    host.active_worlds[&other]
        .runtime
        .client()
        .set_manifest_metadata(json!({"title":"still working"}))?;
    pause.resume();
    // No native domain write: both admitted guest commands must COMMIT during drain.
    host.deactivate_world(fixture.world)?;
    let storage = fixture.home.open_world_storage(fixture.world)?;
    for id in ids.as_array().unwrap() {
        assert!(storage.load_item(id.as_str().unwrap().parse()?)?.is_some());
    }
    drop(storage);
    assert!(host.is_world_active(other));
    host.deactivate_world(other)?;
    host.shutdown()?;
    Ok(())
}
