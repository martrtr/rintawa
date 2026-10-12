//! WASM provider for authenticated ephemeral stream handoff.

wit_bindgen::generate!({ path: "wit", world: "plugin" });

use rintawa::engine::{
    async_operations as ops,
    registration::{ContractProtocol, ResolutionPolicy},
};
use std::sync::Mutex;
const CONTRACT: &str = "example.operation-stream";
static HANDLE: Mutex<Option<String>> = Mutex::new(None);
struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {
        rintawa::engine::registration::define_contract(
            CONTRACT,
            1,
            ResolutionPolicy::Single,
            ContractProtocol::Service,
        )
        .expect("define contract");
        rintawa::engine::registration::provide_contract(CONTRACT, 1, &[])
            .expect("provide contract");
    }
    fn start() {}
    fn stop() {
        *HANDLE.lock().unwrap() = None;
    }
    fn on_event(_topic: String, _payload: Vec<u8>) {}
    fn handle_ui_action(_json: Vec<u8>) {}
    fn handle_service(contract: String, version: u32, payload: Vec<u8>) -> Vec<u8> {
        assert_eq!(contract, CONTRACT);
        assert_eq!(version, 1);
        if payload == b"begin" {
            let handle = ops::create().unwrap();
            ops::push_progress(&handle, 25).unwrap();
            ops::push_data(&handle, b"hello ").unwrap();
            let token = ops::share_with_caller(&handle).unwrap();
            *HANDLE.lock().unwrap() = Some(handle);
            return token.into_bytes();
        }
        let handle = HANDLE.lock().unwrap().clone().unwrap();
        match payload.as_slice() {
            b"continue" => {
                ops::push_progress(&handle, 75).unwrap();
                ops::push_data(&handle, b"world").unwrap();
            }
            b"complete" => ops::complete(&handle, b"ok").unwrap(),
            b"fail" => ops::fail(&handle, "remote failure").unwrap(),
            b"cancel" => ops::cancel(&handle).unwrap(),
            _ => panic!("unrecognized test request"),
        }
        payload
    }
}
export!(Fixture);
