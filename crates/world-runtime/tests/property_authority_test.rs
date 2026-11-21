//! Regression tests for Host-approved property ownership at the World boundary.
//!
//! SQLite persists JSON schemas, not extension authorization. The authoritative
//! command schema owner comes from Host-installed runtime registration, while
//! property ownership is rebound from active installed definitions each session.
//! System reads and writes must not infer authority from a matching schema body,
//! a forged command Principal, or ON DELETE CASCADE.

mod common;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rintawa_sdk::{
    types::{ExtensionId, ExtensionInstanceId},
    world::{ItemId, PrincipalId, SchemaKey},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    PropertyDefinition, RelationDirection, RuntimeSchemaDefinition, SchemaKind, StateChange,
    WorldCommand, WorldError, WorldTransaction,
};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldReadError, WorldRuntime, WorldRuntimeError, WorldSnapshot,
    WorldSystem,
};

use common::{
    ScriptedSystem, activate_fixture_properties, command, create_storage, fixture_definitions, key,
    property_bindings, runtime_builder,
};

fn foreign_definition() -> PropertyDefinition {
    PropertyDefinition::new(
        key("foreign.private@1"),
        ExtensionId::new("foreign.package"),
        serde_json::json!({ "type": "object" }),
    )
}

fn seed_object_property(
    storage: &SqliteWorldStorage,
    item: ItemId,
    key: SchemaKey,
    value: serde_json::Value,
) -> Result<()> {
    let mut transaction = WorldTransaction::new();
    transaction.push_change(StateChange::CreateObject { id: item });
    transaction.push_change(StateChange::SetProperty {
        item,
        key,
        data: value,
    });
    storage.apply(&transaction)?;
    Ok(())
}

