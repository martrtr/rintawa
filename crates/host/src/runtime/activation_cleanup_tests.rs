//! R1: failed activation retains cleanup ownership before a World worker exists.
use super::*;
use rintawa_extension_engine::{
    ExtensionState, RtwComponentHost, RtwComponentHostResult, RtwComponentSource, WasmComponent,
    WasmRuntimeEngine,
};
use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    errors::{ExtensionError, ExtensionResult},
    manifest::{ComponentDescriptor, ExtensionManifest},
    traits::Component,
    types::ComponentId,
};
use std::sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

const TARGET: &str = "r1.test.runtime@1";
const WASM: &str = include_str!("../../../extension-engine/tests/fixtures/stateful_component.wat");
type HeldWasm = Arc<Mutex<WasmComponent>>;
struct Callback {
    release: mpsc::Sender<()>,
    worker: std::thread::JoinHandle<()>,
}
#[derive(Default)]
struct Probe {
    hold: AtomicBool,
    fail_start: AtomicBool,
    callback: Mutex<Option<Callback>>,
    wasm: Mutex<Vec<Weak<Mutex<WasmComponent>>>>,
    stops: Mutex<BTreeMap<String, usize>>,
}
impl Probe {
    fn release(&self) {
        self.hold.store(false, Ordering::Release);
        if let Some(callback) = self.callback.lock().unwrap().take() {
            let _ = callback.release.send(());
            callback.worker.join().unwrap();
        }
    }
    fn stops(&self, component: &str) -> usize {
        self.stops
            .lock()
            .unwrap()
            .get(component)
            .copied()
            .unwrap_or(0)
    }
    fn wasm_alive(&self) -> bool {
        self.wasm
            .lock()
            .unwrap()
            .iter()
            .any(|wasm| wasm.upgrade().is_some())
    }
}
struct TestComponent {
    id: ComponentId,
    wasm: Option<HeldWasm>,
    probe: Arc<Probe>,
}
impl Component for TestComponent {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn id(&self) -> &ComponentId {
        &self.id
    }
    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let key = ContractKey::new("r1.cleanup-order", ContractVersion::new(1));
        if self.id.as_str() == "done" {
            ctx.define_contract(rintawa_sdk::contracts::ContractDefinition::new(
                key.clone(),
                ContractResolutionPolicy::Single,
            ))?;
            ctx.provide_contract(rintawa_sdk::contracts::ContractProvider::new(key))?;
        } else if self.id.as_str() == "busy" {
            ctx.consume_contract(rintawa_sdk::contracts::ContractConsumer::new(key, true))?;
        }
        if let Some(wasm) = &self.wasm {
            wasm.lock().unwrap().register(ctx)?;
        }
        Ok(())
    }
    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        if self.id.as_str() == "fail" && self.probe.fail_start.load(Ordering::Acquire) {
            return Err(ExtensionError::Message(
                "injected later start failure".into(),
            ));
        }
        if let Some(wasm) = &self.wasm {
            wasm.lock().unwrap().start(ctx)?;
            if self.probe.hold.load(Ordering::Acquire) {
                let wasm = wasm.clone();
                let (entered_tx, entered_rx) = mpsc::channel();
                let (release, receiver) = mpsc::channel();
                let worker = std::thread::spawn(move || {
                    // Model a callback holding its component while a real WASM
                    // lifecycle Store and its linear memory remain owned.
                    let _guard = wasm.lock().unwrap();
                    entered_tx.send(()).unwrap();
                    let _ = receiver.recv();
                });
                *self.probe.callback.lock().unwrap() = Some(Callback { release, worker });
                entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        Ok(())
    }
    fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        if let Some(wasm) = &self.wasm {
            let mut wasm = wasm.try_lock().map_err(|_| ExtensionError::CleanupBusy)?;
            wasm.stop(ctx)?;
        }
        *self
            .probe
            .stops
            .lock()
            .unwrap()
            .entry(self.id.to_string())
            .or_default() += 1;
        Ok(())
    }
}
struct Target {
    runtime: WasmRuntimeEngine,
    probe: Arc<Probe>,
}
impl RtwComponentHost for Target {
    fn target(&self) -> &str {
        TARGET
    }
    fn load_component(
        &self,
        _: &mut RtwComponentSource<'_>,
        descriptor: &ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        let wasm = if descriptor.id.as_str() == "busy" {
            let component = self
                .runtime
                .load_component_from_bytes(descriptor.id.clone(), WASM.as_bytes())
                .map_err(|error| {
                    rintawa_extension_engine::RtwComponentHostError::Host(error.to_string())
                })?;
            let wasm = Arc::new(Mutex::new(component));
            self.probe.wasm.lock().unwrap().push(Arc::downgrade(&wasm));
            Some(wasm)
        } else {
            None
        };
        Ok(Box::new(TestComponent {
            id: descriptor.id.clone(),
            wasm,
            probe: self.probe.clone(),
        }))
    }
}
fn manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.into(),
        version: "0.0.1".into(),
        sdk: "^0.0".into(),
        components: Vec::new(),
    }
}
fn install_overlay(
    root: &std::path::Path,
    home: &HostHome,
    world: WorldId,
    name: &str,
    components: &[&str],
) -> anyhow::Result<ExtensionInstanceId> {
    let source = root.join(name);
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    let mut text =
        format!("id = \"{name}\"\nname = \"R1 overlay\"\nversion = \"0.0.1\"\nsdk = \"^0.0\"\n");
    for id in components {
        text.push_str(&format!("[[components]]\nid = \"{id}\"\nkind = \"runtime\"\ntarget = \"{TARGET}\"\nentry = \"payload.bin\"\n"));
    }
    std::fs::write(source.join("manifest.toml"), text)?;
    std::fs::write(source.join("payload.bin"), b"fixture")?;
    let archive = root.join(format!("{name}.rtw"));
    rintawa_artifacts::pack_directory(&source, &archive, rintawa_artifacts::RtwLimits::default())?;
    let digest = home
        .import_rtw_bytes(&std::fs::read(archive)?)?
        .digest()
        .clone();
    Ok(home
        .select_world_stored_rtw(world, &digest, Some(true))?
        .instance_id)
}

