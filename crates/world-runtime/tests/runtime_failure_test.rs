mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::world::{PrincipalId, SchemaKey};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{CommitDisposition, WorldCommand, WorldEventDraft, WorldTransaction};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntimeBuilder, WorldRuntimeError, WorldSnapshot, WorldSystem,
};

use common::{EchoSystem, command, create_storage};

struct PanicOnceSystem {
    command_schema: SchemaKey,
    event_schema: SchemaKey,
    invocations: Arc<AtomicUsize>,
}

impl WorldSystem for PanicOnceSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        if self.invocations.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("intentional runtime test panic");
        }

        let mut transaction = WorldTransaction::new();
        transaction.push_event(WorldEventDraft::new(
            self.event_schema.clone(),
            command.payload().clone(),
        ));
        Ok(transaction)
    }
}

struct RejectingSystem {
    command_schema: SchemaKey,
}

impl WorldSystem for RejectingSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        _command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        Err(SystemError::Rejected("test rejection".into()))
    }
}

#[test]
fn test_should_reject_duplicate_system_registration() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let system = EchoSystem::new(schemas.command.clone(), schemas.event.clone());
    let mut builder = WorldRuntimeBuilder::new(storage);

    builder.register_system(system.clone())?;
    let error = builder
        .register_system(system)
        .expect_err("duplicate command ownership must be rejected");

    assert!(matches!(
        error,
        WorldRuntimeError::DuplicateSystem(schema) if schema == schemas.command
    ));
    Ok(())
}

#[test]
fn test_should_report_unavailable_system_without_committing() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let runtime = WorldRuntimeBuilder::new(storage).start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "missing" }),
    ))?;
    let error = ticket.wait().unwrap_err();
    assert!(matches!(
        error,
        WorldRuntimeError::SystemUnavailable(schema) if schema == schemas.command
    ));
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_isolate_system_panic_and_continue_worker() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let invocations = Arc::new(AtomicUsize::new(0));

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(PanicOnceSystem {
        command_schema: schemas.command.clone(),
        event_schema: schemas.event.clone(),
        invocations: Arc::clone(&invocations),
    })?;
    let runtime = builder.start()?;

    let first = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "panic" }),
    ))?;
    let error = first.wait().unwrap_err();
    assert!(matches!(
        error,
        WorldRuntimeError::SystemPanicked(schema) if schema == schemas.command
    ));

    let second = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "after-panic" }),
    ))?;
    assert_eq!(second.wait()?.position(), 1);
    runtime.shutdown()?;
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn test_should_preserve_domain_system_error_without_commit() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(RejectingSystem {
        command_schema: schemas.command.clone(),
    })?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "reject" }),
    ))?;
    let error = ticket.wait().unwrap_err();
    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed { schema, .. } if schema == schemas.command
    ));
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_skip_system_on_identical_command_retry() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let system = EchoSystem::new(schemas.command.clone(), schemas.event.clone());
    let evaluations = Arc::clone(&system.evaluations);

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let command = command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "retry" }),
    );
    let first = runtime.submit(command.clone())?.wait()?;
    let replay = runtime.submit(command)?.wait()?;

    assert_eq!(first.disposition(), CommitDisposition::Committed);
    assert_eq!(replay.disposition(), CommitDisposition::AlreadyCommitted);
    assert_eq!(replay.position(), first.position());
    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
    runtime.shutdown()?;
    Ok(())
}
