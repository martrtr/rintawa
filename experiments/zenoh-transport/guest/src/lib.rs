//! Sandboxed WASM probe: no socket, filesystem, identity or scope arguments.
wit_bindgen::generate!({
    path: "../wit",
    world: "transport-probe",
});

use std::sync::atomic::{AtomicUsize, Ordering};

static LAST_EVENT_SIZE: AtomicUsize = AtomicUsize::new(0);

struct GuestProbe;

impl exports::rintawa::transport_probe::guest::Guest for GuestProbe {
    fn start() {
        // A malicious guest cannot escape its host-attested namespace.
        assert!(rintawa::transport_probe::broker::declare_queryable("../world-b").is_err());
        assert!(rintawa::transport_probe::broker::publish("**", b"spoof").is_err());
        assert!(rintawa::transport_probe::broker::publish("processed", &vec![0; 4097]).is_err());
        assert!(rintawa::transport_probe::broker::reply(b"outside-query").is_err());
        let broker = rintawa::transport_probe::broker::declare_queryable("answer");
        assert!(broker.is_ok());
        let subscriber = rintawa::transport_probe::broker::subscribe("incoming");
        assert!(subscriber.is_ok());
    }

    fn on_query(payload: Vec<u8>) -> Vec<u8> {
        if payload == b"burn" {
            loop {
                std::hint::spin_loop();
            }
        }
        if payload == b"stream-burn" {
            assert!(rintawa::transport_probe::broker::reply(b"partial").is_ok());
            loop {
                std::hint::spin_loop();
            }
        }
        if payload == b"stream" {
            for chunk in [b"first".as_slice(), b"middle", b"last"] {
                assert!(rintawa::transport_probe::broker::reply(chunk).is_ok());
            }
            assert!(rintawa::transport_probe::broker::reply(&vec![0; 4097]).is_err());
            return Vec::new();
        }
        if payload == b"nested" {
            let roots = rintawa::transport_probe::broker::query_public("workspace", b"render")
                .expect("the host must deliver workspace contributions");
            if roots.is_empty() {
                return b"nested-empty".to_vec();
            }
            assert_eq!(roots, vec![b"chat-ui|controls".to_vec()]);
            let nested_key = roots[0]
                .split(|byte| *byte == b'|')
                .nth(1)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .expect("test contribution contains a nested extension point name");
            let mut controls =
                rintawa::transport_probe::broker::query_public(nested_key, b"render-input")
                    .expect("nested extension point must reuse ordinary transport");
            controls.sort();
            assert_eq!(controls, vec![b"dice".to_vec(), b"emoji".to_vec()]);
            return b"nested:2".to_vec();
        }
        if payload == b"compose" {
            assert!(
                rintawa::transport_probe::broker::query_public("other/world", b"request").is_err()
            );
            let mut contributions =
                rintawa::transport_probe::broker::query_public("controls", b"request")
                    .expect("Host-mediated public contributions must be reachable");
            contributions.sort();
            assert_eq!(contributions, vec![b"dice".to_vec(), b"emoji".to_vec()]);
            return b"composed:2".to_vec();
        }
        if payload == b"compose-empty" {
            let contributions =
                rintawa::transport_probe::broker::query_public("controls", b"request")
                    .expect("valid query to an empty World should succeed");
            assert!(contributions.is_empty());
            return b"empty".to_vec();
        }
        if payload == b"compose-failed" {
            assert!(rintawa::transport_probe::broker::query_public("broken", b"request").is_err());
            return b"failure-forwarded".to_vec();
        }
        if payload == b"overquery" {
            for _ in 0..8 {
                assert!(
                    rintawa::transport_probe::broker::query_public("controls", b"request").is_ok()
                );
            }
            assert!(
                rintawa::transport_probe::broker::query_public("controls", b"request").is_err()
            );
            return b"limited".to_vec();
        }
        let publish = rintawa::transport_probe::broker::publish("processed", b"from-wasm");
        assert!(publish.is_ok());
        let mut response = b"pong:".to_vec();
        response.extend_from_slice(&payload);
        response.push(b':');
        response.extend_from_slice(
            LAST_EVENT_SIZE
                .load(Ordering::Relaxed)
                .to_string()
                .as_bytes(),
        );
        response
    }

    fn on_event(payload: Vec<u8>) {
        LAST_EVENT_SIZE.store(payload.len(), Ordering::Relaxed);
    }
}

export!(GuestProbe);