fn expect_denied(
    runtime: &WorldRuntime,
    command_schema: &SchemaKey,
    denied_key: &SchemaKey,
    operation: &str,
) -> Result<()> {
    // The caller's Principal never grants property ownership.
    let submitted = command(command_schema, PrincipalId::new(), serde_json::json!({}));
    let result = runtime.submit(submitted)?.wait();
    match result {
        Err(WorldRuntimeError::InvalidTransaction {
            schema,
            source:
                WorldError::PropertyAccessDenied {
                    key,
                    operation: actual,
                },
        }) => {
            assert_eq!(&schema, command_schema);
            assert_eq!(&key, denied_key);
            assert_eq!(actual, operation);
        }
        other => panic!("expected property owner rejection, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_should_rollback_foreign_property_set_and_all_preceding_mutations() -> Result<()> {
    let (directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    let item = ItemId::new();
    let transaction = vec![
        StateChange::CreateObject { id: item },
        StateChange::SetProperty {
            item,
            key: foreign.key().clone(),
            data: serde_json::json!({ "secret": "do-not-write" }),
        },
    ];

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(schemas.command.clone(), transaction))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;

    expect_denied(&runtime, &schemas.command, foreign.key(), "set")?;
    assert!(runtime.snapshot()?.load_item(item)?.is_none());

    runtime.shutdown()?;
    let reopened = SqliteWorldStorage::open(directory.path().join("world.sqlite"))?;
    assert!(reopened.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_refuse_foreign_remove_and_preserve_property_value() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    let item = ItemId::new();
    storage.register_definition(&foreign)?;
    seed_object_property(
        &storage,
        item,
        foreign.key().clone(),
        serde_json::json!({ "secret": "still-protected" }),
    )?;

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::RemoveProperty {
            item,
            key: foreign.key().clone(),
        }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;

    expect_denied(&runtime, &schemas.command, foreign.key(), "remove")?;
    assert_eq!(
        runtime
            .snapshot()?
            .load_property(item, foreign.key())?
            .expect("foreign value preserved")
            .data()["secret"],
        "still-protected"
    );
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_refuse_deleting_object_that_contains_foreign_property() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    let item = ItemId::new();
    storage.register_definition(&foreign)?;
    seed_object_property(
        &storage,
        item,
        foreign.key().clone(),
        serde_json::json!({ "secret": "protected" }),
    )?;

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::DeleteItem { id: item }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;

    expect_denied(&runtime, &schemas.command, foreign.key(), "delete")?;
    assert!(runtime.snapshot()?.load_item(item)?.is_some());
    assert!(
        runtime
            .snapshot()?
            .load_property(item, foreign.key())?
            .is_some()
    );
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_refuse_object_deletion_that_cascades_into_foreign_relation() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    let subject = ItemId::new();
    let object = ItemId::new();
    let relation = ItemId::new();
    storage.register_definition(&foreign)?;

    let mut initial = WorldTransaction::new();
    initial.push_change(StateChange::CreateObject { id: subject });
    initial.push_change(StateChange::CreateObject { id: object });
    initial.push_change(StateChange::CreateRelation {
        id: relation,
        subject,
        object,
    });
    initial.push_change(StateChange::SetProperty {
        item: relation,
        key: foreign.key().clone(),
        data: serde_json::json!({ "secret": "relation-metadata" }),
    });
    storage.apply(&initial)?;

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::DeleteItem { id: subject }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;

    expect_denied(&runtime, &schemas.command, foreign.key(), "delete")?;
    let snapshot = runtime.snapshot()?;
    assert!(snapshot.load_item(subject)?.is_some());
    assert!(snapshot.load_item(relation)?.is_some());
    assert!(snapshot.load_property(relation, foreign.key())?.is_some());
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_allow_own_delete_and_relation_cascade_without_touching_unrelated_items() -> Result<()>
{
    let (_directory, storage, schemas) = create_storage()?;
    let own = schemas.identity.clone();
    let subject = ItemId::new();
    let other = ItemId::new();
    let relation = ItemId::new();

    let mut initial = WorldTransaction::new();
    initial.push_change(StateChange::CreateObject { id: subject });
    initial.push_change(StateChange::CreateObject { id: other });
    initial.push_change(StateChange::CreateRelation {
        id: relation,
        subject,
        object: other,
    });
    initial.push_change(StateChange::SetProperty {
        item: relation,
        key: own.clone(),
        data: serde_json::json!({ "name": "owned relation" }),
    });
    storage.apply(&initial)?;

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::DeleteItem { id: subject }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let submitted = command(&schemas.command, PrincipalId::new(), serde_json::json!({}));
    let command_id = submitted.id();
    let outcome = runtime.submit(submitted)?.wait()?;
    assert_eq!(outcome.command_id(), command_id);
    let snapshot = runtime.snapshot()?;
    assert!(snapshot.load_item(subject)?.is_none());
    assert!(snapshot.load_item(relation)?.is_none());
    assert!(snapshot.load_item(other)?.is_some());
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_allow_removing_own_property_without_touching_other_value() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    storage.register_definition(&foreign)?;
    let item = ItemId::new();
    seed_object_property(
        &storage,
        item,
        schemas.identity.clone(),
        serde_json::json!({ "name": "owned" }),
    )?;

    let mut separate = WorldTransaction::new();
    separate.push_change(StateChange::SetProperty {
        item,
        key: foreign.key().clone(),
        data: serde_json::json!({ "secret": "foreign" }),
    });
    storage.apply(&separate)?;

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::RemoveProperty {
            item,
            key: schemas.identity.clone(),
        }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;

    let snapshot = runtime.snapshot()?;
    assert!(snapshot.load_property(item, &schemas.identity)?.is_none());
    assert!(snapshot.load_property(item, foreign.key())?.is_some());
    runtime.shutdown()?;
    Ok(())
}

struct FilterProbeSystem {
    schema: SchemaKey,
    item: ItemId,
    foreign_key: SchemaKey,
    created: ItemId,
    observed: Arc<Mutex<Vec<SchemaKey>>>,
}

impl WorldSystem for FilterProbeSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        _command: &rintawa_world::WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let visible = snapshot
            .properties_of(self.item)?
            .into_iter()
            .map(|value| value.key().clone())
            .collect::<Vec<_>>();
        *self
            .observed
            .lock()
            .map_err(|_| SystemError::Failed(String::from("test observation lock poisoned")))? =
            visible;

        assert!(matches!(
            snapshot.load_property(self.item, &self.foreign_key),
            Err(WorldReadError::PropertyAccessDenied(key)) if key == self.foreign_key
        ));
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: self.created });
        Ok(transaction)
    }
}

