//! P07 acceptance through real RTW/WASM callers, Host ingress and SQL COMMIT.

use crate::runtime::{WorldDataRuntimeControl, installed_world_schemas};
use crate::{HostHome, world_runtime_scope_id};
use rintawa_artifacts::{ArtifactDigest, RtwLimits, pack_directory};
use rintawa_extension_engine::{
    ExtensionEngine, RtwExtensionLoader, TransportError, WasmWorldSystemEndpoint,
};
use rintawa_sdk::{
    contracts::{ComponentPrincipal, ComponentRef},
    types::ExtensionInstanceId,
    world::{ItemId, SchemaKey, WorldId},
};
use rintawa_world::{
    PropertyDefinition, RuntimeSchemaDefinition, SchemaKind, WorldCommand, WorldTransaction,
};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntime, WorldRuntimeBuilder, WorldRuntimePolicy,
    WorldSnapshot, WorldSystem,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

const BACKEND: &[u8] =
    include_bytes!("../../../extension-engine/tests/fixtures/world-system/component.wasm");
const CONSUMER: &[u8] =
    include_bytes!("../../../extension-engine/tests/fixtures/transport/provider.wasm");
type EvaluationGate = Arc<Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>>;

struct GatedSystem {
    schema: SchemaKey,
    endpoint: WasmWorldSystemEndpoint,
    gate: EvaluationGate,
}
impl WorldSystem for GatedSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.schema
    }
    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            release
                .recv_timeout(Duration::from_secs(15))
                .map_err(|error| SystemError::Failed(error.to_string()))?;
        }
        self.endpoint.evaluate(snapshot.clone(), command)
    }
}
struct PausedEvaluation {
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl PausedEvaluation {
    fn wait(&self) -> anyhow::Result<()> {
        self.entered.recv_timeout(Duration::from_secs(5))?;
        Ok(())
    }
    fn resume(mut self) {
        let _ = self.release.take().unwrap().send(());
    }
}
impl Drop for PausedEvaluation {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    home: HostHome,
    world: WorldId,
    engine: ExtensionEngine,
    data: WorldDataRuntimeControl,
    runtime: Option<WorldRuntime>,
    backend: ComponentRef,
    consumer: ComponentPrincipal,
    digest: ArtifactDigest,
    gate: EvaluationGate,
    capacity: usize,
}
fn artifact(
    root: &Path,
    home: &HostHome,
    id: &str,
    bytes: &[u8],
) -> anyhow::Result<ArtifactDigest> {
    let source = root.join(id);
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    std::fs::write(
        source.join("manifest.toml"),
        format!(
            r#"id = "fixture.{id}"
name = "Command ticket fixture"
version = "0.0.1"
sdk = "^0.0"
[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "component.wasm"
[components.permissions]
runtime = ["asset-import", "asset-read", "user-resource-import", "user-resource-read"]
"#
        ),
    )?;
    std::fs::write(source.join("component.wasm"), bytes)?;
    let archive = root.join(format!("{id}.rtw"));
    pack_directory(source, &archive, RtwLimits::default())?;
    Ok(home
        .import_rtw_bytes(&std::fs::read(archive)?)?
        .digest()
        .clone())
}
impl Fixture {
    fn new(capacity: usize) -> anyhow::Result<Self> {
        Self::with_permissions(capacity, &[])
    }
    fn with_permissions(
        capacity: usize,
        permissions: &[rintawa_sdk::runtime_permissions::RuntimePermission],
    ) -> anyhow::Result<Self> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let world = home.create_world()?.id;
        let digest = artifact(root.path(), &home, "backend", BACKEND)?;
        let consumer_digest = artifact(root.path(), &home, "consumer", CONSUMER)?;
        let data = WorldDataRuntimeControl::default();
        let access = Arc::new(crate::runtime::LocalHostAccess::new(
            home.root(),
            crate::runtime::UserResourceRuntimeControl::default(),
            crate::runtime::WorldSessionRuntimeControl::default(),
            data.clone(),
        ));
        let mut engine = ExtensionEngine::with_host_access(
            access.clone(),
            access.clone(),
            access.clone(),
            access.clone(),
            access.clone(),
            access,
        );
        engine.attach_world_data_access(Arc::new(data.clone()));
        for (id, digest) in [("backend", &digest), ("consumer", &consumer_digest)] {
            RtwExtensionLoader::new().load_stored_extension(
                &mut engine,
                home.artifact_store(),
                digest,
                ExtensionInstanceId::new(id),
                world_runtime_scope_id(world),
            )?;
            if id == "backend" {
                for permission in permissions {
                    engine.grant_requested_runtime_permission_for_instance(
                        &ExtensionInstanceId::new(id),
                        &rintawa_sdk::types::ComponentId::new("runtime"),
                        *permission,
                    )?;
                }
            }
            engine.start_extension_instance(&ExtensionInstanceId::new(id))?;
        }
        let consumer =
            engine.active_component_principal(&ComponentRef::new("consumer", "runtime"))?;
        let mut fixture = Self {
            _root: root,
            home,
            world,
            engine,
            data,
            runtime: None,
            backend: ComponentRef::new("backend", "runtime"),
            consumer,
            digest,
            gate: Arc::new(Mutex::new(None)),
            capacity,
        };
        fixture.start_world()?;
        Ok(fixture)
    }
    fn start_world(&mut self) -> anyhow::Result<()> {
        // Read actual declarations from the loaded RTW. The wrapper only delays
        // evaluation; the authoritative transaction still comes from WASM.
        let schemas = installed_world_schemas(
            &self.engine,
            std::slice::from_ref(&self.backend.instance_id),
        )?;
        let endpoint = self.engine.world_system_endpoint(&self.backend)?;
        let mut builder = WorldRuntimeBuilder::new(self.home.open_world_storage(self.world)?);
        builder.set_policy(WorldRuntimePolicy::new(self.capacity));
        let mut definitions = Vec::new();
        let mut owners = BTreeMap::new();
        for schema in schemas {
            if schema.kind == SchemaKind::Property {
                definitions.push((
                    schema.declarer.instance_id,
                    PropertyDefinition::new(schema.key, schema.owner, schema.definition),
                ));
            } else {
                builder.register_runtime_schema(
                    schema.declarer.instance_id.clone(),
                    RuntimeSchemaDefinition::new(
                        schema.key.clone(),
                        schema.kind,
                        schema.owner,
                        schema.definition,
                    ),
                )?;
                owners.insert(schema.key.clone(), schema.declarer);
                builder.register_system(GatedSystem {
                    schema: schema.key,
                    endpoint: endpoint.clone(),
                    gate: self.gate.clone(),
                })?;
            }
        }
        let runtime = builder.start()?;
        runtime.register_property_definitions(&definitions)?;
        self.data.activate(self.world, runtime.client(), owners)?;
        self.runtime = Some(runtime);
        Ok(())
    }
    fn pause(&self) -> PausedEvaluation {
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        *self.gate.lock().unwrap() = Some((entered_tx, release_rx));
        PausedEvaluation {
            entered,
            release: Some(release),
        }
    }
    fn call(&self, request: Value) -> anyhow::Result<Value> {
        let mut payload = b"forward:fixture/commands\n".to_vec();
        payload.extend(serde_json::to_vec(&request)?);
        let replies = self.engine.query_transport_contributions(
            &self.consumer,
            "fixture/caller",
            &payload,
        )?;
        anyhow::ensure!(replies.len() == 1, "expected one backend reply");
        Ok(serde_json::from_slice(&replies[0])?)
    }
    fn submit(&self, schema: &str, item: ItemId) -> anyhow::Result<u64> {
        let value = self.call(json!({"op":"submit","schema":schema,"item":item.to_string()}))?;
        value["ticket"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("submit returned {value}"))
    }
    fn poll(&self, ticket: u64) -> anyhow::Result<Value> {
        self.call(json!({"op":"poll","ticket":ticket}))
    }
    fn close(&self, ticket: u64) -> anyhow::Result<Value> {
        self.call(json!({"op":"close","ticket":ticket}))
    }
    fn flush(&self) -> anyhow::Result<()> {
        // A metadata fence uses the same queue and waits for all earlier commits;
        // it performs no domain mutation on behalf of the guest.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match self
                .runtime
                .as_ref()
                .unwrap()
                .client()
                .set_manifest_metadata(json!({"title":"Ticket test"}))
            {
                Ok(_) => return Ok(()),
                Err(rintawa_world_runtime::WorldRuntimeError::QueueFull)
                    if std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn reload(&mut self) -> anyhow::Result<()> {
        self.data.deactivate(self.world);
        self.runtime.take().unwrap().shutdown()?;
        self.engine
            .stop_extension_instance(&self.backend.instance_id)?;
        self.engine
            .unregister_extension_instance(&self.backend.instance_id)?;
        RtwExtensionLoader::new().load_stored_extension(
            &mut self.engine,
            self.home.artifact_store(),
            &self.digest,
            self.backend.instance_id.clone(),
            world_runtime_scope_id(self.world),
        )?;
        self.engine
            .start_extension_instance(&self.backend.instance_id)?;
        self.start_world()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.data.deactivate(self.world);
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.shutdown();
        }
        let _ = self
            .engine
            .stop_extension_instance(&self.backend.instance_id);
        let _ = self
            .engine
            .stop_extension_instance(&self.consumer.instance_id);
    }
}

#[test]
fn test_guest_tickets_survive_callbacks_until_commit_and_reject_terminal_reuse()
-> anyhow::Result<()> {
    let mut fixture = Fixture::new(128)?;
    let item = ItemId::new();
    let pause = fixture.pause();
    let ticket = fixture.submit("fixture.create@1", item)?;
    pause.wait()?;
    assert_eq!(fixture.poll(ticket)?, json!({"state":"pending"}));
    assert_eq!(
        fixture.call(json!({"op":"read","item":item.to_string()}))?,
        json!({"exists":false})
    );
    pause.resume();
    fixture.flush()?;
    assert_eq!(fixture.poll(ticket)?, json!({"state":"committed"}));
    assert_eq!(fixture.poll(ticket)?, json!({"error":"invalid-request"}));
    assert_eq!(fixture.close(ticket)?, json!({"error":"invalid-request"}));
    assert_eq!(
        fixture.call(json!({"op":"read","item":item.to_string()}))?,
        json!({"exists":true})
    );
    fixture
        .runtime
        .as_ref()
        .unwrap()
        .snapshot()?
        .load_item(item)?
        .ok_or_else(|| anyhow::anyhow!("COMMIT not visible"))?;
    fixture.data.deactivate(fixture.world);
    fixture.runtime.take().unwrap().shutdown()?;
    let reopened = fixture.home.open_world_storage(fixture.world)?;
    assert_eq!(
        reopened
            .load_property(item, &"fixture.value@1".parse()?)?
            .unwrap()
            .data(),
        &json!(7)
    );
    Ok(())
}

#[test]
fn test_guest_tickets_report_definite_rejections_and_sql_rollback() -> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let item = ItemId::new();
    let create = fixture.submit("fixture.create@1", item)?;
    fixture.flush()?;
    assert_eq!(fixture.poll(create)?["state"], "committed");
    for schema in [
        "fixture.reject@1",
        "fixture.invalid@1",
        "fixture.conflict@1",
    ] {
        let ticket = fixture.submit(schema, item)?;
        fixture.flush()?;
        let failed = fixture.poll(ticket)?;
        assert_eq!(failed["state"], "failed", "{schema}: {failed}");
        assert!(failed["reason"].as_str().unwrap().len() <= 2048);
        assert_eq!(fixture.poll(ticket)?, json!({"error":"invalid-request"}));
        assert_eq!(
            fixture
                .runtime
                .as_ref()
                .unwrap()
                .snapshot()?
                .load_property(item, &"fixture.value@1".parse()?)?
                .unwrap()
                .data(),
            &json!(7)
        );
    }
    Ok(())
}

#[test]
fn test_guest_close_releases_quota_without_cancelling_admitted_commands() -> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let pause = fixture.pause();
    let mut tickets = Vec::new();
    let mut items = Vec::new();
    for _ in 0..16 {
        let item = ItemId::new();
        tickets.push(fixture.submit("fixture.create@1", item)?);
        items.push(item);
    }
    pause.wait()?;
    assert_eq!(
        fixture.call(
            json!({"op":"submit","schema":"fixture.create@1","item":ItemId::new().to_string()})
        )?,
        json!({"error":"queue-full"})
    );
    assert_eq!(fixture.close(tickets[0])?, json!({"closed":true}));
    assert_eq!(
        fixture.close(tickets[0])?,
        json!({"error":"invalid-request"})
    );
    let extra = ItemId::new();
    let replacement = fixture.submit("fixture.create@1", extra)?;
    assert!(!tickets.contains(&replacement));
    items.push(extra);
    pause.resume();
    fixture.flush()?;
    assert_eq!(
        fixture.poll(tickets[0])?,
        json!({"error":"invalid-request"})
    );
    // Completed-but-uncollected tickets still consume quota.
    assert_eq!(
        fixture.call(
            json!({"op":"submit","schema":"fixture.create@1","item":ItemId::new().to_string()})
        )?,
        json!({"error":"queue-full"})
    );
    for ticket in tickets.into_iter().skip(1).chain([replacement]) {
        assert_eq!(fixture.poll(ticket)?["state"], "committed");
    }
    for item in items {
        assert!(
            fixture
                .runtime
                .as_ref()
                .unwrap()
                .snapshot()?
                .load_item(item)?
                .is_some()
        );
    }
    assert_eq!(fixture.poll(u64::MAX)?, json!({"error":"invalid-request"}));
    Ok(())
}

