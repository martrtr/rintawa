//! P08 graph/entropy/media acceptance using the existing real RTW fixture.

use super::*;
use rintawa_sdk::runtime_permissions::RuntimePermission;

fn ids(fixture: &Fixture) -> anyhow::Result<Vec<String>> {
    Ok(serde_json::from_value(
        fixture.call(json!({"op":"ids","count":3}))?["ids"].clone(),
    )?)
}
fn submit_payload(fixture: &Fixture, schema: &str, payload: Value) -> anyhow::Result<u64> {
    let reply = fixture.call(json!({"op":"submit","schema":schema,"payload":payload}))?;
    reply["ticket"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("submit: {reply}"))
}
fn commit_payload(fixture: &Fixture, schema: &str, payload: Value) -> anyhow::Result<()> {
    let ticket = submit_payload(fixture, schema, payload)?;
    fixture.flush()?;
    let outcome = fixture.poll(ticket)?;
    assert_eq!(outcome["state"], "committed", "{schema}: {outcome}");
    Ok(())
}
fn graph(fixture: &Fixture) -> anyhow::Result<Vec<String>> {
    let ids = ids(fixture)?;
    commit_payload(fixture, "fixture.graph@1", json!({"ids":ids}))?;
    Ok(ids)
}
fn batch(fixture: &Fixture, ids: &[String], keys: &[&str]) -> anyhow::Result<Value> {
    fixture.call(json!({"op":"batch","items":ids,"keys":keys}))
}

