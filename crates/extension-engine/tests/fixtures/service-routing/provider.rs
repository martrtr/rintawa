wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
});

const SECRET_PATH: &str = "service.credentials.fixture";

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        use rintawa::engine::registration::{ContractProtocol, ResolutionPolicy};

        for contract in ["example.echo", "example.secure-echo"] {
            rintawa::engine::registration::define_contract(
                contract,
                1,
                ResolutionPolicy::Single,
                ContractProtocol::Service,
            )
            .expect("fixture contract definition should register");
        }
        rintawa::engine::registration::provide_contract("example.echo", 1, &[])
            .expect("fixture provider should register");
        rintawa::engine::registration::provide_contract(
            "example.secure-echo",
            1,
            &[SECRET_PATH.to_string()],
        )
        .expect("secure fixture provider should register");
    }

    fn start() {}

    fn stop() {}

    fn on_event(_topic: String, _payload: Vec<u8>) {}

    fn handle_ui_action(_action_json: Vec<u8>) {}

    fn handle_service(contract: String, version: u32, payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(version, 1);
        assert_eq!(payload, b"ping");
        match contract.as_str() {
            "example.echo" => b"pong".to_vec(),
            "example.secure-echo" => {
                let secret = rintawa::engine::secrets::read(SECRET_PATH)
                    .expect("granted fixture secret should be readable");
                assert_eq!(secret, "fixture-key");
                b"secure-pong".to_vec()
            }
            _ => panic!("unexpected fixture service contract"),
        }
    }
}

export!(Fixture);
