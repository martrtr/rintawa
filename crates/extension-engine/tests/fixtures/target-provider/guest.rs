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
    fn register() {
        assert!(rintawa::engine::host::monotonic_millis().is_err());
        assert!(rintawa::engine::host::random_bytes(16).is_err());
    }

    fn start() {
        rintawa::engine::host::monotonic_millis().expect("provider start clock");
        assert_eq!(rintawa::engine::host::random_bytes(16).unwrap().len(), 16);
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
        assert!(rintawa::engine::host::random_bytes(16).is_err());
        assert!(rintawa::engine::host::monotonic_millis().is_err());
        if COMPONENTS.lock().unwrap().contains_key(&handle) {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn start_component(
        handle: u64,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        assert_eq!(rintawa::engine::host::random_bytes(16).unwrap().len(), 16);
        rintawa::engine::host::monotonic_millis().expect("dependent start clock");
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
        rintawa::engine::host::monotonic_millis().expect("ordinary/delegated task clock");
        assert_eq!(rintawa::engine::host::random_bytes(16).unwrap().len(), 16);
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
            if expected.starts_with("d8-") {
                d8_task(&expected, &current);
                return;
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

struct D8State {
    incoming: Option<rintawa::engine::transport::InboundQuery>,
    pending: Option<Vec<u8>>,
    activation: u64,
    queryable: Option<u64>,
    outgoing: Option<u64>,
    subscription: Option<u64>,
}
static D8_STATES: Mutex<BTreeMap<String, D8State>> = Mutex::new(BTreeMap::new());
fn d8_task(role: &str, current: &rintawa::engine::runtime_context::Context) {
    use rintawa::engine::transport;
    let mut states = D8_STATES.lock().unwrap();
    let id = &current.principal.instance_id;
    let activation = current.principal.activation;
    // Same-Store dependents cannot use each other's declarations/results.
    for (foreign, state) in states.iter() {
        if foreign == id {
            continue;
        }
        if let Some(handle) = state.queryable {
            assert!(matches!(
                transport::next_query(handle),
                Err(transport::Error::InvalidHandle | transport::Error::Unavailable)
            ));
            assert!(matches!(
                transport::close_queryable(handle),
                Err(transport::Error::InvalidHandle | transport::Error::Unavailable)
            ));
        }
        if let Some(handle) = state.outgoing {
            assert!(matches!(
                transport::poll_query(handle),
                Err(transport::Error::InvalidHandle) | Ok(None)
            ));
            assert!(matches!(
                transport::close_query(handle),
                Err(transport::Error::InvalidHandle | transport::Error::Unavailable)
            ));
        }
    }
    if states
        .get(id)
        .is_some_and(|state| state.activation != activation)
    {
        let old = states.remove(id).unwrap();
        if let Some(handle) = old.queryable {
            assert!(transport::next_query(handle).is_err());
        }
        if let Some(handle) = old.outgoing {
            assert!(matches!(
                transport::poll_query(handle),
                Err(transport::Error::InvalidHandle) | Ok(None)
            ));
        }
    }
    let state = states.entry(id.clone()).or_insert(D8State {
        incoming: None,
        pending: None,
        activation,
        queryable: None,
        outgoing: None,
        subscription: None,
    });
    let trace = |kind: &str, value: &str| {
        transport::publish(
            "fixture/d8-events",
            format!("{kind}|{}|{id}|{activation}|{value}", current.scope_id).as_bytes(),
        )
        .unwrap();
    };
    if role == "d8-provider" {
        if state.queryable.is_none() {
            match transport::declare_polled("fixture/d8-echo") {
                Ok(handle) => {
                    state.queryable = Some(handle);
                    trace("ready", "provider");
                }
                Err(transport::Error::Unavailable) => return,
                Err(error) => panic!("declare: {error:?}"),
            }
        }
        if state.incoming.is_none() {
            state.incoming = match transport::next_query(state.queryable.unwrap()) {
                Ok(request) => request,
                Err(transport::Error::Unavailable) => return,
                Err(error) => panic!("next: {error:?}"),
            };
        }
        let Some(request) = &state.incoming else {
            return;
        };
        match transport::reply_query(request.handle, Some(&request.payload)) {
            Ok(()) => {
                trace(
                    "handled",
                    &format!(
                        "{}:{}",
                        request.caller.instance_id,
                        std::str::from_utf8(&request.payload).unwrap()
                    ),
                );
                state.incoming = None;
            }
            Err(transport::Error::Unavailable | transport::Error::LimitExceeded) => {}
            Err(error) => panic!("reply: {error:?}"),
        }
    } else {
        if state.subscription.is_none() {
            state.subscription = Some(transport::subscribe("fixture/d8-start").unwrap());
            trace("ready", "caller");
        }
        if let Some(handle) = state.outgoing {
            match transport::poll_query(handle) {
                Ok(Some(values)) => {
                    assert_eq!(values.len(), 1);
                    trace("done", std::str::from_utf8(&values[0]).unwrap());
                    state.outgoing = None;
                }
                Ok(None) => {}
                Err(error) => panic!("poll: {error:?}"),
            }
        }
        if state.outgoing.is_none() {
            if state.pending.is_none() {
                state.pending = transport::poll(state.subscription.unwrap())
                    .unwrap()
                    .into_iter()
                    .next();
            }
            if let Some(payload) = &state.pending {
                match transport::begin_query("fixture/d8-echo", payload) {
                    Ok(handle) => {
                        state.outgoing = Some(handle);
                        state.pending = None;
                    }
                    Err(transport::Error::Unavailable | transport::Error::LimitExceeded) => {}
                    Err(error) => panic!("begin: {error:?}"),
                }
            }
        }
    }
}
