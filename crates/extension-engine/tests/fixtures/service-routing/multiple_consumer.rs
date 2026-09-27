//! Auditable WASM fixture for targeted calls to `Multiple` service providers.

wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
});

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        rintawa::engine::registration::consume_contract(
            "example.multiple-echo",
            1,
            true,
            &[],
        )
        .expect("multiple fixture consumer should register");
    }

    fn start() {
        let providers = rintawa::engine::services::list_providers("example.multiple-echo", 1)
            .expect("multiple fixture provider enumeration should route");
        assert_eq!(providers.len(), 2);
        for provider in providers {
            let response = rintawa::engine::services::call_provider(provider, b"ping")
                .expect("targeted multiple fixture call should route");
            assert_eq!(response, b"multi-pong");
        }
    }

    fn stop() {}
    fn on_event(_topic: String, _payload: Vec<u8>) {}
    fn handle_ui_action(_action_json: Vec<u8>) {}
    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}

export!(Fixture);