#[test]
fn test_should_filter_foreign_properties_from_system_snapshot_enumeration() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    storage.register_definition(&foreign)?;
    let item = ItemId::new();
    let created = ItemId::new();
    seed_object_property(
        &storage,
        item,
        schemas.identity.clone(),
        serde_json::json!({ "name": "visible" }),
    )?;
    let mut other = WorldTransaction::new();
    other.push_change(StateChange::SetProperty {
        item,
        key: foreign.key().clone(),
        data: serde_json::json!({ "secret": "hidden" }),
    });
    storage.apply(&other)?;
    let observed = Arc::new(Mutex::new(Vec::new()));

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(FilterProbeSystem {
        schema: schemas.command.clone(),
        item,
        foreign_key: foreign.key().clone(),
        created,
        observed: Arc::clone(&observed),
    })?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(&[foreign]))?;
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;

    assert_eq!(
        *observed.lock().expect("observation lock"),
        vec![schemas.identity]
    );
    assert_eq!(runtime.snapshot()?.properties_of(item)?.len(), 2);
    assert!(runtime.snapshot()?.load_item(created)?.is_some());
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_owner_collision_before_any_batch_persistence() -> Result<()> {
    let (directory, storage, schemas) = create_storage()?;
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![StateChange::CreateObject { id: ItemId::new() }],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;

    let key = key("shared.same-schema@1");
    let body = serde_json::json!({ "type": "string" });
    let first =
        PropertyDefinition::new(key.clone(), ExtensionId::new("package.first"), body.clone());
    let second = PropertyDefinition::new(key.clone(), ExtensionId::new("package.second"), body);

    assert!(matches!(
        runtime.register_property_definitions(&property_bindings(&[first.clone(), second.clone()])),
        Err(WorldRuntimeError::World(
            WorldError::DefinitionConflict { .. }
        ))
    ));
    let persisted =
        || SqliteWorldStorage::load_session_read_only(directory.path().join("world.sqlite"));
    assert!(persisted()?.properties().get(&key).is_none());

    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&first)))?;
    assert!(matches!(
        runtime.register_property_definitions(&property_bindings(&[second])),
        Err(WorldRuntimeError::World(
            WorldError::DefinitionConflict { .. }
        ))
    ));
    assert!(persisted()?.properties().get(&key).is_some());
    runtime.register_property_definitions(&property_bindings(&[first]))?;
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_not_inherit_property_authority_from_sqlite_after_restart() -> Result<()> {
    let (directory, storage, schemas) = create_storage()?;
    let item = ItemId::new();
    let proposed = vec![
        StateChange::CreateObject { id: item },
        StateChange::SetProperty {
            item,
            key: schemas.identity.clone(),
            data: serde_json::json!({ "name": "first" }),
        },
    ];
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(schemas.command.clone(), proposed))?;
    let runtime = builder.start()?;

    // Existing JSON Schema from SQLite does not grant execution rights.
    expect_denied(&runtime, &schemas.command, &schemas.identity, "set")?;
    assert!(runtime.snapshot()?.load_item(item)?.is_none());

    runtime.register_property_definitions(&property_bindings(&fixture_definitions(&schemas)))?;
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;
    runtime.shutdown()?;

    let next = ItemId::new();
    let reopened = SqliteWorldStorage::open(directory.path().join("world.sqlite"))?;
    let mut builder = runtime_builder(reopened, &[&schemas.command])?;
    builder.register_system(ScriptedSystem::new(
        schemas.command.clone(),
        vec![
            StateChange::CreateObject { id: next },
            StateChange::SetProperty {
                item: next,
                key: schemas.identity.clone(),
                data: serde_json::json!({ "name": "reopened" }),
            },
        ],
    ))?;
    let runtime = builder.start()?;
    expect_denied(&runtime, &schemas.command, &schemas.identity, "set")?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;
    assert!(runtime.snapshot()?.load_item(next)?.is_some());
    runtime.shutdown()?;
    Ok(())
}

/// A native command System using the same owner-scoped read capability as the
/// service-backed System. This verifies that introducing indexed/batched reads
/// does not silently revert property authorization in the core runtime.
struct IndexedReadSystem {
    schema: SchemaKey,
    allowed_property: SchemaKey,
    forbidden_property: SchemaKey,
    root: ItemId,
    expected_relation: ItemId,
    observed: Arc<Mutex<Vec<ItemId>>>,
    creates: ItemId,
}