#[test]
fn test_guest_queue_admission_failure_has_no_ticket_or_mutation() -> anyhow::Result<()> {
    let fixture = Fixture::new(1)?;
    let pause = fixture.pause();
    let first = fixture.submit("fixture.create@1", ItemId::new())?;
    pause.wait()?;
    let queued = fixture.submit("fixture.create@1", ItemId::new())?;
    let refused = ItemId::new();
    assert_eq!(
        fixture
            .call(json!({"op":"submit","schema":"fixture.create@1","item":refused.to_string()}))?,
        json!({"error":"queue-full"})
    );
    pause.resume();
    fixture.flush()?;
    assert_eq!(fixture.poll(first)?["state"], "committed");
    assert_eq!(fixture.poll(queued)?["state"], "committed");
    assert!(
        fixture
            .runtime
            .as_ref()
            .unwrap()
            .snapshot()?
            .load_item(refused)?
            .is_none()
    );
    Ok(())
}

#[test]
fn test_guest_stale_and_foreign_ticket_handles_fail_after_stop_trap_and_reload()
-> anyhow::Result<()> {
    let mut fixture = Fixture::new(128)?;
    let other = Fixture::new(128)?;
    let old = fixture.submit("fixture.create@1", ItemId::new())?;
    fixture.flush()?;
    let foreign = other.submit("fixture.create@1", ItemId::new())?;
    other.flush()?;
    assert_ne!(old, foreign);
    assert_eq!(other.poll(old)?, json!({"error":"invalid-request"}));
    assert_eq!(fixture.close(foreign)?, json!({"error":"invalid-request"}));
    fixture
        .engine
        .stop_extension_instance(&fixture.backend.instance_id)?;
    fixture
        .engine
        .start_extension_instance(&fixture.backend.instance_id)?;
    assert_eq!(fixture.poll(old)?, json!({"error":"invalid-request"}));
    let before_reload = fixture.submit("fixture.create@1", ItemId::new())?;
    fixture.flush()?;
    fixture.reload()?;
    let after_reload = fixture.submit("fixture.create@1", ItemId::new())?;
    fixture.flush()?;
    assert_ne!(after_reload, before_reload);
    assert_eq!(
        fixture.poll(before_reload)?,
        json!({"error":"invalid-request"})
    );
    // Invoke the trapping backend directly so failure does not trap the forwarder.
    assert!(matches!(
        fixture.engine.query_transport_contributions(
            &fixture.consumer,
            "fixture/commands",
            br#"{"op":"trap"}"#
        ),
        Err(TransportError::Unavailable)
    ));
    fixture.reload()?;
    assert_eq!(
        fixture.poll(after_reload)?,
        json!({"error":"invalid-request"})
    );
    assert_eq!(other.poll(foreign)?["state"], "committed");
    Ok(())
}

