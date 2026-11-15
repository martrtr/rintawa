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

}

impl exports::rintawa::engine::task_handler::Guest for Fixture {
    fn on_task(handle: u64) {
        let cancelled = rintawa::engine::runtime_tasks::cancel(handle);
        assert!(cancelled.is_ok());
    }
}

export!(Fixture);
