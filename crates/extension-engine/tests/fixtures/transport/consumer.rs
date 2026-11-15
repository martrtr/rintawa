//! WASM fixture: queries independent same-scope providers without a registry.
wit_bindgen::generate!({ path: "wit", world: "plugin" });

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {}
    fn start() {
        use rintawa::engine::transport;
        assert!(transport::query("../escape", b"ping").is_err());
        assert!(transport::publish("chat/updates/v1", b"uncommitted").is_err());
        let mut replies = transport::query("chat/send-controls/v1", b"ping")
            .expect("same-scope transport query");
        replies.sort();
        assert_eq!(replies, vec![b"wasm:ping".to_vec(), b"wasm:ping".to_vec()]);
    }
    fn stop() {}
}
export!(Fixture);
