//! Integration tests for authoritative commits, rollbacks and pinned reads.
//!
//! The runtime is the authority over the persisted property vocabulary and over
//! the atomicity of one change set. A System proposal is validated against that
//! vocabulary before SQLite sees it, and a later invalid change rolls back every
//! earlier item and property change of the same command.

mod common;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rintawa_sdk::world::{ItemId, PrincipalId, SchemaKey, WorldId};
use rintawa_storage::{SqliteWorldStorage, StorageError};
use rintawa_world::{ItemKind, StateChange, WorldCommand, WorldError, WorldTransaction};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntimeError, WorldSnapshot, WorldSystem,
};

use common::{
    ScriptedSystem, activate_fixture_properties, command, create_storage, key, runtime_builder,
};

/// What one evaluation observed in its pinned snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeOutcome {
    world_matches: bool,
    item_present: bool,
    sequence: Option<i64>,
    property_count: usize,
}

/// System that records snapshot reads and appends the next sequence value.
struct SnapshotProbeSystem {
    command_schema: SchemaKey,
    item: ItemId,
    sequence: SchemaKey,
    world_id: WorldId,
    observed: Arc<Mutex<Vec<ProbeOutcome>>>,
}

impl WorldSystem for SnapshotProbeSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let world_matches = snapshot.world_id() == self.world_id;
        let item_present = snapshot.load_item(self.item)?.is_some();
        let sequence = snapshot
            .load_property(self.item, &self.sequence)?
            .map(|record| record.data().as_i64().unwrap_or_default());
        let property_count = snapshot.properties_of(self.item)?.len();
        self.observed
            .lock()
            .map_err(|_| SystemError::Failed(String::from("probe lock poisoned")))?
            .push(ProbeOutcome {
                world_matches,
                item_present,
                sequence,
                property_count,
            });

        let next = command.payload()["next"].as_i64().unwrap_or_default();
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::SetProperty {
            item: self.item,
            key: self.sequence.clone(),
            data: serde_json::json!(next),
        });
        Ok(transaction)
    }
}

#[test]
fn test_should_commit_items_and_properties_atomically() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let alice = ItemId::new();
    let forest = ItemId::new();
    let link = ItemId::new();

    let changes = vec![
        StateChange::CreateObject { id: alice },
        StateChange::CreateObject { id: forest },
        StateChange::CreateRelation {
            id: link,
            subject: alice,
            object: forest,
        },
        StateChange::SetProperty {
            item: alice,
            key: schemas.identity.clone(),
            data: serde_json::json!({ "name": "Alice" }),
        },
        StateChange::SetProperty {
            item: alice,
            key: schemas.mood.clone(),
            data: serde_json::json!({ "level": 3 }),
        },
    ];
    let system = ScriptedSystem::new(schemas.command.clone(), changes);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let submitted = command(&schemas.command, principal, serde_json::json!({}));
    let command_id = submitted.id();
    let outcome = runtime.submit(submitted)?.wait()?;
    assert_eq!(outcome.command_id(), command_id);

    let snapshot = runtime.snapshot()?;
    assert_eq!(snapshot.world_id(), runtime.world_id());
    let alice_record = snapshot.load_item(alice)?.expect("alice committed");
    assert_eq!(alice_record.kind(), ItemKind::Object);
    let link_record = snapshot.load_item(link)?.expect("link committed");
    assert!(link_record.is_relation());
    assert_eq!(link_record.subject(), Some(alice));
    assert_eq!(link_record.object_target(), Some(forest));
    let identity = snapshot
        .load_property(alice, &schemas.identity)?
        .expect("identity must be committed");
    assert_eq!(identity.data()["name"], "Alice");
    assert_eq!(snapshot.properties_of(alice)?.len(), 2);
    assert_eq!(snapshot.relations_pointing_at(forest)?.len(), 1);
    assert_eq!(snapshot.relations_pointing_at(alice)?.len(), 1);
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(&path)?;
    assert!(storage.load_item(alice)?.is_some());
    assert!(storage.load_item(link)?.is_some());
    assert_eq!(storage.properties_of(alice)?.len(), 2);
    let mood = storage
        .load_property(alice, &schemas.mood)?
        .expect("mood must be durable");
    assert_eq!(mood.data()["level"], 3);
    Ok(())
}