#[test]
fn test_guest_lost_submit_reply_does_not_cancel_or_replay_the_command() -> anyhow::Result<()> {
    let fixture = Fixture::new(128)?;
    let item = ItemId::new();
    let pause = fixture.pause();
    let request = serde_json::to_vec(
        &json!({"op":"submit-lost-reply","schema":"fixture.create@1","item":item.to_string()}),
    )?;
    assert!(matches!(
        fixture.engine.query_transport_contributions(
            &fixture.consumer,
            "fixture/commands",
            &request
        ),
        Err(TransportError::Unavailable)
    ));
    pause.wait()?;
    assert_eq!(fixture.call(json!({"op":"last"}))?["state"], "pending");
    pause.resume();
    fixture.flush()?;
    let lost_status = serde_json::to_vec(&json!({"op":"poll-lost-reply"}))?;
    assert!(matches!(
        fixture.engine.query_transport_contributions(
            &fixture.consumer,
            "fixture/commands",
            &lost_status
        ),
        Err(TransportError::Unavailable)
    ));
    // The terminal Core poll consumed its handle. The missing reply does not
    // establish failure and must be resolved by reading the known item ID.
    assert_eq!(
        fixture.call(json!({"op":"last"}))?,
        json!({"error":"invalid-request"})
    );
    assert_eq!(
        fixture.call(json!({"op":"read","item":item.to_string()}))?,
        json!({"exists":true})
    );
    Ok(())
}

