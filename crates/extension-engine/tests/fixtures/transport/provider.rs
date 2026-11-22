//! WASM fixture: declares a host-scoped Zenoh point on start.
wit_bindgen::generate!({ path: "wit", world: "plugin-transport" });

use std::sync::Mutex;

static SUBSCRIPTION: Mutex<Option<u64>> = Mutex::new(None);

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        use rintawa::engine::{registration, transport};
        registration::define_contract("fixture.backend", 1, registration::ResolutionPolicy::Single)
            .expect("backend definition");
        registration::provide_contract("fixture.backend", 1, &[]).expect("backend provider");
        registration::provide_contract("rintawa.host.shell", 1, &[])
            .expect("generic shell binding");
        assert!(transport::declare_polled("fixture/not-active").is_err());
        assert!(transport::begin_query("fixture/not-active", b"no").is_err());
        assert!(transport::declare("chat/send-controls/v1").is_err());
        assert!(transport::query("chat/send-controls/v1", b"ping").is_err());
        assert!(transport::subscribe("chat/updates/v1").is_err());
        assert!(transport::publish("chat/updates/v1", b"not-started").is_err());
    }

    fn start() {
        use rintawa::engine::transport;
        assert!(transport::declare_polled("fixture/not-yet-active").is_err());
        assert!(transport::begin_query("fixture/not-yet-active", b"no").is_err());
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
        match transport::declare("fixture/caller") {
            Ok(()) | Err(transport::Error::NotSelected) => {}
            Err(error) => panic!("caller point: {error:?}"),
        }
        assert!(transport::publish("chat/updates/v1", b"start-not-committed").is_err());
        assert!(transport::subscribe("chat/updates/v1").is_err());
    }

    fn stop() {
        assert!(rintawa::engine::transport::declare("chat/send-controls/v1").is_err());
        assert!(rintawa::engine::transport::query("chat/send-controls/v1", b"ping").is_err());
        assert!(rintawa::engine::transport::publish("chat/updates/v1", b"stop").is_err());
        *SUBSCRIPTION.lock().expect("fixture subscription lock") = None;
    }
}

impl exports::rintawa::engine::transport_handler::Guest for Fixture {
    fn on_query(
        point: String,
        payload: Vec<u8>,
        caller: rintawa::engine::runtime_context::Principal,
    ) -> Vec<u8> {
        assert!(point == "chat/send-controls/v1" || point == "fixture/caller");
        if let Some(result) = d8_control(&payload) {
            return result;
        }
        if let Some(forward) = payload.strip_prefix(b"forward:") {
            let split = forward
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("forward point delimiter");
            let point = std::str::from_utf8(&forward[..split]).expect("forward point");
            return rintawa::engine::transport::query(point, &forward[split + 1..])
                .expect("forward query")
                .remove(0);
        }
        let mut subscription = SUBSCRIPTION.lock().expect("fixture subscription lock");
        if subscription.is_none() {
            *subscription = Some(
                rintawa::engine::transport::subscribe("chat/updates/v1")
                    .expect("active subscription"),
            );
        }
        drop(subscription);
        if let Some(id) = payload.strip_prefix(b"selected:") {
            let id = std::str::from_utf8(id).expect("contract ID");
            return match rintawa::engine::runtime_context::selected_providers(id, 1) {
                Ok(providers) => providers
                    .into_iter()
                    .map(|principal| {
                        format!(
                            "{}|{}|{}",
                            principal.instance_id, principal.component_id, principal.activation
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into_bytes(),
                Err(rintawa::engine::runtime_context::Error::Rejected) => b"rejected".to_vec(),
                Err(error) => format!("error:{error:?}").into_bytes(),
            };
        }
        if payload.starts_with(b"identity:") {
            return format!(
                "{}|{}|{}",
                caller.instance_id, caller.component_id, caller.activation
            )
            .into_bytes();
        }
        if payload.starts_with(b"nested:") {
            let point = std::str::from_utf8(&payload[b"nested:".len()..]).expect("nested point");
            return rintawa::engine::transport::query(point, b"identity:spoofed")
                .expect("nested query")
                .remove(0);
        }

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
            return match rintawa::engine::world_data::find_properties("fixture.private@1", None, 1)
            {
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
            let handle = SUBSCRIPTION
                .lock()
                .expect("fixture lock")
                .expect("active handle");
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

fn d8_control(payload: &[u8]) -> Option<Vec<u8>> {
    use rintawa::engine::transport;
    let text = std::str::from_utf8(payload).ok()?;
    let rest = text.strip_prefix("d8:")?;
    let (operation, input) = rest.split_once(':').unwrap();
    let parsed = || input.split('\n').next().unwrap().parse::<u64>().unwrap();
    let bytes = match operation {
        "declare" => {
            transport::declare_polled(input).map(|handle| format!("handle:{handle}").into_bytes())
        }
        "begin" => {
            let (point, payload) = input.split_once('\n').unwrap();
            transport::begin_query(point, payload.as_bytes())
                .map(|handle| format!("handle:{handle}").into_bytes())
        }
        "poll" => transport::poll_query(parsed()).map(|reply| match reply {
            None => b"pending".to_vec(),
            Some(mut values) => {
                assert_eq!(values.len(), 1);
                let mut bytes = b"reply:".to_vec();
                bytes.append(&mut values.remove(0));
                bytes
            }
        }),
        "next" => transport::next_query(parsed()).map(|request| match request {
            None => b"pending".to_vec(),
            Some(request) => {
                let mut bytes = format!(
                    "{}|{}|{}|{}\n",
                    request.handle,
                    request.caller.instance_id,
                    request.caller.component_id,
                    request.caller.activation
                )
                .into_bytes();
                bytes.extend(request.payload);
                bytes
            }
        }),
        "reply" => {
            let (_, bytes) = input.split_once('\n').unwrap();
            transport::reply_query(parsed(), Some(bytes.as_bytes())).map(|_| b"ok".to_vec())
        }
        "close" => transport::close_query(parsed()).map(|_| b"closed".to_vec()),
        "undeclare" => transport::close_queryable(parsed()).map(|_| b"closed".to_vec()),
        _ => panic!("unknown D8 operation"),
    };
    Some(bytes.unwrap_or_else(|error| format!("error:{error:?}").into_bytes()))
}
