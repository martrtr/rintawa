//! Real WASM fixture for authoritative World System execution.
wit_bindgen::generate!({ path: "wit", world: "world-system-plugin" });

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

    fn start() {}
    fn stop() {}
    fn on_event(_topic: String, _payload: Vec<u8>) {}
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