#[test]
fn test_guest_creates_and_queries_graph_with_host_ids_and_no_native_domain_seed()
-> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let ids = ids(&fixture)?;
    for id in &ids {
        let bytes = id.parse::<ItemId>()?.into_bytes();
        assert_eq!(bytes[6] >> 4, 7);
    }
    assert!(
        batch(&fixture, &ids, &["fixture.value@1"])?["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    commit_payload(&fixture, "fixture.graph@1", json!({"ids":ids}))?;
    let values = batch(
        &fixture,
        &[
            ids[1].clone(),
            ids[0].clone(),
            ids[0].clone(),
            ids[2].clone(),
        ],
        &["fixture.value@1", "fixture.value@1"],
    )?;
    assert_eq!(values["items"].as_array().unwrap().len(), 3);
    assert_eq!(values["properties"].as_array().unwrap().len(), 2);
    let actual: Vec<_> = values["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    let mut expected: Vec<_> = ids.iter().map(String::as_str).collect();
    expected.sort();
    assert_eq!(actual, expected);
    assert!(
        batch(&fixture, &ids, &["fixture.label@2"])?["properties"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.call(json!({"op":"property","item":ids[0],"key":"fixture.label@2"}))?,
        json!({"error":"invalid-request"})
    );
    for (target, direction, count) in [
        (&ids[0], "outgoing", 1),
        (&ids[0], "incoming", 0),
        (&ids[1], "incoming", 1),
        (&ids[0], "either", 1),
    ] {
        let result=fixture.call(json!({"op":"relations","targets":[target,target],"direction":direction,"property":"fixture.edge@1","limit":1}))?;
        assert_eq!(result["records"].as_array().unwrap().len(), count);
        if count == 1 {
            let relation: Value = serde_json::from_str(result["records"][0].as_str().unwrap())?;
            assert_eq!(relation["id"], ids[2]);
        }
    }
    assert_eq!(fixture.call(json!({"op":"relations","targets":[ids[0]],"direction":"either","after":ids[2],"limit":256}))?,json!({"records":[]}));
    let page = fixture.call(json!({"op":"properties","key":"fixture.value@1","limit":1}))?;
    let record: Value = serde_json::from_str(page["records"][0].as_str().unwrap())?;
    let next = fixture.call(
        json!({"op":"properties","key":"fixture.value@1","after":record["item"],"limit":256}),
    )?;
    assert_eq!(next["records"].as_array().unwrap().len(), 1);
    let empty: Vec<String> = Vec::new();
    assert_eq!(
        batch(&fixture, &empty, &[])?,
        json!({"items":[],"properties":[]})
    );
    // Pinned System reads use the original snapshot even after a later COMMIT.
    let snapshot = fixture
        .runtime
        .as_ref()
        .unwrap()
        .client()
        .snapshot_for_instance(fixture.backend.instance_id.clone())?;
    snapshot.read(&[ids[0].parse()?], &["fixture.value@1".parse()?])?;
    commit_payload(
        &fixture,
        "fixture.pair@1",
        json!({"ids":&ids[..2],"value":42}),
    )?;
    let endpoint = fixture.engine.world_system_endpoint(&fixture.backend)?;
    endpoint.evaluate(
        snapshot,
        &WorldCommand::system("fixture.pinned@1".parse()?, json!({"ids":ids,"expected":7})),
    )?;
    for property in batch(&fixture, &ids, &["fixture.value@1"])?["properties"]
        .as_array()
        .unwrap()
    {
        assert_eq!(property["data"], 42);
    }
    Ok(())
}

#[test]
fn test_guest_graph_rejects_foreign_keys_invalid_inputs_and_preserves_world_isolation()
-> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let ids = graph(&fixture)?;
    let other = Fixture::new(128)?;
    assert_eq!(
        batch(&other, &ids, &["fixture.value@1"])?["items"],
        json!([])
    );
    assert_eq!(
        batch(&fixture, &[], &["foreign.key@1"])?,
        json!({"error":"access-denied"})
    );
    assert_eq!(fixture.call(json!({"op":"relations","targets":[],"direction":"either","property":"foreign.key@1","limit":1}))?,json!({"error":"access-denied"}));
    assert_eq!(
        fixture.call(json!({"op":"properties","key":"foreign.key@1","limit":1}))?,
        json!({"error":"access-denied"})
    );
    for count in [0, 65] {
        assert_eq!(
            fixture.call(json!({"op":"ids","count":count}))?,
            json!({"error":"invalid-request"})
        );
    }
    for request in [
        json!({"op":"batch","items":vec![ids[0].clone();65],"keys":[]}),
        json!({"op":"batch","items":[],"keys":vec!["fixture.value@1";65]}),
        json!({"op":"batch","items":["not-a-uuid"],"keys":[]}),
        json!({"op":"batch","items":[],"keys":["bad@0"]}),
        json!({"op":"relations","targets":vec![ids[0].clone();65],"direction":"either","limit":1}),
        json!({"op":"relations","targets":[],"direction":"either","limit":0}),
        json!({"op":"relations","targets":[],"direction":"either","limit":257}),
        json!({"op":"relations","targets":[],"direction":"either","after":"bad","limit":1}),
        json!({"op":"properties","key":"fixture.value@1","after":"bad","limit":1}),
    ] {
        assert_eq!(
            fixture.call(request.clone())?,
            json!({"error":"invalid-request"}),
            "{request}"
        );
    }
    let v4 = String::from("00000000-0000-4000-8000-000000000001");
    assert_eq!(
        batch(&fixture, &[v4], &[])?,
        json!({"items":[],"properties":[]})
    );
    assert_eq!(
        fixture.call(json!({"op":"sized-input","size":1024*1024+1}))?,
        json!({"error":"message-too-large"})
    );
    Ok(())
}

#[test]
fn test_guest_result_limits_include_array_encoding_and_page_escaping() -> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let ids = graph(&fixture)?;
    for id in &ids[..2] {
        commit_payload(
            &fixture,
            "fixture.set-label@1",
            json!({"item":id,"size":530000}),
        )?;
    }
    assert_eq!(
        fixture.call(json!({"op":"batch-summary","items":&ids[..2],"keys":["fixture.label@1"]}))?,
        json!({"error":"message-too-large"})
    );
    // Raw JSON properties fit; the escaped list<string> representation does not.
    for id in &ids[..2] {
        commit_payload(
            &fixture,
            "fixture.set-label@1",
            json!({"item":id,"size":132000,"escaped":true}),
        )?;
    }
    let summary =
        fixture.call(json!({"op":"batch-summary","items":&ids[..2],"keys":["fixture.label@1"]}))?;
    assert!(summary["encoded_bytes"].as_u64().unwrap() < 1024 * 1024);
    assert_eq!(
        fixture.call(json!({"op":"properties","key":"fixture.label@1","limit":256}))?,
        json!({"error":"message-too-large"})
    );
    commit_payload(&fixture, "fixture.system-limits@1", json!({"item":ids[0]}))?;
    Ok(())
}

#[test]
fn test_guest_entropy_has_exact_count_and_callback_limits() -> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    assert_eq!(
        fixture.call(json!({"op":"entropy","count":16}))?["lengths"],
        json!([16])
    );
    for count in [0, 257] {
        assert_eq!(
            fixture.call(json!({"op":"entropy","count":count}))?["error"],
            "RandomError::InvalidRequest"
        );
    }
    let result = fixture.call(json!({"op":"entropy","count":256,"calls":17}))?;
    assert_eq!(result["lengths"].as_array().unwrap().len(), 16);
    assert_eq!(result["error"], "RandomError::InvalidRequest");
    assert!(fixture.call(json!({"op":"entropy","count":256}))?["error"].is_null());
    Ok(())
}