struct Harness {
    _root: tempfile::TempDir,
    home: HostHome,
    host: HostRuntime,
    world: WorldId,
    instance: ExtensionInstanceId,
    completed: ExtensionInstanceId,
    provider: ComponentRef,
    probe: Arc<Probe>,
}
impl Harness {
    fn new(composition_failure: bool) -> anyhow::Result<Self> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world = home.create_world()?.id;
        let mut host = HostRuntime::start(&home)?;
        let probe = Arc::new(Probe::default());
        probe.hold.store(true, Ordering::Release);
        probe
            .fail_start
            .store(composition_failure, Ordering::Release);
        let provider = ComponentRef::new("r1-provider", "provider");
        // Trusted native embedding installs the target owner in baseline tracking.
        host.engine.register_extension_instance(
            provider.instance_id.clone(),
            RuntimeScopeId::new(HOST_SCOPE),
            manifest("r1-provider"),
            vec![Box::new(TestComponent {
                id: provider.component_id.clone(),
                wasm: None,
                probe: probe.clone(),
            })],
        )?;
        host.engine
            .start_extension_instance(&provider.instance_id)?;
        host.started_instances.push(provider.instance_id.clone());
        host.engine.register_execution_target_host(
            &provider,
            Arc::new(Target {
                runtime: host.engine.wasm_runtime_engine()?,
                probe: probe.clone(),
            }),
        )?;
        let completed = install_overlay(root.path(), &home, world, "r1.done", &["done"])?;
        let instance = install_overlay(
            root.path(),
            &home,
            world,
            "r1.overlay",
            &["completed", "busy", "fail"],
        )?;
        Ok(Self {
            _root: root,
            home,
            host,
            world,
            instance,
            completed,
            provider,
            probe,
        })
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.probe.release();
    }
}

