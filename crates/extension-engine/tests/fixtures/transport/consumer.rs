//! WASM fixture: nested queries run only after activation completes.
wit_bindgen::generate!({ path: "wit", world: "plugin-transport" });
struct Fixture;
impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {}
    fn start() {
        assert!(rintawa::engine::runtime_context::current().is_err());
        assert!(rintawa::engine::transport::query("chat/send-controls/v1", b"ping").is_err());
        rintawa::engine::transport::declare("fixture/consumer").expect("consumer callback");
    }
    fn stop() {}
}
impl exports::rintawa::engine::transport_handler::Guest for Fixture {
    fn on_query(_: String, _: Vec<u8>, _: rintawa::engine::runtime_context::Principal) -> Vec<u8> {
        let mut replies = rintawa::engine::transport::query("chat/send-controls/v1", b"ping")
            .expect("same-scope query");
        replies.sort();
        assert_eq!(replies, vec![b"wasm:ping".to_vec(), b"wasm:ping".to_vec()]);
        b"consumer-ok".to_vec()
    }
}
export!(Fixture);