fn grant(fixture: &Fixture, permission: RuntimePermission) -> anyhow::Result<()> {
    fixture
        .engine
        .grant_requested_runtime_permission_for_instance(
            &fixture.backend.instance_id,
            &fixture.backend.component_id,
            permission,
        )?;
    Ok(())
}
fn open(fixture: &Fixture, reference: &Value) -> anyhow::Result<u64> {
    let result = fixture.call(json!({"op":"asset-open","reference":reference}))?;
    result["handle"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("asset open: {result}"))
}
fn read_all(fixture: &Fixture, handle: u64) -> anyhow::Result<usize> {
    let mut length = 0;
    loop {
        let result = fixture.call(json!({"op":"asset-read","handle":handle,"maximum":65536}))?;
        let count = result["length"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("read: {result}"))?;
        assert_eq!(result["sum"].as_u64().unwrap(), count * 7);
        if count == 0 {
            return Ok(length);
        }
        length += count as usize;
    }
}

#[test]
fn test_guest_asset_read_requires_permission_and_bounds_handles_and_cleanup() -> anyhow::Result<()>
{
    let mut fixture = Fixture::with_permissions(128, &[RuntimePermission::AssetImport])?;
    let reference = fixture.call(json!({"op":"asset-ref"}))?;
    assert!(reference["digest"].is_string());
    assert_eq!(
        fixture.call(json!({"op":"asset-open","reference":reference}))?,
        json!({"error":"permission-denied"})
    );
    grant(&fixture, RuntimePermission::AssetRead)?;
    let handle = open(&fixture, &reference)?;
    assert_eq!(
        fixture.call(json!({"op":"asset-read","handle":handle,"maximum":0}))?,
        json!({"error":"invalid-asset"})
    );
    assert_eq!(
        fixture.call(json!({"op":"asset-read","handle":handle,"maximum":65537}))?,
        json!({"error":"message-too-large"})
    );
    assert_eq!(read_all(&fixture, handle)?, 1024 * 1024 + 17);
    assert_eq!(
        fixture.call(json!({"op":"asset-close","handle":handle}))?,
        json!({"closed":true})
    );
    assert_eq!(
        fixture.call(json!({"op":"asset-close","handle":handle}))?,
        json!({"error":"invalid-asset"})
    );
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(open(&fixture, &reference)?);
    }
    assert_eq!(
        fixture.call(json!({"op":"asset-open","reference":reference}))?,
        json!({"error":"unavailable"})
    );
    let other = Fixture::new(128)?;
    grant(&other, RuntimePermission::AssetRead)?;
    assert_eq!(
        other.call(json!({"op":"asset-read","handle":handles[0],"maximum":1}))?,
        json!({"error":"invalid-asset"})
    );
    fixture
        .engine
        .stop_extension_instance(&fixture.backend.instance_id)?;
    fixture
        .engine
        .start_extension_instance(&fixture.backend.instance_id)?;
    assert_eq!(
        fixture.call(json!({"op":"asset-read","handle":handles[0],"maximum":1}))?,
        json!({"error":"invalid-asset"})
    );
    let fresh = open(&fixture, &reference)?;
    assert!(!handles.contains(&fresh));
    fixture.reload()?;
    grant(&fixture, RuntimePermission::AssetRead)?;
    assert_eq!(
        fixture.call(json!({"op":"asset-read","handle":fresh,"maximum":1}))?,
        json!({"error":"invalid-asset"})
    );
    let current = open(&fixture, &reference)?;
    assert!(matches!(
        fixture.engine.query_transport_contributions(
            &fixture.consumer,
            "fixture/commands",
            br#"{"op":"trap"}"#
        ),
        Err(TransportError::Unavailable)
    ));
    fixture.reload()?;
    grant(&fixture, RuntimePermission::AssetRead)?;
    assert_eq!(
        fixture.call(json!({"op":"asset-read","handle":current,"maximum":1}))?,
        json!({"error":"invalid-asset"})
    );
    Ok(())
}

