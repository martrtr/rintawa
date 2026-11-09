//! Real WASM fixture for authoritative World System execution.
wit_bindgen::generate!({ path: "wit", world: "world-system-plugin" });

use std::sync::Mutex;

static WORLD_COMMAND_TICKET: Mutex<Option<u64>> = Mutex::new(None);

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
        for command in ["fixture.commit", "fixture.reject", "fixture.foreign"] {
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
        rintawa::engine::runtime_effects::subscribe_event("fixture.world-data")
            .expect("world-data event subscription");
    }
    fn stop() {}
    fn on_event(topic: String, payload: Vec<u8>) {
        if topic != "fixture.world-data" {
            return;
        }
        let request = String::from_utf8(payload).expect("UTF-8 fixture request");
        if let Some(item) = request.strip_prefix("submit:") {
            let loaded = rintawa::engine::world_data::load_item(item)
                .expect("active World item read");
            assert!(loaded.is_some(), "fixture item must exist");
            let ticket = rintawa::engine::world_data::submit(
                "fixture.commit@1",
                &serde_json::json!({ "item": item }).to_string(),
            )
            .expect("nonblocking World command submission");
            *WORLD_COMMAND_TICKET.lock().expect("ticket lock") = Some(ticket);
            return;
        }
        if let Some(item) = request.strip_prefix("verify:") {
            let ticket = WORLD_COMMAND_TICKET
                .lock()
                .expect("ticket lock")
                .take()
                .expect("submitted ticket");
            match rintawa::engine::world_data::poll(ticket).expect("nonblocking ticket poll") {
                rintawa::engine::world_data::TicketStatus::Committed => {}
                rintawa::engine::world_data::TicketStatus::Pending => {
                    panic!("test must not observe success before commit")
                }
                rintawa::engine::world_data::TicketStatus::Failed(reason) => {
                    panic!("World command unexpectedly failed: {reason}")
                }
            }
            let property = rintawa::engine::world_data::load_property(item, "fixture.value@1")
                .expect("owner-scoped property read")
                .expect("committed property");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&property)
                    .expect("property JSON")["data"],
                serde_json::json!(7)
            );
            return;
        }
        if let Some(item) = request.strip_prefix("foreign:") {
            assert!(matches!(
                rintawa::engine::world_data::load_property(item, "fixture.foreign-property@1"),
                Err(rintawa::engine::world_data::Error::AccessDenied)
            ));
            return;
        }
        if request == "invalid" {
            assert!(matches!(
                rintawa::engine::world_data::submit("fixture.commit@1", "{"),
                Err(rintawa::engine::world_data::Error::InvalidRequest)
            ));
            return;
        }
        panic!("unknown fixture request");
    }
}

impl exports::rintawa::engine::world_system::Guest for Fixture {
    fn evaluate(command_json: String) -> Result<String, exports::rintawa::engine::world_system::Error> {
        let command: serde_json::Value = serde_json::from_str(&command_json)
            .map_err(|error| exports::rintawa::engine::world_system::Error::Failed(error.to_string()))?;
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
            Some("fixture.foreign@1") => Ok(serde_json::json!({"changes":[{
                "operation":"set-property", "item":item,
                "key":{"id":"fixture.foreign-property","version":1}, "data":9
            }]}).to_string()),
            _ => Err(exports::rintawa::engine::world_system::Error::Failed(String::from("unexpected command"))),
        }
    }
}

export!(Fixture);
