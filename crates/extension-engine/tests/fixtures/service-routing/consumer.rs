//! Guest fixture that consumes a routed test service.

wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
});

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        rintawa::engine::registration::consume_contract("example.echo", 1, true, &[])
            .expect("fixture consumer should register");
        rintawa::engine::registration::consume_contract(
            "example.secure-echo",
            1,
            false,
            &[],
        )
        .expect("optional secure fixture consumer should register");
    }

    fn start() {
        let response = rintawa::engine::services::call("example.echo", 1, b"ping")
            .expect("fixture service call should route");
        assert_eq!(response, b"pong");
    }

    fn stop() {}

    fn on_event(_topic: String, _payload: Vec<u8>) {}

    fn handle_ui_action(_action_json: Vec<u8>) {}

    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}

export!(Fixture);
