//! Real WASM fixture for authoritative World System execution.
wit_bindgen::generate!({ path: "wit", world: "command-ticket-fixture" });

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        use rintawa::engine::world_registration::{self, SchemaKind};
        world_registration::register_world_schema(
            "fixture.value",
            1,
            SchemaKind::Property,
            r#"{"type":"integer"}"#,
        )
        .expect("value property schema");
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
        rintawa::engine::transport::declare("fixture/commands").expect("command point");
    }
    fn stop() {
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
        let command: serde_json::Value = serde_json::from_str(&command_json).map_err(|error| {
            exports::rintawa::engine::world_system::Error::Failed(error.to_string())
        })?;
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

static LAST_TICKET: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

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
            "submit" | "submit-lost-reply" => {
                let schema = request["schema"].as_str().expect("schema");
                let body = serde_json::json!({"item":request["item"]}).to_string();
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
