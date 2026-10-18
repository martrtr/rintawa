//! Auditable WASM fixture for one provider of a `Multiple` service contract.

wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
});

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        use rintawa::engine::registration::{ContractProtocol, ResolutionPolicy};

        rintawa::engine::registration::define_contract(
            "example.multiple-echo",
            1,
            ResolutionPolicy::Multiple,
            ContractProtocol::Service,
        )
        .expect("multiple fixture contract definition should register");
        rintawa::engine::registration::provide_contract("example.multiple-echo", 1, &[])
            .expect("multiple fixture provider should register");
    }

    fn start() {}
    fn stop() {}
    fn on_event(_topic: String, _payload: Vec<u8>) {}
    fn handle_ui_action(_action_json: Vec<u8>) {}

    fn handle_service(contract: String, version: u32, payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(contract, "example.multiple-echo");
        assert_eq!(version, 1);
        assert_eq!(payload, b"ping");
        b"multi-pong".to_vec()
    }
}

export!(Fixture);
