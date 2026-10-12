//! WASM consumer verifying ordered streaming, cancellation and failure.

wit_bindgen::generate!({ path: "wit", world: "plugin" });

use rintawa::engine::async_operations::{Error, Event};
use rintawa::engine::{async_operations as ops, services};

const CONTRACT: &str = "example.operation-stream";
struct Fixture;

fn call(payload: &[u8]) -> Vec<u8> {
    services::call(CONTRACT, 1, payload).expect("service call")
}
fn begin() -> String {
    String::from_utf8(call(b"begin")).expect("UTF-8 lease")
}
fn read_first(token: &str) {
    match ops::poll_observer(token).expect("first progress") {
        Event::Progress(progress) => {
            assert_eq!(progress.sequence, 1);
            assert_eq!(progress.percent, 25);
        }
        _ => panic!("missing first progress"),
    }
    match ops::poll_observer(token).expect("first chunk") {
        Event::Data(data) => {
            assert_eq!(data.sequence, 2);
            assert_eq!(data.bytes, b"hello ");
        }
        _ => panic!("missing first chunk"),
    }
    assert!(matches!(ops::poll_observer(token), Ok(Event::Pending)));
}
impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        rintawa::engine::registration::consume_contract(CONTRACT, 1, true, &[])
            .expect("declare service consumer");
    }
    fn start() {
        let token = begin();
        read_first(&token);
        assert_eq!(call(b"continue"), b"continue");
        match ops::poll_observer(&token).expect("second progress") {
            Event::Progress(value) => {
                assert_eq!(value.sequence, 3);
                assert_eq!(value.percent, 75);
            }
            _ => panic!("missing second progress"),
        }
        match ops::poll_observer(&token).expect("second chunk") {
            Event::Data(value) => {
                assert_eq!(value.sequence, 4);
                assert_eq!(value.bytes, b"world");
            }
            _ => panic!("missing second chunk"),
        }
        assert_eq!(call(b"complete"), b"complete");
        assert!(
            matches!(ops::poll_observer(&token), Ok(Event::Completed(bytes)) if bytes == b"ok")
        );
        assert!(matches!(ops::cancel(&token), Err(Error::UnknownHandle)));
        ops::close_observer(&token).unwrap();
        assert!(matches!(
            ops::poll_observer(&token),
            Err(Error::UnknownHandle)
        ));

        let token = begin();
        assert_eq!(call(b"cancel"), b"cancel");
        assert!(matches!(ops::poll_observer(&token), Ok(Event::Cancelled)));
        ops::close_observer(&token).unwrap();

        let token = begin();
        read_first(&token);
        assert_eq!(call(b"fail"), b"fail");
        assert!(matches!(ops::poll_observer(&token),
            Ok(Event::Failed(message)) if message == "remote failure"));
        ops::close_observer(&token).unwrap();
    }
    fn stop() {}
    fn on_event(_topic: String, _payload: Vec<u8>) {}
    fn handle_ui_action(_json: Vec<u8>) {}
    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}
export!(Fixture);