#[test]
fn test_guest_reopens_persisted_asset_reference_after_full_host_restart() -> anyhow::Result<()> {
    use crate::HostRuntime;
    let root = tempfile::tempdir()?;
    let path = root.path().join("home");
    let home = HostHome::open(&path)?;
    let world = home.create_world()?.id;
    let backend_digest = artifact(root.path(), &home, "backend", BACKEND)?;
    let consumer_digest = artifact(root.path(), &home, "consumer", CONSUMER)?;
    let backend = home.select_world_stored_rtw(world, &backend_digest, Some(true))?;
    let consumer = home.select_world_stored_rtw(world, &consumer_digest, Some(true))?;
    let scope = world_runtime_scope_id(world);
    let owner = ComponentRef::new(backend.instance_id.clone(), "runtime");
    for permission in [
        RuntimePermission::AssetImport,
        RuntimePermission::AssetRead,
        RuntimePermission::UserResourceImport,
        RuntimePermission::UserResourceRead,
    ] {
        home.grant_runtime_permission(scope.clone(), owner.clone(), permission)?;
    }
    let call = |host: &HostRuntime, request: Value| -> anyhow::Result<Value> {
        let principal = host.engine.active_component_principal(&ComponentRef::new(
            consumer.instance_id.clone(),
            "runtime",
        ))?;
        let mut payload = b"forward:fixture/commands\n".to_vec();
        payload.extend(serde_json::to_vec(&request)?);
        let replies =
            host.engine
                .query_transport_contributions(&principal, "fixture/caller", &payload)?;
        anyhow::ensure!(replies.len() == 1, "missing backend");
        Ok(serde_json::from_slice(&replies[0])?)
    };
    let mut host = HostRuntime::start(&home)?;
    host.activate_world(&home, world)?;
    let reference = call(&host, json!({"op":"asset-ref"}))?;
    let png = include_bytes!("../../../../extension-engine/tests/fixtures/world-system/pixel.png");
    assert_eq!(reference["size"], png.len());
    let ids = call(&host, json!({"op":"ids","count":3}))?["ids"].clone();
    let ticket = call(
        &host,
        json!({"op":"submit","schema":"fixture.graph@1","payload":{"ids":ids,"asset":reference}}),
    )?["ticket"]
        .as_u64()
        .unwrap();
    host.active_worlds[&world]
        .runtime
        .client()
        .set_manifest_metadata(json!({"title":"Asset acceptance"}))?;
    assert_eq!(
        call(&host, json!({"op":"poll","ticket":ticket}))?["state"],
        "committed"
    );
    let old_handle = call(&host, json!({"op":"asset-open","reference":reference}))?["handle"]
        .as_u64()
        .unwrap();
    host.shutdown()?;
    drop(home);

    let home = HostHome::open(&path)?;
    // Persisted bytes and graph reference must suffice: no second import.
    home.revoke_runtime_permission(&scope, &owner, RuntimePermission::AssetImport)?;
    let mut host = HostRuntime::start(&home)?;
    host.activate_world(&home, world)?;
    assert_eq!(
        call(&host, json!({"op":"asset-ref"}))?["error"],
        "not-imported"
    );
    let saved = call(
        &host,
        json!({"op":"batch","items":[ids[0]],"keys":["fixture.asset@1"]}),
    )?["properties"][0]["data"]
        .clone();
    assert_eq!(saved, reference);
    assert_eq!(
        call(
            &host,
            json!({"op":"asset-read","handle":old_handle,"maximum":1})
        )?,
        json!({"error":"invalid-asset"})
    );
    let handle = call(&host, json!({"op":"asset-open","reference":saved}))?["handle"]
        .as_u64()
        .unwrap();
    assert_ne!(handle, old_handle);
    let mut total = 0;
    loop {
        let chunk = call(
            &host,
            json!({"op":"asset-read","handle":handle,"maximum":65536}),
        )?;
        let count = chunk["length"].as_u64().unwrap();
        assert_eq!(
            chunk["sum"],
            if count == 0 {
                0
            } else {
                png.iter().map(|byte| u64::from(*byte)).sum::<u64>()
            }
        );
        total += count;
        if count == 0 {
            break;
        }
    }
    assert_eq!(total, png.len() as u64);
    assert_eq!(
        call(&host, json!({"op":"asset-close","handle":handle}))?,
        json!({"closed":true})
    );
    host.shutdown()?;
    Ok(())
}

#[test]
fn test_guest_asset_open_rejects_corrupt_missing_and_traversal_references() -> anyhow::Result<()> {
    let fixture = Fixture::with_permissions(
        128,
        &[RuntimePermission::AssetImport, RuntimePermission::AssetRead],
    )?;
    let reference = fixture.call(json!({"op":"asset-ref"}))?;
    for (field, value) in [
        ("size", json!(reference["size"].as_u64().unwrap() + 1)),
        ("media_type", json!("not a media type")),
        ("digest", json!("../../secret")),
        ("digest", json!(format!("sha256:{}", "0".repeat(64)))),
    ] {
        let mut invalid = reference.clone();
        invalid[field] = value;
        assert_eq!(
            fixture.call(json!({"op":"asset-open","reference":invalid}))?,
            json!({"error":"invalid-asset"})
        );
    }
    let path = fixture.home.asset_store().root().join("sha256").join(
        reference["digest"]
            .as_str()
            .unwrap()
            .strip_prefix("sha256:")
            .unwrap(),
    );
    // Storage fault injection, never domain seeding.
    std::fs::write(path, vec![9; reference["size"].as_u64().unwrap() as usize])?;
    assert_eq!(
        fixture.call(json!({"op":"asset-open","reference":reference}))?,
        json!({"error":"invalid-asset"})
    );
    Ok(())
}
