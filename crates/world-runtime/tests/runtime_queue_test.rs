//! Integration tests for the bounded single-writer command queue.
//!
//! The runtime serializes evaluation and commit for one World. These tests pin the
//! two guarantees that survive the four-table redesign: commands are evaluated and
//! committed in submission order, and a bounded queue refuses work instead of
//! growing without limit.

mod common;

use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::world::{ItemId, PrincipalId, SchemaKey};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{StateChange, WorldCommand, WorldTransaction};
use rintawa_world_runtime::{
    MAX_COMMAND_QUEUE_CAPACITY, SystemResult, WorldRuntimeError, WorldRuntimePolicy, WorldSnapshot,
    WorldSystem,
};

use common::{RecordingSystem, command, create_storage, payload_item, runtime_builder};

/// System that blocks its first evaluation until the test releases it.
struct BlockingFirstSystem {
    command_schema: SchemaKey,
    invocations: Arc<AtomicUsize>,
    started: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl WorldSystem for BlockingFirstSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        if self.invocations.fetch_add(1, Ordering::SeqCst) == 0 {
            self.started.wait();
            self.release.wait();
        }

        let item = payload_item(command, "item")?;
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: item });
        Ok(transaction)
    }
}

#[test]
fn test_should_process_commands_in_fifo_order() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let system = RecordingSystem::new(schemas.command.clone());
    let observed = system.observed_sequences();
    let evaluations = system.evaluations();

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let items: Vec<ItemId> = (0..3).map(|_| ItemId::new()).collect();
    let mut tickets = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let submitted = command(
            &schemas.command,
            principal,
            serde_json::json!({ "sequence": index + 1, "item": item.to_string() }),
        );
        let command_id = submitted.id();
        let ticket = runtime.submit(submitted)?;
        assert_eq!(ticket.command_id(), command_id);
        tickets.push((command_id, ticket));
    }

    for (command_id, ticket) in tickets {
        let outcome = ticket.wait()?;
        assert_eq!(outcome.command_id(), command_id);
    }
    assert_eq!(evaluations.load(Ordering::SeqCst), 3);
    assert_eq!(*observed.lock().expect("observed lock"), vec![1, 2, 3]);
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(&path)?;
    for item in items {
        let record = storage
            .load_item(item)?
            .expect("queued command must be committed");
        assert!(!record.is_relation());
    }
    Ok(())
}

#[test]
fn test_should_apply_bounded_queue_backpressure() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let invocations = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let system = BlockingFirstSystem {
        command_schema: schemas.command.clone(),
        invocations: Arc::clone(&invocations),
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    };

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.set_policy(WorldRuntimePolicy::new(1));
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let first = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": ItemId::new().to_string() }),
    ))?;
    started.wait();

    let second = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": ItemId::new().to_string() }),
    ))?;
    // Capacity one means one running evaluation plus one waiting job.
    let overflow = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": ItemId::new().to_string() }),
    ));

    release.wait();
    let error = match overflow {
        Err(error) => error,
        Ok(_) => panic!("third command must exceed bounded queue capacity"),
    };
    assert!(matches!(error, WorldRuntimeError::QueueFull));

    first.wait()?;
    second.wait()?;
    runtime.shutdown()?;
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn test_should_reject_invalid_queue_capacity() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let mut builder = runtime_builder(storage, &[])?;
    builder.set_policy(WorldRuntimePolicy::new(0));

    let error = match builder.start() {
        Err(error) => error,
        Ok(_) => panic!("zero queue capacity must be rejected"),
    };
    assert!(matches!(
        error,
        WorldRuntimeError::InvalidQueueCapacity { capacity: 0, .. }
    ));

    let (_root, storage, _schemas) = create_storage()?;
    let mut builder = runtime_builder(storage, &[])?;
    builder.set_policy(WorldRuntimePolicy::new(MAX_COMMAND_QUEUE_CAPACITY + 1));

    let error = match builder.start() {
        Err(error) => error,
        Ok(_) => panic!("oversized queue capacity must be rejected"),
    };
    match error {
        WorldRuntimeError::InvalidQueueCapacity { capacity, .. } => {
            assert_eq!(capacity, MAX_COMMAND_QUEUE_CAPACITY + 1);
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    Ok(())
}

#[test]
fn test_should_revoke_cloned_client_before_worker_join() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let system = RecordingSystem::new(schemas.command.clone());
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    let client = runtime.client();

    // Leaving a completed-or-pending ticket unconsumed must not retain a sender
    // clone and prevent shutdown from joining the authoritative worker.
    let unused_ticket = client.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 1, "item": ItemId::new().to_string() }),
    ))?;
    runtime.shutdown()?;

    assert!(matches!(
        client.snapshot_for_extension(rintawa_sdk::types::ExtensionId::new(common::EXTENSION)),
        Err(WorldRuntimeError::RuntimeStopped)
    ));
    assert!(matches!(
        client.submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({ "sequence": 2, "item": ItemId::new().to_string() }),
        )),
        Err(WorldRuntimeError::RuntimeStopped)
    ));
    drop(unused_ticket);
    Ok(())
}