#[test]
fn test_guest_stop_releases_pending_tickets_without_cancelling_admitted_work() -> anyhow::Result<()>
{
    let mut fixture = Fixture::new(128)?;
    let pause = fixture.pause();
    let mut old_tickets = Vec::new();
    let mut items = Vec::new();
    for _ in 0..16 {
        let item = ItemId::new();
        old_tickets.push(fixture.submit("fixture.create@1", item)?);
        items.push(item);
    }
    pause.wait()?;
    fixture
        .engine
        .stop_extension_instance(&fixture.backend.instance_id)?;
    fixture
        .engine
        .start_extension_instance(&fixture.backend.instance_id)?;
    assert_eq!(
        fixture.call(json!({"op":"last"}))?,
        json!({"error":"invalid-request"})
    );
    let mut current_tickets = Vec::new();
    for _ in 0..16 {
        let item = ItemId::new();
        current_tickets.push(fixture.submit("fixture.create@1", item)?);
        items.push(item);
    }
    pause.resume();
    fixture.flush()?;
    for ticket in old_tickets {
        assert_eq!(fixture.close(ticket)?, json!({"error":"invalid-request"}));
    }
    for ticket in current_tickets {
        assert_eq!(fixture.poll(ticket)?["state"], "committed");
    }
    for item in items {
        assert!(
            fixture
                .runtime
                .as_ref()
                .unwrap()
                .snapshot()?
                .load_item(item)?
                .is_some()
        );
    }
    Ok(())
}

mod data_primitive_tests;