fn regression(composition_failure: bool, shutdown: bool) -> anyhow::Result<()> {
    let mut fixture = Harness::new(composition_failure)?;
    // Refuse prepare_world_runtime deterministically after all starts, without
    // injecting domain data or constructing a fake runtime on the failure path.
    let writer = if composition_failure {
        None
    } else {
        Some(fixture.home.open_world_storage(fixture.world)?)
    };
    let error = fixture
        .host
        .activate_world(&fixture.home, fixture.world)
        .unwrap_err();
    assert!(
        matches!(error, HostError::BootstrapRollback(_)),
        "{error:?}"
    );
    if let HostError::BootstrapRollback(rollback) = &error {
        if composition_failure {
            assert!(
                matches!(
                    rollback.primary.as_ref(),
                    HostError::Engine(EngineError::StartupRollbackFailed { .. })
                ),
                "{error:?}"
            );
        } else {
            assert!(
                matches!(
                    rollback.primary.as_ref(),
                    HostError::Storage(rintawa_storage::StorageError::WriterBusy(_))
                ),
                "{error:?}"
            );
        }
    }
    assert!(!fixture.host.is_world_active(fixture.world));
    assert_eq!(
        fixture
            .host
            .engine
            .extension_instance_state(&fixture.instance),
        Some(ExtensionState::Stopping)
    );
    assert!(fixture.probe.wasm_alive());
    assert_eq!(fixture.probe.stops("completed"), 1);
    assert_eq!(fixture.probe.stops("done"), 1);
    assert_eq!(
        fixture
            .host
            .engine
            .extension_instance_state(&fixture.completed),
        None
    );
    if shutdown {
        assert!(
            fixture.host.shutdown().is_err(),
            "busy rollback must not report successful shutdown"
        );
    } else {
        assert!(
            matches!(
                fixture.host.deactivate_world(fixture.world),
                Err(HostError::ShutdownFailed(_))
            ),
            "failed activation cleanup must be reachable without an active worker"
        );
    }
    fixture.probe.release();
    assert!(
        fixture.probe.wasm_alive(),
        "released callback leaves ownership with the Engine until cleanup"
    );
    if shutdown {
        fixture.host.shutdown()?;
    } else {
        fixture.host.deactivate_world(fixture.world)?;
    }
    assert_eq!(
        fixture
            .host
            .engine
            .extension_instance_state(&fixture.instance),
        None
    );
    assert!(
        !fixture.probe.wasm_alive(),
        "unregister must drop the real WASM component/Store memory"
    );
    assert_eq!(
        fixture.probe.stops("completed"),
        1,
        "completed cleanup must not repeat"
    );
    assert_eq!(fixture.probe.stops("busy"), 1);
    assert_eq!(
        fixture.probe.stops("done"),
        1,
        "unregistered instances must be pruned from retry tracking"
    );
    drop(writer);
    if shutdown {
        assert_eq!(
            fixture
                .host
                .engine
                .extension_instance_state(&fixture.provider.instance_id),
            None
        );
        assert_eq!(fixture.host.engine.execution_target_owner(TARGET), None);
    } else {
        fixture.probe.fail_start.store(false, Ordering::Release);
        fixture.host.activate_world(&fixture.home, fixture.world)?;
        fixture.host.deactivate_world(fixture.world)?;
        fixture.host.shutdown()?;
    }
    Ok(())
}
#[test]
fn test_composition_rollback_shutdown_retries_retained_wasm_cleanup() -> anyhow::Result<()> {
    regression(true, true)
}
#[test]
fn test_prepare_rollback_shutdown_retries_retained_wasm_cleanup() -> anyhow::Result<()> {
    regression(false, true)
}
#[test]
fn test_composition_rollback_deactivate_cleans_before_reactivation() -> anyhow::Result<()> {
    regression(true, false)
}
#[test]
fn test_prepare_rollback_deactivate_cleans_before_reactivation() -> anyhow::Result<()> {
    regression(false, false)
}

#[test]
fn test_failed_activation_retries_cleanup_before_loading_same_instance_ids() -> anyhow::Result<()> {
    let mut fixture = Harness::new(true)?;
    assert!(
        fixture
            .host
            .activate_world(&fixture.home, fixture.world)
            .is_err()
    );
    let retry = fixture.host.activate_world(&fixture.home, fixture.world);
    assert!(
        matches!(retry, Err(HostError::ShutdownFailed(_))),
        "must retry retained cleanup, not duplicate registration: {retry:?}"
    );
    fixture.probe.release();
    fixture.probe.fail_start.store(false, Ordering::Release);
    fixture.host.activate_world(&fixture.home, fixture.world)?;
    assert!(fixture.host.is_world_active(fixture.world));
    assert_eq!(fixture.probe.stops("completed"), 1);
    fixture.host.shutdown()?;
    assert!(!fixture.probe.wasm_alive());
    Ok(())
}

#[test]
fn test_deferred_stop_retries_cleanup_of_world_that_never_became_active() -> anyhow::Result<()> {
    let mut fixture = Harness::new(true)?;
    assert!(
        fixture
            .host
            .activate_world(&fixture.home, fixture.world)
            .is_err()
    );
    let access = fixture.host.host_access.as_ref().unwrap().clone();
    for released in [false, true] {
        if released {
            fixture.probe.release();
        }
        WorldSessionAccess::set_active(&*access, &fixture.world.to_string(), false)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        fixture.host.poll_runtime()?;
        assert_eq!(
            fixture
                .host
                .engine
                .extension_instance_state(&fixture.instance)
                .is_none(),
            released
        );
        assert_eq!(fixture.probe.wasm_alive(), !released);
    }
    assert_eq!(fixture.probe.stops("completed"), 1);
    fixture.host.shutdown()?;
    Ok(())
}

#[test]
fn test_failed_activation_cleanup_counts_toward_world_admission_until_released()
-> anyhow::Result<()> {
    let mut fixture = Harness::new(true)?;
    assert!(
        fixture
            .host
            .activate_world(&fixture.home, fixture.world)
            .is_err()
    );
    for _ in 0..7 {
        let world = fixture.home.create_world()?.id;
        fixture.host.activate_world(&fixture.home, world)?;
    }
    let next = fixture.home.create_world()?.id;
    assert!(matches!(
        fixture.host.activate_world(&fixture.home, next),
        Err(HostError::ActiveWorldLimitExceeded)
    ));
    fixture.probe.release();
    fixture.host.deactivate_world(fixture.world)?;
    fixture.host.activate_world(&fixture.home, next)?;
    fixture.host.shutdown()?;
    assert!(!fixture.probe.wasm_alive());
    Ok(())
}
