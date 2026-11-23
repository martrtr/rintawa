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
    fn on_query(
        _: String,
        payload: Vec<u8>,
        _: rintawa::engine::runtime_context::Principal,
    ) -> Vec<u8> {
        use rintawa::engine::transport;
        let point = "chat/send-controls/v1";
        if let Some(claim) = payload.strip_prefix(b"sourced-begin:") {
            return transport::begin_query(point, claim)
                .map(|handle| handle.to_string().into_bytes())
                .unwrap_or_else(|error| format!("error:{error:?}").into_bytes());
        }
        if let Some(handle) = payload.strip_prefix(b"sourced-poll:") {
            let handle = std::str::from_utf8(handle).unwrap().parse().unwrap();
            return match transport::poll_query_sourced(handle).expect("sourced poll") {
                None => b"pending".to_vec(),
                Some(replies) => {
                    let mut rows = replies
                        .into_iter()
                        .map(|reply| {
                            format!(
                                "{}|{}|{}|{}",
                                reply.provider.instance_id,
                                reply.provider.component_id,
                                reply.provider.activation,
                                String::from_utf8(reply.payload).unwrap()
                            )
                        })
                        .collect::<Vec<_>>();
                    rows.sort();
                    rows.join("\n").into_bytes()
                }
            };
        }
        let mut replies = transport::query(point, b"ping").expect("same-scope query");
        replies.sort();
        assert_eq!(replies, vec![b"wasm:ping".to_vec(), b"wasm:ping".to_vec()]);
        b"consumer-ok".to_vec()
    }
}
export!(Fixture);