#[test]
fn test_should_roll_back_when_a_later_property_is_invalid() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let item = ItemId::new();

    let changes = vec![
        StateChange::CreateObject { id: item },
        StateChange::SetProperty {
            item,
            key: schemas.identity.clone(),
            data: serde_json::json!({ "name": "Valid" }),
        },
        StateChange::SetProperty {
            item,
            key: schemas.mood.clone(),
            data: serde_json::json!({ "level": 99 }),
        },
    ];
    let system = ScriptedSystem::new(schemas.command.clone(), changes);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "invalid-property" }),
    ))?;
    let error = ticket
        .wait()
        .expect_err("invalid property payload must fail the whole command");
    match error {
        WorldRuntimeError::InvalidTransaction { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                WorldError::PropertyValidationFailed { .. } => {}
                other => panic!("unexpected world error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }

    let snapshot = runtime.snapshot()?;
    assert!(snapshot.load_item(item)?.is_none());
    assert!(snapshot.properties_of(item)?.is_empty());
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(&path)?;
    assert!(storage.load_item(item)?.is_none());
    assert!(storage.properties_of(item)?.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_unregistered_property_before_commit() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let unknown = key("runtime-test.unknown@1");

    let changes = vec![
        StateChange::CreateObject { id: item },
        StateChange::SetProperty {
            item,
            key: unknown.clone(),
            data: serde_json::json!({ "value": true }),
        },
    ];
    let system = ScriptedSystem::new(schemas.command.clone(), changes);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "unknown-property" }),
    ))?;
    let error = ticket
        .wait()
        .expect_err("unregistered property must fail the whole command");
    match error {
        WorldRuntimeError::InvalidTransaction { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                WorldError::DefinitionNotRegistered { key } => {
                    assert_eq!(key, unknown);
                }
                other => panic!("unexpected world error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }

    assert!(runtime.snapshot()?.load_item(item)?.is_none());
    runtime.shutdown()?;
    assert!(SqliteWorldStorage::open(&path)?.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_roll_back_when_storage_rejects_a_later_change() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let created = ItemId::new();
    let missing = ItemId::new();
    let link = ItemId::new();

    let changes = vec![
        StateChange::CreateObject { id: created },
        StateChange::CreateRelation {
            id: link,
            subject: created,
            object: missing,
        },
    ];
    let system = ScriptedSystem::new(schemas.command.clone(), changes);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "dangling-relation" }),
    ))?;
    let error = ticket
        .wait()
        .expect_err("dangling relation must fail the whole command");
    let storage_error = match &error {
        WorldRuntimeError::Storage(storage_error) => storage_error,
        other => panic!("unexpected runtime error: {other:?}"),
    };
    let world_error = match storage_error {
        StorageError::World(world_error) => world_error,
        other => panic!("unexpected storage error: {other:?}"),
    };
    match world_error {
        WorldError::RelationEndpointAbsent(relation) => assert_eq!(relation, &link),
        other => panic!("unexpected world error: {other:?}"),
    }

    // The earlier object creation of the same change set must be rolled back.
    let snapshot = runtime.snapshot()?;
    assert!(snapshot.load_item(created)?.is_none());
    assert!(snapshot.load_item(link)?.is_none());
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(&path)?;
    assert!(storage.load_item(created)?.is_none());
    Ok(())
}

#[test]
fn test_should_evaluate_against_pinned_snapshot() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();

    let mut seed = WorldTransaction::new();
    seed.push_change(StateChange::CreateObject { id: item });
    seed.push_change(StateChange::SetProperty {
        item,
        key: schemas.sequence.clone(),
        data: serde_json::json!(0),
    });
    storage.apply(&seed)?;

    let observed = Arc::new(Mutex::new(Vec::new()));
    let system = SnapshotProbeSystem {
        command_schema: schemas.command.clone(),
        item,
        sequence: schemas.sequence.clone(),
        world_id,
        observed: Arc::clone(&observed),
    };
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let mut tickets = Vec::new();
    for next in [1, 2] {
        let submitted = command(
            &schemas.command,
            principal,
            serde_json::json!({ "next": next }),
        );
        let command_id = submitted.id();
        tickets.push((command_id, runtime.submit(submitted)?));
    }
    for (command_id, ticket) in tickets {
        assert_eq!(ticket.wait()?.command_id(), command_id);
    }

    let outcomes = observed.lock().expect("probe lock").clone();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].world_matches);
    assert!(outcomes[0].item_present);
    assert_eq!(outcomes[0].sequence, Some(0));
    assert_eq!(outcomes[0].property_count, 1);
    assert_eq!(outcomes[1].sequence, Some(1));
    assert_eq!(outcomes[1].property_count, 1);

    let snapshot = runtime.snapshot()?;
    let sequence = snapshot
        .load_property(item, &schemas.sequence)?
        .expect("sequence must be committed");
    assert_eq!(sequence.data(), &serde_json::json!(2));
    runtime.shutdown()?;

    let reopened = SqliteWorldStorage::open(&path)?;
    let durable = reopened
        .load_property(item, &schemas.sequence)?
        .expect("sequence must be durable");
    assert_eq!(durable.data(), &serde_json::json!(2));
    Ok(())
}
