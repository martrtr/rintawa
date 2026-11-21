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
}

impl exports::rintawa::engine::transport_handler::Guest for Fixture {
    fn on_query(point: String, payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(point, "chat/send-controls/v1");
        if let Some(item) = payload.strip_prefix(b"world-property:") {
            let item = std::str::from_utf8(item).expect("fixture item ID");
            return match rintawa::engine::world_data::load_property(item, "fixture.private@1") {
                Ok(Some(value)) => value.into_bytes(),
                Ok(None) => b"missing".to_vec(),
                Err(rintawa::engine::world_data::Error::AccessDenied) => b"denied".to_vec(),
                Err(error) => format!("error:{error:?}").into_bytes(),
            };
        }
        if payload == b"world-find" {
            return match rintawa::engine::world_data::find_properties("fixture.private@1", None, 1) {
                Ok(values) => format!("found:{}", values.len()).into_bytes(),
                Err(rintawa::engine::world_data::Error::AccessDenied) => b"denied".to_vec(),
                Err(error) => format!("error:{error:?}").into_bytes(),
            };
        }
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
