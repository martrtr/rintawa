//! Integration tests for fail-closed World Runtime behavior.
//!
//! A failing or panicking System must never leave partial state behind, and the
//! single worker must keep serving later commands afterwards.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::world::{ItemId, PrincipalId, SchemaKey};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{RuntimeSignal, StateChange, WorldCommand, WorldError, WorldTransaction};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntimeError, WorldSnapshot, WorldSystem,
};

use common::{
    RecordingSystem, ScriptedSystem, command, create_storage, payload_item, runtime_builder,
};

/// System that panics on its first evaluation and succeeds afterwards.
struct PanicOnceSystem {
    command_schema: SchemaKey,
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
            panic!("intentional World System test panic");
        }

        let item = payload_item(command, "item")?;
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: item });
        Ok(transaction)
    }
}

/// System that rejects every command with one fixed domain diagnostic.
struct RejectingSystem {
    command_schema: SchemaKey,
    reason: &'static str,
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
        Err(SystemError::Rejected(self.reason.to_owned()))
    }
}

#[test]
fn test_should_reject_duplicate_system_registration() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    let system = RecordingSystem::new(schemas.command.clone());

    builder.register_system(system.clone())?;
    let error = builder
        .register_system(system)
        .expect_err("duplicate command ownership must be rejected");
    match error {
        WorldRuntimeError::DuplicateSystem(schema) => {
            assert_eq!(schema, schemas.command);
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    Ok(())
}

#[test]
fn test_should_report_unavailable_system_without_committing() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let runtime = runtime_builder(storage, &[&schemas.command])?.start()?;

    let item = ItemId::new();
    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": item.to_string() }),
    ))?;
    let error = ticket.wait().expect_err("missing System must not commit");
    match error {
        WorldRuntimeError::SystemUnavailable(schema) => {
            assert_eq!(schema, schemas.command);
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert!(storage.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_isolate_system_panic_and_continue_worker() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let principal = PrincipalId::new();
    let invocations = Arc::new(AtomicUsize::new(0));
    let system = PanicOnceSystem {
        command_schema: schemas.command.clone(),
        invocations: Arc::clone(&invocations),
    };
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let panicking_item = ItemId::new();
    let first = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": panicking_item.to_string() }),
    ))?;
    let error = first.wait().expect_err("panicking System must fail closed");
    match error {
        WorldRuntimeError::SystemPanicked(schema) => {
            assert_eq!(schema, schemas.command);
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    assert!(runtime.snapshot()?.load_item(panicking_item)?.is_none());

    let recovered_item = ItemId::new();
    let submitted = command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": recovered_item.to_string() }),
    );
    let command_id = submitted.id();
    let second = runtime.submit(submitted)?;
    assert_eq!(second.wait()?.command_id(), command_id);
    assert!(runtime.snapshot()?.load_item(recovered_item)?.is_some());

    runtime.shutdown()?;
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn test_should_preserve_domain_system_error_without_commit() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let system = RejectingSystem {
        command_schema: schemas.command.clone(),
        reason: "test rejection",
    };
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": item.to_string() }),
    ))?;
    let error = ticket.wait().expect_err("domain rejection must not commit");
    match error {
        WorldRuntimeError::SystemFailed { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                SystemError::Rejected(reason) => assert_eq!(reason, "test rejection"),
                other => panic!("unexpected System error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert!(storage.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_reject_empty_change_set_without_commit() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let signal = RuntimeSignal::new(
        schemas.signal.clone(),
        serde_json::json!({ "ignored": true }),
    );
    // Signals alone are best-effort post-COMMIT notifications, never a change.
    let system = ScriptedSystem::new(schemas.command.clone(), Vec::new(), vec![signal]);
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "signal-only" }),
    ))?;
    let error = ticket.wait().expect_err("empty change set must not commit");
    match error {
        WorldRuntimeError::InvalidTransaction { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                WorldError::EmptyTransaction => {}
                other => panic!("unexpected world error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert_eq!(storage.load_session()?.properties().len(), 3);
    Ok(())
}
