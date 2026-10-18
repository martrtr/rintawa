//! Guest fixture for task-runtime integration tests.

wit_bindgen::generate!({
    path: "wit",
    world: "task-runtime-plugin",
});

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {}

    fn start() {
        let task = rintawa::engine::runtime_tasks::spawn_periodic(10);
        assert!(task.is_ok());
    }

    fn stop() {}

    fn on_event(_topic: String, _payload: Vec<u8>) {}

    fn handle_ui_action(_action_json: Vec<u8>) {}

    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}

impl exports::rintawa::engine::task_handler::Guest for Fixture {
    fn on_task(handle: u64) {
        let effect = rintawa::engine::runtime_effects::subscribe_event("fixture.task");
        assert!(effect.is_ok());
        let cancelled = rintawa::engine::runtime_tasks::cancel(handle);
        assert!(cancelled.is_ok());
    }
}

export!(Fixture);