impl WorldSystem for IndexedReadSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        _command: &rintawa_world::WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let page = snapshot.find_properties(&self.allowed_property, None, 20)?;
        let ids = page.iter().map(|record| record.item()).collect::<Vec<_>>();
        *self
            .observed
            .lock()
            .map_err(|_| SystemError::Failed(String::from("search observer lock poisoned")))? = ids;

        assert!(matches!(
            snapshot.find_properties(&self.forbidden_property, None, 20),
            Err(WorldReadError::PropertyAccessDenied(key)) if key == self.forbidden_property
        ));
        let batch = snapshot.read(&[self.root], std::slice::from_ref(&self.allowed_property))?;
        assert_eq!(batch.items().len(), 1);
        assert_eq!(batch.properties().len(), 1);
        assert!(matches!(
            snapshot.read(&[self.root], std::slice::from_ref(&self.forbidden_property)),
            Err(WorldReadError::PropertyAccessDenied(key)) if key == self.forbidden_property
        ));
        // Structural relations are accessible without exposing their properties.
        let relations = snapshot.relations_touching_any(&[self.root], None, 10)?;
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].id(), self.expected_relation);
        let filtered = snapshot.find_relations(
            &[self.root],
            RelationDirection::Outgoing,
            Some(&self.allowed_property),
            None,
            10,
        )?;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id(), self.expected_relation);
        assert!(matches!(
            snapshot.find_relations(
                &[self.root],
                RelationDirection::Outgoing,
                Some(&self.forbidden_property),
                None,
                10,
            ),
            Err(WorldReadError::PropertyAccessDenied(key)) if key == self.forbidden_property
        ));

        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: self.creates });
        Ok(transaction)
    }
}

#[test]
fn test_should_keep_indexed_search_scoped_to_command_system_owner() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let foreign = foreign_definition();
    storage.register_definition(&foreign)?;
    let root = ItemId::new();
    let next = ItemId::new();
    let link = ItemId::new();
    let created = ItemId::new();

    let mut initial = WorldTransaction::new();
    initial.push_change(StateChange::CreateObject { id: root });
    initial.push_change(StateChange::CreateObject { id: next });
    initial.push_change(StateChange::CreateRelation {
        id: link,
        subject: root,
        object: next,
    });
    initial.push_change(StateChange::SetProperty {
        item: root,
        key: schemas.identity.clone(),
        data: serde_json::json!({ "name": "visible" }),
    });
    initial.push_change(StateChange::SetProperty {
        item: next,
        key: foreign.key().clone(),
        data: serde_json::json!({ "secret": "not readable" }),
    });
    initial.push_change(StateChange::SetProperty {
        item: link,
        key: schemas.identity.clone(),
        data: serde_json::json!({ "name": "visible relation" }),
    });
    storage.apply(&initial)?;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(IndexedReadSystem {
        schema: schemas.command.clone(),
        allowed_property: schemas.identity.clone(),
        forbidden_property: foreign.key().clone(),
        root,
        expected_relation: link,
        observed: Arc::clone(&observed),
        creates: created,
    })?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    runtime.register_property_definitions(&property_bindings(std::slice::from_ref(&foreign)))?;

    // Host has an unrestricted snapshot; extension Systems don't.
    assert_eq!(
        runtime
            .snapshot()?
            .find_properties(foreign.key(), None, 10)?
            .len(),
        1
    );
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;
    assert_eq!(
        *observed.lock().expect("search observer lock"),
        vec![root, link]
    );
    assert!(runtime.snapshot()?.load_item(created)?.is_some());
    runtime.shutdown()?;
    Ok(())
}

/// A System intentionally attempts a read before the installed package has
/// been rebound to the active world session. The schema is already in SQLite.
struct UnboundSearchSystem {
    schema: SchemaKey,
    property: SchemaKey,
    created: ItemId,
}

impl WorldSystem for UnboundSearchSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        _command: &rintawa_world::WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        snapshot.find_properties(&self.property, None, 20)?;
        let mut output = WorldTransaction::new();
        output.push_change(StateChange::CreateObject { id: self.created });
        Ok(output)
    }
}

