//! Guest fixture for execution-target provider integration tests.

wit_bindgen::generate!({
    path: "wit",
    world: "runtime-provider-task-plugin",
});

use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(7);
static COMPONENTS: Mutex<BTreeMap<u64, String>> = Mutex::new(BTreeMap::new());
static TASKS: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
static UPLOADS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
static SUBSCRIPTIONS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {}

    fn start() {
        let result = rintawa::engine::execution_targets::register_target("test.wasm-target@1");
        assert!(result.is_ok());
        if let Ok(task) = rintawa::engine::runtime_tasks::spawn_periodic(10) {
            TASKS.lock().unwrap().insert(task, 0);
        }
    }

    fn stop() {}
}

impl exports::rintawa::engine::target_provider::Guest for Fixture {
    fn load_component(
        target: String,
        descriptor: exports::rintawa::engine::target_provider::ComponentDescriptor,
        source: &rintawa::engine::execution_targets::ArtifactSource,
    ) -> Result<u64, exports::rintawa::engine::target_provider::Error> {
        use exports::rintawa::engine::target_provider::Error;

        if target != "test.wasm-target@1" || descriptor.target != target {
            return Err(Error::InvalidDescriptor);
        }
        let path = source
            .resolve_component_entry("payload.bin")
            .map_err(|_| Error::Unavailable)?;
        let bytes = source.read(&path).map_err(|_| Error::Unavailable)?;
        if bytes != b"hosted payload" {
            return Err(Error::InvalidDescriptor);
        }
        let handle = NEXT.fetch_add(1, Ordering::Relaxed);
        COMPONENTS.lock().unwrap().insert(handle, descriptor.id);
        Ok(handle)
    }

    fn register_component(
        handle: u64,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if COMPONENTS.lock().unwrap().contains_key(&handle) {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn start_component(
        handle: u64,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if !COMPONENTS.lock().unwrap().contains_key(&handle) {
            return Err(exports::rintawa::engine::target_provider::Error::UnknownComponent);
        }

        assert!(rintawa::engine::runtime_context::current().is_err());
        assert!(rintawa::engine::transport::query("fixture/context", b"start").is_err());
        assert!(rintawa::engine::transport::declare("fixture/hosted-declare").is_err());
        if let Ok(task) = rintawa::engine::runtime_tasks::spawn_periodic(10) {
            TASKS.lock().unwrap().insert(task, handle);
        }
        Ok(())
    }

    fn stop_component(handle: u64) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if COMPONENTS.lock().unwrap().contains_key(&handle) {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn drop_component(handle: u64) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if COMPONENTS.lock().unwrap().contains_key(&handle) {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }
}

impl exports::rintawa::engine::task_handler::Guest for Fixture {
    fn on_task(task: u64) {
        use rintawa::engine::{runtime_context, transport, user_resources};
        let component = *TASKS.lock().unwrap().get(&task).expect("known task");
        let current = runtime_context::current().expect("attested task context");
        if component != 0 {
            let expected = COMPONENTS
                .lock()
                .unwrap()
                .get(&component)
                .cloned()
                .expect("loaded dependent");
            assert_eq!(current.principal.component_id, expected);
            if expected == "trap" {
                panic!("hosted task trap");
            }
        }
        let subscriptions = &mut *SUBSCRIPTIONS.lock().unwrap();
        for (owner, foreign) in subscriptions.iter() {
            if owner != &current.principal.instance_id {
                assert!(transport::poll(*foreign).is_err());
            }
        }
        subscriptions
            .entry(current.principal.instance_id.clone())
            .or_insert_with(|| {
                transport::subscribe("fixture/hosted-hints").expect("hosted subscribe")
            });
        let uploads = &mut *UPLOADS.lock().unwrap();
        for (owner, foreign) in uploads.iter() {
            if owner != &current.principal.instance_id {
                assert!(user_resources::append_upload(*foreign, b"no").is_err());
            }
        }
        if !uploads.contains_key(&current.principal.instance_id) {
            if let Ok(upload) = user_resources::begin_upload("application/octet-stream", None, 4) {
                user_resources::append_upload(upload, b"ok").expect("own upload");
                uploads.insert(current.principal.instance_id.clone(), upload);
            }
        }
        transport::publish(
            "fixture/context",
            format!(
                "{}|{}|{}|{}|{}",
                current.scope_id,
                current.principal.instance_id,
                current.principal.component_id,
                current.principal.activation,
                current.world_id.unwrap_or_default()
            )
            .as_bytes(),
        )
        .expect("scoped task publication");
    }
}

export!(Fixture);
