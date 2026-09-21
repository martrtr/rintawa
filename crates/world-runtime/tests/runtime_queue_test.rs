mod common;

use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::world::{PrincipalId, SchemaKey};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{WorldCommand, WorldEventDraft, WorldTransaction};
use rintawa_world_runtime::{
    MAX_COMMAND_QUEUE_CAPACITY, SystemResult, WorldRuntimeBuilder, WorldRuntimeError,
    WorldRuntimePolicy, WorldSnapshot, WorldSystem,
};

use common::{EchoSystem, command, create_storage};

struct BlockingFirstSystem {
    command_schema: SchemaKey,
    event_schema: SchemaKey,
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

        let mut transaction = WorldTransaction::new();
        transaction.push_event(WorldEventDraft::new(
            self.event_schema.clone(),
            command.payload().clone(),
        ));
        Ok(transaction)
    }
}

#[test]
fn test_should_process_commands_in_fifo_order() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(EchoSystem::new(
        schemas.command.clone(),
        schemas.event.clone(),
    ))?;
    let runtime = builder.start()?;

    let first = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 1 }),
    ))?;
    let second = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 2 }),
    ))?;
    let third = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 3 }),
    ))?;

    assert_eq!(first.wait()?.position(), 1);
    assert_eq!(second.wait()?.position(), 2);
    assert_eq!(third.wait()?.position(), 3);
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    let events = storage.events_after(0, 10)?;
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].payload, serde_json::json!({ "sequence": 1 }));
    assert_eq!(events[1].payload, serde_json::json!({ "sequence": 2 }));
    assert_eq!(events[2].payload, serde_json::json!({ "sequence": 3 }));
    Ok(())
}

#[test]
fn test_should_apply_bounded_queue_backpressure() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let invocations = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.set_policy(WorldRuntimePolicy::new(1));
    builder.register_system(BlockingFirstSystem {
        command_schema: schemas.command.clone(),
        event_schema: schemas.event.clone(),
        invocations: Arc::clone(&invocations),
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    })?;
    let runtime = builder.start()?;

    let first = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 1 }),
    ))?;
    started.wait();

    let second = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 2 }),
    ))?;
    let error = match runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "sequence": 3 }),
    )) {
        Err(error) => error,
        Ok(_) => panic!("third command must exceed bounded queue capacity"),
    };
    assert!(matches!(error, WorldRuntimeError::QueueFull));

    release.wait();
    assert_eq!(first.wait()?.position(), 1);
    assert_eq!(second.wait()?.position(), 2);
    runtime.shutdown()?;
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn test_should_reject_invalid_queue_capacity() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let mut builder = WorldRuntimeBuilder::new(storage);
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
    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.set_policy(WorldRuntimePolicy::new(MAX_COMMAND_QUEUE_CAPACITY + 1));

    let error = match builder.start() {
        Err(error) => error,
        Ok(_) => panic!("oversized queue capacity must be rejected"),
    };
    assert!(matches!(
        error,
        WorldRuntimeError::InvalidQueueCapacity { capacity, .. }
            if capacity == MAX_COMMAND_QUEUE_CAPACITY + 1
    ));
    Ok(())
}
