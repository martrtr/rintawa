//! Real WASM fixture for authoritative World System execution.
wit_bindgen::generate!({ path: "wit", world: "command-ticket-fixture" });

struct Fixture;

fn dummy_resource() -> rintawa::engine::user_resources::ResourceRef {
    rintawa::engine::user_resources::ResourceRef {
        id: "missing".into(),
        size: 1,
        media_type: "application/octet-stream".into(),
        name: None,
    }
}

#[derive(Default)]
struct ResourceProbe {
    pending: Vec<usize>,
    owned: Vec<rintawa::engine::user_resources::ResourceRef>,
    originals: Vec<rintawa::engine::user_resources::ResourceRef>,
    trap: bool,
}
static RESOURCE_PROBE: std::sync::Mutex<Option<ResourceProbe>> = std::sync::Mutex::new(None);

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        assert!(rintawa::engine::user_resources::take_resource(&dummy_resource()).is_err());
        use rintawa::engine::world_registration::{self, SchemaKind};
        world_registration::register_world_schema(
            "fixture.value",
            1,
            SchemaKind::Property,
            r#"{"type":"integer"}"#,
        )
        .expect("value property schema");
        for (name, version, schema) in [
            ("fixture.label", 1, r#"{"type":"string"}"#),
            ("fixture.label", 2, r#"{"type":"string"}"#),
            ("fixture.edge", 1, r#"{"type":"boolean"}"#),
            ("fixture.asset", 1, r#"{"type":"object"}"#),
        ] {
            world_registration::register_world_schema(name, version, SchemaKind::Property, schema)
                .unwrap();
        }
        for command in [
            "fixture.graph",
            "fixture.set-label",
            "fixture.pinned",
            "fixture.pair",
            "fixture.system-limits",
            "fixture.system-counter-a",
            "fixture.system-counter-b",
        ] {
            world_registration::register_world_schema(
                command,
                1,
                SchemaKind::Command,
                r#"{"type":"object"}"#,
            )
            .unwrap();
        }
        assert!(rintawa::engine::host::random_bytes(16).is_err());
        assert!(rintawa::engine::host::monotonic_millis().is_err());
        for command in [
            "fixture.commit",
            "fixture.reject",
            "fixture.foreign",
            "fixture.create",
            "fixture.invalid",
            "fixture.conflict",
        ] {
            world_registration::register_world_schema(
                command,
                1,
                SchemaKind::Command,
                r#"{"type":"object","properties":{"item":{"type":"string"}},"required":["item"],"additionalProperties":false}"#,
            )
            .expect("command schema");
        }
    }

    fn start() {
        assert!(rintawa::engine::user_resources::take_resource(&dummy_resource()).is_err());
        *RESOURCE_PROBE.lock().unwrap() = Some(ResourceProbe::default());
        let _ = rintawa::engine::runtime_tasks::spawn_periodic(10);
        rintawa::engine::host::monotonic_millis().expect("start clock");
        LIFECYCLE_STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(rintawa::engine::host::random_bytes(16).unwrap().len(), 16);
        assert!(rintawa::engine::world_data::new_item_ids(1).is_err());
        if let Ok(reference) = import_fixture_asset() {
            *IMPORTED_ASSET.lock().unwrap() = Some(reference);
        }
        rintawa::engine::transport::declare("fixture/commands").expect("command point");
    }
    fn stop() {
        use rintawa::engine::user_resources;
        assert!(user_resources::take_resource(&dummy_resource()).is_err());
        if let Some(probe) = RESOURCE_PROBE.lock().unwrap().as_ref() {
            for resource in &probe.owned {
                assert!(user_resources::read_resource(resource, resource.size).is_err());
                assert!(user_resources::release_resource(resource).is_err());
            }
        }
        assert!(rintawa::engine::host::monotonic_millis().is_err());
        assert!(rintawa::engine::host::random_bytes(16).is_err());
        if let Some(ticket) = *LAST_TICKET.lock().unwrap() {
            assert!(matches!(
                rintawa::engine::world_data::close(ticket),
                Err(rintawa::engine::world_data::Error::AccessNotActive)
            ));
        }
    }
}

impl exports::rintawa::engine::world_system::Guest for Fixture {
    fn evaluate(
        command_json: String,
    ) -> Result<String, exports::rintawa::engine::world_system::Error> {
        use rintawa::engine::preferences::{self, PublishedWriteError};

        assert!(matches!(
            preferences::compare_exchange_published_preference(None, Some("forbidden")),
            Err(PublishedWriteError::AccessNotActive)
        ));
        assert!(rintawa::engine::user_resources::take_resource(&dummy_resource()).is_err());
        let command: serde_json::Value = serde_json::from_str(&command_json).map_err(|error| {
            exports::rintawa::engine::world_system::Error::Failed(error.to_string())
        })?;
        assert!(rintawa::engine::host::monotonic_millis().is_err());
        assert!(rintawa::engine::host::random_bytes(16).is_err());
        assert!(rintawa::engine::world_data::new_item_ids(1).is_err());
        if let Some(result) = graph_system(&command) {
            return result;
        }
        let item = command["payload"]["item"].as_str().ok_or_else(|| {
            exports::rintawa::engine::world_system::Error::Failed(String::from("missing item"))
        })?;
        match command["schema"].as_str() {
            Some("fixture.reject@1") => Err(exports::rintawa::engine::world_system::Error::Rejected(
                String::from("fixture rollback"),
            )),
            Some("fixture.commit@1") => {
                // This read is only valid while Host has installed the pinned snapshot.
                let _ = rintawa::engine::world_system_snapshot::load_item(item)
                    .map_err(|error| exports::rintawa::engine::world_system::Error::Failed(format!("snapshot read failed: {error:?}")))?;
                Ok(serde_json::json!({"changes":[{
                    "operation":"set-property", "item":item,
                    "key":{"id":"fixture.value","version":1}, "data":7
                }]}).to_string())
            }
            Some("fixture.create@1") => Ok(serde_json::json!({"changes":[
                {"operation":"create-object","id":item},
                {"operation":"set-property","item":item,"key":{"id":"fixture.value","version":1},"data":7}
            ]}).to_string()),
            Some("fixture.invalid@1") => Ok(serde_json::json!({"changes":[
                {"operation":"set-property","item":item,"key":{"id":"fixture.value","version":1},"data":"invalid integer"}
            ]}).to_string()),
            Some("fixture.conflict@1") => Ok(serde_json::json!({"changes":[
                {"operation":"set-property","item":item,"key":{"id":"fixture.value","version":1},"data":99},
                {"operation":"create-object","id":item}
            ]}).to_string()),
            Some("fixture.foreign@1") => Ok(serde_json::json!({"changes":[{
                "operation":"set-property", "item":item,
                "key":{"id":"fixture.foreign-property","version":1}, "data":9
            }]}).to_string()),
            _ => Err(exports::rintawa::engine::world_system::Error::Failed(String::from("unexpected command"))),
        }
    }
}

static LIFECYCLE_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SYSTEM_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn graph_system(
    command: &serde_json::Value,
) -> Option<Result<String, exports::rintawa::engine::world_system::Error>> {
    use rintawa::engine::world_system_snapshot as snapshot;
    let payload = &command["payload"];
    let set = |item: &str, name: &str, value: serde_json::Value| serde_json::json!({"operation":"set-property","item":item,"key":{"id":name,"version":1},"data":value});
    let changes = match command["schema"].as_str()? {
        "fixture.system-counter-a@1" | "fixture.system-counter-b@1" => {
            assert!(!LIFECYCLE_STARTED.load(std::sync::atomic::Ordering::Relaxed));
            let count = SYSTEM_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            assert_eq!(count, payload["expected"].as_u64().unwrap());
            return Some(Ok(
                serde_json::json!({"changes":[{"operation":"create-object","id":payload["item"]}]})
                    .to_string(),
            ));
        }
        "fixture.graph@1" => {
            let ids: Vec<String> = serde_json::from_value(payload["ids"].clone()).unwrap();
            assert_eq!(ids.len(), 3);
            let before: serde_json::Value =
                serde_json::from_str(&snapshot::read(&ids, &["fixture.value@1".into()]).unwrap())
                    .unwrap();
            assert!(before["items"].as_array().unwrap().is_empty());
            assert!(
                snapshot::find_relations(&ids, snapshot::Direction::Either, None, None, 256)
                    .unwrap()
                    .is_empty()
            );
            let mut changes = vec![
                serde_json::json!({"operation":"create-object","id":ids[0]}),
                serde_json::json!({"operation":"create-object","id":ids[1]}),
                serde_json::json!({"operation":"create-relation","id":ids[2],"subject":ids[0],"object":ids[1]}),
                set(&ids[0], "fixture.value", serde_json::json!(7)),
                set(&ids[1], "fixture.value", serde_json::json!(7)),
                set(&ids[0], "fixture.label", serde_json::json!("first")),
                set(&ids[1], "fixture.label", serde_json::json!("second")),
                set(&ids[2], "fixture.edge", serde_json::json!(true)),
            ];
            if let Some(reference) = payload.get("asset") {
                changes.push(set(&ids[0], "fixture.asset", reference.clone()));
            }
            changes
        }
        "fixture.set-label@1" => {
            // Avoid spending the default System CPU budget walking a large JSON
            // Value: the fixture generates only these two known JSON characters.
            let unit = if payload["escaped"] == true {
                "\\\""
            } else {
                "x"
            };
            let data = unit.repeat(payload["size"].as_u64().unwrap() as usize);
            let item = payload["item"].as_str().unwrap();
            return Some(Ok(format!(
                r#"{{"changes":[{{"operation":"set-property","item":"{item}","key":{{"id":"fixture.label","version":1}},"data":"{data}"}}]}}"#
            )));
        }
        "fixture.pair@1" => payload["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| {
                set(
                    id.as_str().unwrap(),
                    "fixture.value",
                    payload["value"].clone(),
                )
            })
            .collect(),
        "fixture.pinned@1" => {
            let ids: Vec<String> = serde_json::from_value(payload["ids"].clone()).unwrap();
            let batch: serde_json::Value =
                serde_json::from_str(&snapshot::read(&ids, &["fixture.value@1".into()]).unwrap())
                    .unwrap();
            for record in batch["properties"].as_array().unwrap() {
                assert_eq!(record["data"], payload["expected"]);
            }
            assert_eq!(
                snapshot::find_relations(
                    &ids,
                    snapshot::Direction::Either,
                    Some("fixture.edge@1"),
                    None,
                    256
                )
                .unwrap()
                .len(),
                1
            );
            assert_eq!(
                snapshot::find_properties("fixture.value@1", None, 256)
                    .unwrap()
                    .len(),
                2
            );
            assert!(matches!(
                snapshot::read(&[], &["foreign.key@1".into()]),
                Err(snapshot::Error::AccessDenied)
            ));
            vec![set(&ids[0], "fixture.value", payload["expected"].clone())]
        }
        "fixture.system-limits@1" => {
            assert!(matches!(
                snapshot::find_properties("fixture.label@1", None, 256),
                Err(snapshot::Error::MessageTooLarge)
            ));
            assert!(matches!(
                snapshot::find_relations(&[], snapshot::Direction::Either, None, None, 0),
                Err(snapshot::Error::InvalidRequest)
            ));
            vec![set(
                payload["item"].as_str().unwrap(),
                "fixture.value",
                serde_json::json!(7),
            )]
        }
        _ => return None,
    };
    Some(Ok(serde_json::json!({"changes":changes}).to_string()))
}

static IMPORTED_ASSET: std::sync::Mutex<Option<rintawa::engine::asset_store::AssetRef>> =
    std::sync::Mutex::new(None);
fn import_fixture_asset()
-> Result<rintawa::engine::asset_store::AssetRef, rintawa::engine::asset_store::Error> {
    use rintawa::engine::{asset_store, user_resources};
    if let Ok(resource) =
        user_resources::import_resource(include_bytes!("pixel.png"), "image/png", Some("pixel.png"))
    {
        let bytes = user_resources::read_resource(&resource, resource.size)
            .expect("read selected resource");
        let reference = asset_store::import_asset(&bytes, &resource.media_type);
        user_resources::release_resource(&resource).expect("release resource");
        return reference;
    }
    let size = 1024 * 1024 + 17;
    let upload = asset_store::begin_upload("image/png", size)?;
    let chunk = vec![7u8; 32768];
    let mut offset = 0;
    while offset < size {
        let count = (size - offset).min(chunk.len() as u64) as usize;
        asset_store::append_upload(upload, &chunk[..count])?;
        offset += count as u64;
    }
    asset_store::finish_upload(upload)
}
fn asset_error(error: rintawa::engine::asset_store::Error) -> serde_json::Value {
    use rintawa::engine::asset_store::Error;
    serde_json::json!({"error":match error { Error::AccessNotActive=>"access-not-active",Error::PermissionDenied=>"permission-denied",Error::MessageTooLarge=>"message-too-large",Error::InvalidAsset=>"invalid-asset",Error::Rejected=>"rejected",Error::Unavailable=>"unavailable" }})
}

static LAST_TICKET: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

impl exports::rintawa::engine::task_handler::Guest for Fixture {
    fn on_task(_: u64) {
        use rintawa::engine::user_resources as resources;
        let mut guard = RESOURCE_PROBE.lock().unwrap();
        let Some(probe) = guard.as_mut() else { return };
        if probe.trap {
            panic!("intentional resource task trap");
        }
        let Some(size) = probe.pending.pop() else {
            return;
        };
        let bytes = vec![0; size];
        let original =
            resources::import_resource(&bytes, "application/octet-stream", None).unwrap();
        let owned = resources::take_resource(&original).unwrap();
        assert!(matches!(
            resources::read_resource(&original, 1),
            Err(resources::Error::NotFound)
        ));
        assert!(resources::take_resource(&owned).is_err());
        probe.originals.push(original);
        probe.owned.push(owned);
    }
}

impl exports::rintawa::engine::transport_handler::Guest for Fixture {
    fn on_query(
        _: String,
        payload: Vec<u8>,
        _: rintawa::engine::runtime_context::Principal,
    ) -> Vec<u8> {
        use rintawa::engine::world_data::{self, Error, TicketStatus};
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("request JSON");
        let error = |error: Error| {
            serde_json::json!({"error": match error {
                Error::AccessNotActive => "access-not-active", Error::InvalidRequest => "invalid-request",
                Error::AccessDenied => "access-denied", Error::QueueFull => "queue-full",
                Error::Unavailable => "unavailable", Error::MessageTooLarge => "message-too-large",
            }})
        };
        let result = match request["op"].as_str().expect("operation") {
            "resource-fill" => {
                let size = request["bytes"].as_u64().unwrap() as usize;
                let count = request["count"].as_u64().unwrap() as usize;
                let mut guard = RESOURCE_PROBE.lock().unwrap();
                let probe = guard.as_mut().unwrap();
                assert!(count <= 2 && probe.pending.len() + probe.owned.len() + count <= 2);
                probe.pending.extend(std::iter::repeat_n(size, count));
                serde_json::json!({"queued":count})
            }
            "resource-state" => {
                let guard = RESOURCE_PROBE.lock().unwrap();
                let probe = guard.as_ref().unwrap();
                let summarize = |v: &rintawa::engine::user_resources::ResourceRef| serde_json::json!({"id":v.id,"size":v.size,"mediaType":v.media_type,"name":v.name});
                serde_json::json!({"count":probe.owned.len(),"originals":probe.originals.iter().map(summarize).collect::<Vec<_>>(),"owned":probe.owned.iter().map(summarize).collect::<Vec<_>>()})
            }
            "resource-trap" => {
                RESOURCE_PROBE.lock().unwrap().as_mut().unwrap().trap = true;
                serde_json::json!({"armed":true})
            }
            "submit" | "submit-lost-reply" => {
                let schema = request["schema"].as_str().expect("schema");
                let body = request
                    .get("payload")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({"item":request["item"]}))
                    .to_string();
                match world_data::submit(schema, &body) {
                    Ok(ticket) => {
                        *LAST_TICKET.lock().unwrap() = Some(ticket);
                        serde_json::json!({"ticket":ticket})
                    }
                    Err(failure) => error(failure),
                }
            }
            "poll" | "last" | "poll-lost-reply" => {
                let ticket = request["ticket"]
                    .as_u64()
                    .or(*LAST_TICKET.lock().unwrap())
                    .expect("ticket");
                match world_data::poll(ticket) {
                    Ok(TicketStatus::Pending) => serde_json::json!({"state":"pending"}),
                    Ok(TicketStatus::Committed) => serde_json::json!({"state":"committed"}),
                    Ok(TicketStatus::Failed(reason)) => {
                        serde_json::json!({"state":"failed","reason":reason})
                    }
                    Err(failure) => error(failure),
                }
            }
            "close" => match world_data::close(request["ticket"].as_u64().expect("ticket")) {
                Ok(()) => serde_json::json!({"closed":true}),
                Err(failure) => error(failure),
            },
            "read" => {
                let item = request["item"].as_str().expect("item");
                match world_data::load_item(item) {
                    Ok(value) => serde_json::json!({"exists":value.is_some()}),
                    Err(failure) => error(failure),
                }
            }
            "ids" => match world_data::new_item_ids(request["count"].as_u64().unwrap() as u32) {
                Ok(ids) => serde_json::json!({"ids":ids}),
                Err(e) => error(e),
            },
            "batch" | "batch-summary" => {
                let items: Vec<String> = serde_json::from_value(request["items"].clone()).unwrap();
                let keys: Vec<String> = serde_json::from_value(request["keys"].clone()).unwrap();
                match world_data::read(&items, &keys) {
                    Ok(value) => {
                        if request["op"] == "batch-summary" {
                            serde_json::json!({"encoded_bytes":value.len()})
                        } else {
                            serde_json::from_str(&value).unwrap()
                        }
                    }
                    Err(e) => error(e),
                }
            }
            "sized-input" => match world_data::read(
                &[],
                &["x".repeat(request["size"].as_u64().unwrap() as usize)],
            ) {
                Ok(_) => panic!("oversized input accepted"),
                Err(e) => error(e),
            },
            "relations" => {
                let direction = match request["direction"].as_str().unwrap() {
                    "incoming" => rintawa::engine::world_system_snapshot::Direction::Incoming,
                    "outgoing" => rintawa::engine::world_system_snapshot::Direction::Outgoing,
                    _ => rintawa::engine::world_system_snapshot::Direction::Either,
                };
                let targets: Vec<String> =
                    serde_json::from_value(request["targets"].clone()).unwrap();
                let property = request["property"].as_str();
                let after = request["after"].as_str();
                match world_data::find_relations(
                    &targets,
                    direction,
                    property,
                    after,
                    request["limit"].as_u64().unwrap() as u32,
                ) {
                    Ok(values) => serde_json::json!({"records":values}),
                    Err(e) => error(e),
                }
            }
            "properties" => match world_data::find_properties(
                request["key"].as_str().unwrap(),
                request["after"].as_str(),
                request["limit"].as_u64().unwrap() as u32,
            ) {
                Ok(values) => serde_json::json!({"records":values}),
                Err(e) => error(e),
            },
            "property" => match world_data::load_property(
                request["item"].as_str().unwrap(),
                request["key"].as_str().unwrap(),
            ) {
                Ok(value) => serde_json::json!({"property":value}),
                Err(e) => error(e),
            },
            "entropy" => {
                let count = request["count"].as_u64().unwrap() as u32;
                let mut lengths = Vec::new();
                let mut failed = None;
                for _ in 0..request["calls"].as_u64().unwrap_or(1) {
                    match rintawa::engine::host::random_bytes(count) {
                        Ok(bytes) => lengths.push(bytes.len()),
                        Err(e) => {
                            failed = Some(format!("{e:?}"));
                            break;
                        }
                    }
                }
                serde_json::json!({"lengths":lengths,"error":failed})
            }
            "asset-ref" => {
                let stored = IMPORTED_ASSET.lock().unwrap();
                match stored.as_ref() {
                    Some(r) => {
                        serde_json::json!({"digest":r.digest,"size":r.size,"media_type":r.media_type})
                    }
                    None => serde_json::json!({"error":"not-imported"}),
                }
            }
            "asset-open" => {
                let r = &request["reference"];
                let reference = rintawa::engine::asset_store::AssetRef {
                    digest: r["digest"].as_str().unwrap().into(),
                    size: r["size"].as_u64().unwrap(),
                    media_type: r["media_type"].as_str().unwrap().into(),
                };
                match rintawa::engine::asset_store::open_asset(&reference) {
                    Ok(handle) => serde_json::json!({"handle":handle}),
                    Err(e) => asset_error(e),
                }
            }
            "asset-read" => match rintawa::engine::asset_store::read_asset(
                request["handle"].as_u64().unwrap(),
                request["maximum"].as_u64().unwrap() as u32,
            ) {
                Ok(bytes) => {
                    // Summarize a chunk so transport's 64 KiB payload cap does not
                    // masquerade as the independent 64 KiB asset-read boundary.
                    serde_json::json!({"length":bytes.len(),"sum":bytes.iter().map(|b|u64::from(*b)).sum::<u64>()})
                }
                Err(e) => asset_error(e),
            },
            "asset-close" => {
                match rintawa::engine::asset_store::close_asset(request["handle"].as_u64().unwrap())
                {
                    Ok(()) => serde_json::json!({"closed":true}),
                    Err(e) => asset_error(e),
                }
            }
            "trap" => panic!("intentional callback trap"),
            _ => panic!("unexpected fixture operation"),
        };
        if request["op"] == "submit-lost-reply" && result.get("ticket").is_some()
            || request["op"] == "poll-lost-reply" && result["state"] == "committed"
        {
            return vec![0; 64 * 1024 + 1];
        }
        serde_json::to_vec(&result).unwrap()
    }
}

export!(Fixture);
