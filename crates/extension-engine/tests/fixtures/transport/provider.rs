//! WASM fixture: declares a host-scoped Zenoh point on start.
wit_bindgen::generate!({ path: "wit", world: "plugin-transport" });

use std::sync::Mutex;

static SUBSCRIPTION: Mutex<Option<u64>> = Mutex::new(None);

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        use rintawa::engine::transport;
        assert!(transport::declare("chat/send-controls/v1").is_err());
        assert!(transport::query("chat/send-controls/v1", b"ping").is_err());
        assert!(transport::subscribe("chat/updates/v1").is_err());
        assert!(transport::publish("chat/updates/v1", b"not-started").is_err());
    }

    fn start() {
        use rintawa::engine::transport;
        assert!(transport::declare("../escape").is_err());
        match transport::declare("chat/send-controls/v1") {
            Ok(()) => {
                assert!(transport::declare("chat/send-controls/v1").is_err());
            }
            Err(transport::Error::NotSelected) => {
                // Non-selected providers remain active for their own independent APIs.
            }
            Err(error) => panic!("unexpected transport declaration error: {error:?}"),
        }
        assert!(transport::publish("chat/updates/v1", b"start-not-committed").is_err());
        let handle = transport::subscribe("chat/updates/v1").expect("subscribe on start");
        *SUBSCRIPTION.lock().expect("fixture subscription lock") = Some(handle);
    }

    fn stop() {
        assert!(rintawa::engine::transport::declare("chat/send-controls/v1").is_err());
        assert!(rintawa::engine::transport::query("chat/send-controls/v1", b"ping").is_err());
        assert!(rintawa::engine::transport::publish("chat/updates/v1", b"stop").is_err());
        *SUBSCRIPTION.lock().expect("fixture subscription lock") = None;
    }
    fn on_event(_topic: String, _payload: Vec<u8>) {}
    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}

impl exports::rintawa::engine::transport_handler::Guest for Fixture {
    fn on_query(point: String, payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(point, "chat/send-controls/v1");
        if payload == b"send-event" {
            rintawa::engine::transport::publish("chat/updates/v1", b"from-wasm")
                .expect("publish inside active WASM handler");
            return b"published".to_vec();
        }
        if payload == b"drain-events" {
            let handle = SUBSCRIPTION.lock().expect("fixture lock").expect("active handle");
            let events = rintawa::engine::transport::poll(handle).expect("WASM poll");
            return format!("events:{}", events.len()).into_bytes();
        }
        if payload == b"fuel-burn" {
            loop {
                std::hint::black_box(7_u64);
            }
        }
        if payload == b"oversized" {
            return vec![0; 64 * 1024 + 1];
        }
        let mut answer = b"wasm:".to_vec();
        answer.extend_from_slice(&payload);
        answer
    }
}
export!(Fixture);