#[test]
fn test_should_deny_indexed_reads_until_active_owner_is_bound() -> Result<()> {
    let (directory, storage, schemas) = create_storage()?;
    let created = ItemId::new();
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(UnboundSearchSystem {
        schema: schemas.command.clone(),
        property: schemas.identity.clone(),
        created,
    })?;
    let runtime = builder.start()?;
    let rejected = runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait();
    match rejected {
        Err(WorldRuntimeError::SystemFailed {
            source: SystemError::Read(WorldReadError::PropertyAccessDenied(key)),
            ..
        }) => assert_eq!(key, schemas.identity),
        other => panic!("unbound property lookup must fail closed: {other:?}"),
    }
    assert!(runtime.snapshot()?.load_item(created)?.is_none());

    activate_fixture_properties(&runtime, &schemas)?;
    runtime
        .submit(command(
            &schemas.command,
            PrincipalId::new(),
            serde_json::json!({}),
        ))?
        .wait()?;
    assert!(runtime.snapshot()?.load_item(created)?.is_some());
    runtime.shutdown()?;

    // A later activation starts without owner rights, despite SQLite having
    // the property definition and the prior successful command.
    let storage = SqliteWorldStorage::open(directory.path().join("world.sqlite"))?;
    let another = ItemId::new();
    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(UnboundSearchSystem {
        schema: schemas.command.clone(),
        property: schemas.identity.clone(),
        created: another,
    })?;
    let runtime = builder.start()?;
    assert!(matches!(
        runtime
            .submit(command(
                &schemas.command,
                PrincipalId::new(),
                serde_json::json!({})
            ))?
            .wait(),
        Err(WorldRuntimeError::SystemFailed {
            source: SystemError::Read(WorldReadError::PropertyAccessDenied(_)),
            ..
        })
    ));
    assert!(runtime.snapshot()?.load_item(another)?.is_none());
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_bind_every_version_of_a_property_name_to_one_exact_instance() -> Result<()> {
    let (directory, storage, schemas) = create_storage()?;
    let runtime = runtime_builder(storage, &[&schemas.command])?.start()?;
    let logical_owner = ExtensionId::new("shared.logical-package");
    let first = PropertyDefinition::new(
        key("shared.versioned@1"),
        logical_owner.clone(),
        serde_json::json!({}),
    );
    let second = PropertyDefinition::new(
        key("shared.versioned@2"),
        logical_owner,
        serde_json::json!({}),
    );
    let first_instance = ExtensionInstanceId::new("first-instance");
    let second_instance = ExtensionInstanceId::new("second-instance");
    // Equal package names and JSON schemas cannot turn different instances into one owner.
    assert!(matches!(
        runtime.register_property_definitions(&[
            (first_instance.clone(), first.clone()),
            (second_instance.clone(), second.clone()),
        ]),
        Err(WorldRuntimeError::World(
            WorldError::DefinitionConflict { .. }
        ))
    ));
    assert!(
        SqliteWorldStorage::load_session_read_only(directory.path().join("world.sqlite"))?
            .properties()
            .get(first.key())
            .is_none()
    );
    runtime.register_property_definitions(&[(first_instance.clone(), first.clone())])?;
    assert!(
        runtime
            .register_property_definitions(&[(second_instance, second.clone())])
            .is_err()
    );
    assert!(
        SqliteWorldStorage::load_session_read_only(directory.path().join("world.sqlite"))?
            .properties()
            .get(second.key())
            .is_none()
    );
    runtime.register_property_definitions(&[(first_instance, second)])?;
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_check_stored_version_before_replacement_and_rollback_prior_changes() -> Result<()> {
    for prefix in ["set", "remove", "delete"] {
        let (_directory, storage, schemas) = create_storage()?;
        let old = PropertyDefinition::new(
            key("shared.transition@1"),
            ExtensionId::new(common::EXTENSION),
            serde_json::json!({"type":"integer"}),
        );
        let new = PropertyDefinition::new(
            key("shared.transition@2"),
            ExtensionId::new(common::EXTENSION),
            serde_json::json!({"type":"integer"}),
        );
        storage.register_definition(&old)?;
        let item = ItemId::new();
        let created = ItemId::new();
        seed_object_property(&storage, item, old.key().clone(), serde_json::json!(1))?;
        let mut changes = vec![StateChange::CreateObject { id: created }];
        // A stale version-specific remove is a no-op; it cannot hide the old row.
        if prefix == "remove" {
            changes.push(StateChange::RemoveProperty {
                item,
                key: new.key().clone(),
            });
        }
        if prefix == "delete" {
            changes.push(StateChange::DeleteItem { id: item });
        }
        changes.push(StateChange::SetProperty {
            item,
            key: new.key().clone(),
            data: serde_json::json!(2),
        });
        let mut builder = runtime_builder(storage, &[&schemas.command])?;
        builder.register_system(ScriptedSystem::new(schemas.command.clone(), changes))?;
        let runtime = builder.start()?;
        // The vocabulary remembers v1 but only v2 is currently authorized.
        runtime.register_property_definitions(&property_bindings(&[new]))?;
        expect_denied(
            &runtime,
            &schemas.command,
            old.key(),
            if prefix == "delete" { "delete" } else { "set" },
        )?;
        assert!(runtime.snapshot()?.load_item(created)?.is_none());
        assert_eq!(
            runtime
                .snapshot()?
                .load_property(item, old.key())?
                .expect("old value preserved")
                .data(),
            &serde_json::json!(1)
        );
        runtime.shutdown()?;
    }
    Ok(())
}

#[test]
fn test_should_apply_authorized_version_changes_and_ordered_cascade_effects() -> Result<()> {
    let (_directory, storage, _schemas) = create_storage()?;
    let old = PropertyDefinition::new(
        key("shared.transition@1"),
        ExtensionId::new(common::EXTENSION),
        serde_json::json!({"type":"integer"}),
    );
    let new = PropertyDefinition::new(
        key("shared.transition@2"),
        ExtensionId::new(common::EXTENSION),
        serde_json::json!({"type":"integer"}),
    );
    storage.register_definition(&old)?;
    let subject = ItemId::new();
    let object = ItemId::new();
    let relation = ItemId::new();
    seed_object_property(&storage, subject, old.key().clone(), serde_json::json!(1))?;
    let transition = key("fixture.transition@1");
    let remove_and_delete = key("fixture.remove-delete@1");
    let mut builder = runtime_builder(storage, &[&transition, &remove_and_delete])?;
    builder.register_system(ScriptedSystem::new(
        transition.clone(),
        vec![
            StateChange::SetProperty {
                item: subject,
                key: new.key().clone(),
                data: serde_json::json!(2),
            },
            // Removing v1 after setting v2 must preserve v2.
            StateChange::RemoveProperty {
                item: subject,
                key: old.key().clone(),
            },
        ],
    ))?;
    builder.register_system(ScriptedSystem::new(
        remove_and_delete.clone(),
        vec![
            StateChange::CreateObject { id: object },
            StateChange::CreateRelation {
                id: relation,
                subject,
                object,
            },
            StateChange::SetProperty {
                item: relation,
                key: new.key().clone(),
                data: serde_json::json!(3),
            },
            StateChange::RemoveProperty {
                item: subject,
                key: new.key().clone(),
            },
            StateChange::DeleteItem { id: subject },
        ],
    ))?;
    let runtime = builder.start()?;
    runtime.register_property_definitions(&property_bindings(&[old, new.clone()]))?;
    runtime
        .submit(WorldCommand::system(transition, serde_json::json!({})))?
        .wait()?;
    assert_eq!(
        runtime
            .snapshot()?
            .load_property(subject, new.key())?
            .expect("transition committed")
            .data(),
        &serde_json::json!(2)
    );
    runtime
        .submit(WorldCommand::system(
            remove_and_delete,
            serde_json::json!({}),
        ))?
        .wait()?;
    assert!(runtime.snapshot()?.load_item(subject)?.is_none());
    assert!(runtime.snapshot()?.load_item(relation)?.is_none());
    assert!(runtime.snapshot()?.load_item(object)?.is_some());
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_deny_same_package_command_from_another_exact_instance() -> Result<()> {
    let (_directory, storage, schemas) = create_storage()?;
    let item = ItemId::new();
    let command_schema = key("fixture.sibling-command@1");
    let mut builder = rintawa_world_runtime::WorldRuntimeBuilder::new(storage);
    builder.register_runtime_schema(
        ExtensionInstanceId::new("sibling-instance"),
        RuntimeSchemaDefinition::new(
            command_schema.clone(),
            SchemaKind::Command,
            ExtensionId::new(common::EXTENSION),
            serde_json::json!({"type":"object"}),
        ),
    )?;
    builder.register_system(ScriptedSystem::new(
        command_schema.clone(),
        vec![
            StateChange::CreateObject { id: item },
            StateChange::SetProperty {
                item,
                key: schemas.identity.clone(),
                data: serde_json::json!({"name":"spoofed"}),
            },
        ],
    ))?;
    let runtime = builder.start()?;
    activate_fixture_properties(&runtime, &schemas)?;
    expect_denied(&runtime, &command_schema, &schemas.identity, "set")?;
    assert!(runtime.snapshot()?.load_item(item)?.is_none());
    runtime.shutdown()?;
    Ok(())
}
