//! Integration tests for the four-table world storage.

use rintawa_sdk::{
    types::ExtensionId,
    world::{ItemId, SchemaKey, WorldId},
};
use rintawa_world::{
    DefinitionRegistration, ItemKind, PropertyDefinition, StateChange, WorldTransaction,
};
use rusqlite::Connection;
use tempfile::TempDir;

use crate::SqliteWorldStorage;

fn schema(key: &str) -> SchemaKey {
    key.parse().expect("test property name must be valid")
}

fn definition(key: &str, schema_json: serde_json::Value) -> PropertyDefinition {
    PropertyDefinition::new(
        schema(key),
        ExtensionId::new("example.extension"),
        schema_json,
    )
}

fn object_schema() -> PropertyDefinition {
    definition(
        "character.identity@1",
        serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"],
            "additionalProperties": false
        }),
    )
}

fn temp_world() -> (TempDir, SqliteWorldStorage) {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("world.sqlite");
    let storage = SqliteWorldStorage::create(&path, WorldId::new()).expect("create world");
    (directory, storage)
}

fn table_names(connection: &Connection) -> Vec<String> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("prepare");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect")
}

#[test]
fn test_should_create_exactly_four_domain_tables() -> anyhow::Result<()> {
    let (directory, storage) = temp_world();
    let connection = Connection::open(directory.path().join("world.sqlite"))?;

    assert_eq!(
        table_names(&connection),
        vec![
            "items".to_string(),
            "properties".to_string(),
            "property_registry".to_string(),
            "world".to_string()
        ]
    );
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, 1);
    assert_eq!(storage.world_id(), storage.load_session()?.id());
    Ok(())
}

#[test]
fn test_should_round_trip_items_and_properties_after_reopen() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("world.sqlite");
    let world_id = WorldId::new();
    let alice = ItemId::new();
    let forest = ItemId::new();
    let link = ItemId::new();

    {
        let storage = SqliteWorldStorage::create(&path, world_id)?;
        storage.register_definitions(&[object_schema()])?;
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: alice });
        transaction.push_change(StateChange::CreateObject { id: forest });
        transaction.push_change(StateChange::CreateRelation {
            id: link,
            subject: alice,
            object: forest,
        });
        transaction.push_change(StateChange::SetProperty {
            item: alice,
            key: schema("character.identity@1"),
            data: serde_json::json!({ "name": "Alice" }),
        });
        storage.apply(&transaction)?;
    }

    let storage = SqliteWorldStorage::open(&path)?;
    assert_eq!(storage.world_id(), world_id);
    let alice_record = storage.load_item(alice)?.expect("alice exists");
    assert_eq!(alice_record.kind(), ItemKind::Object);
    let link_record = storage.load_item(link)?.expect("link exists");
    assert_eq!(link_record.kind(), ItemKind::Relation);
    assert_eq!(link_record.subject(), Some(alice));
    assert_eq!(link_record.object_target(), Some(forest));

    let property = storage
        .load_property(alice, &schema("character.identity@1"))?
        .expect("property exists");
    assert_eq!(property.data()["name"], "Alice");
    assert_eq!(storage.properties_of(alice)?.len(), 1);
    Ok(())
}

#[test]
fn test_should_reject_relation_to_relation_and_missing_item() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let first = ItemId::new();
    let second = ItemId::new();
    let link = ItemId::new();

    let mut create = WorldTransaction::new();
    create.push_change(StateChange::CreateObject { id: first });
    create.push_change(StateChange::CreateObject { id: second });
    storage.apply(&create)?;

    let mut dangling = WorldTransaction::new();
    dangling.push_change(StateChange::CreateRelation {
        id: ItemId::new(),
        subject: first,
        object: ItemId::new(),
    });
    assert!(storage.apply(&dangling).is_err());

    let mut relation = WorldTransaction::new();
    relation.push_change(StateChange::CreateRelation {
        id: link,
        subject: first,
        object: second,
    });
    storage.apply(&relation)?;

    let mut chained = WorldTransaction::new();
    chained.push_change(StateChange::CreateRelation {
        id: ItemId::new(),
        subject: first,
        object: link,
    });
    assert!(storage.apply(&chained).is_err());
    Ok(())
}

#[test]
fn test_should_roll_back_all_changes_when_one_fails() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let created = ItemId::new();

    let mut transaction = WorldTransaction::new();
    transaction.push_change(StateChange::CreateObject { id: created });
    transaction.push_change(StateChange::CreateRelation {
        id: ItemId::new(),
        subject: created,
        object: ItemId::new(),
    });
    assert!(storage.apply(&transaction).is_err());
    assert!(storage.load_item(created)?.is_none());
    Ok(())
}

#[test]
fn test_should_cascade_properties_and_relations_on_item_delete() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    storage.register_definitions(&[object_schema()])?;
    let alice = ItemId::new();
    let forest = ItemId::new();
    let link = ItemId::new();

    let mut create = WorldTransaction::new();
    create.push_change(StateChange::CreateObject { id: alice });
    create.push_change(StateChange::CreateObject { id: forest });
    create.push_change(StateChange::CreateRelation {
        id: link,
        subject: alice,
        object: forest,
    });
    create.push_change(StateChange::SetProperty {
        item: link,
        key: schema("character.identity@1"),
        data: serde_json::json!({ "name": "located" }),
    });
    storage.apply(&create)?;

    let mut delete = WorldTransaction::new();
    delete.push_change(StateChange::DeleteItem { id: alice });
    storage.apply(&delete)?;

    assert!(storage.load_item(link)?.is_none());
    assert!(storage.properties_of(link)?.is_empty());
    Ok(())
}

#[test]
fn test_should_keep_one_current_version_per_property_name() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let alice = ItemId::new();
    storage.register_definitions(&[
        object_schema(),
        definition(
            "character.identity@2",
            serde_json::json!({ "type": "object" }),
        ),
    ])?;

    let mut transaction = WorldTransaction::new();
    transaction.push_change(StateChange::CreateObject { id: alice });
    transaction.push_change(StateChange::SetProperty {
        item: alice,
        key: schema("character.identity@1"),
        data: serde_json::json!({ "name": "Alice" }),
    });
    storage.apply(&transaction)?;

    let mut upgrade = WorldTransaction::new();
    upgrade.push_change(StateChange::SetProperty {
        item: alice,
        key: schema("character.identity@2"),
        data: serde_json::json!({ "name": "Alice" }),
    });
    storage.apply(&upgrade)?;

    let properties = storage.properties_of(alice)?;
    assert_eq!(properties.len(), 1);
    assert_eq!(properties[0].key().version().get(), 2);
    Ok(())
}

#[test]
fn test_should_keep_newer_property_when_removing_old_version() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let item = ItemId::new();
    storage.register_definitions(&[
        object_schema(),
        definition(
            "character.identity@2",
            serde_json::json!({"type": "object"}),
        ),
    ])?;
    let mut create = WorldTransaction::new();
    create.push_change(StateChange::CreateObject { id: item });
    create.push_change(StateChange::SetProperty {
        item,
        key: schema("character.identity@2"),
        data: serde_json::json!({"name": "Current"}),
    });
    storage.apply(&create)?;

    let mut stale_remove = WorldTransaction::new();
    stale_remove.push_change(StateChange::RemoveProperty {
        item,
        key: schema("character.identity@1"),
    });
    storage.apply(&stale_remove)?;
    assert!(
        storage
            .load_property(item, &schema("character.identity@2"))?
            .is_some()
    );

    let mut current_remove = WorldTransaction::new();
    current_remove.push_change(StateChange::RemoveProperty {
        item,
        key: schema("character.identity@2"),
    });
    storage.apply(&current_remove)?;
    assert!(storage.properties_of(item)?.is_empty());
    Ok(())
}

#[test]
fn test_should_register_definitions_idempotently_and_reject_conflicts() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let identity = object_schema();

    assert_eq!(
        storage.register_definition(&identity)?,
        DefinitionRegistration::Registered
    );
    assert_eq!(
        storage.register_definition(&identity)?,
        DefinitionRegistration::AlreadyPresent
    );

    let conflicting = definition(
        "character.identity@1",
        serde_json::json!({ "type": "string" }),
    );
    assert!(storage.register_definition(&conflicting).is_err());
    Ok(())
}

#[test]
fn test_should_reject_property_values_through_the_vocabulary() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    storage.register_definitions(&[object_schema()])?;
    let alice = ItemId::new();
    let registry = storage.property_registry()?;

    let mut transaction = WorldTransaction::new();
    transaction.push_change(StateChange::CreateObject { id: alice });
    transaction.push_change(StateChange::SetProperty {
        item: alice,
        key: schema("character.identity@1"),
        data: serde_json::json!({ "name": 7 }),
    });
    assert!(transaction.validate(&registry).is_err());

    let mut unknown = WorldTransaction::new();
    unknown.push_change(StateChange::SetProperty {
        item: alice,
        key: schema("absent.property@1"),
        data: serde_json::json!({}),
    });
    assert!(unknown.validate(&registry).is_err());
    Ok(())
}

#[test]
fn test_should_reject_second_world_row() -> anyhow::Result<()> {
    let (directory, storage) = temp_world();
    drop(storage);
    let connection = Connection::open(directory.path().join("world.sqlite"))?;
    let result = connection.execute(
        "INSERT INTO world (id, manifest) VALUES (?1, '{}')",
        [ItemId::new().as_uuid().as_bytes().to_vec()],
    );
    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_should_pin_snapshot_reads() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let item = ItemId::new();
    let mut create = WorldTransaction::new();
    create.push_change(StateChange::CreateObject { id: item });
    storage.apply(&create)?;

    let snapshot = storage.snapshot()?;
    assert!(snapshot.load_item(item)?.is_some());

    let mut delete = WorldTransaction::new();
    delete.push_change(StateChange::DeleteItem { id: item });
    storage.apply(&delete)?;

    assert!(snapshot.load_item(item)?.is_some());
    assert!(storage.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_load_manifest_and_registry_from_one_commit() -> anyhow::Result<()> {
    use std::sync::{Arc, Barrier};

    let (_directory, storage) = temp_world();
    let path = storage.path().to_owned();
    let barrier = Arc::new(Barrier::new(2));
    let writer_barrier = barrier.clone();
    let writer = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut connection = Connection::open(path)?;
        writer_barrier.wait();
        for generation in 1..=64_u64 {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO property_registry (name, version, schema) VALUES (?1, 1, '{}')",
                [format!("snapshot.commit-{generation}")],
            )?;
            transaction.execute(
                "UPDATE world SET manifest = ?1",
                [serde_json::json!({"generation": generation}).to_string()],
            )?;
            transaction.commit()?;
            std::thread::yield_now();
        }
        Ok(())
    });

    barrier.wait();
    for _ in 0..256 {
        let state = storage.load_session()?;
        let generation = state.manifest().as_value()["generation"]
            .as_u64()
            .unwrap_or(0);
        if generation > 0 {
            assert!(
                state
                    .properties()
                    .get(&schema(&format!("snapshot.commit-{generation}@1")))
                    .is_some()
            );
        }
        assert!(
            state
                .properties()
                .get(&schema(&format!("snapshot.commit-{}@1", generation + 1)))
                .is_none()
        );
        std::thread::yield_now();
    }
    writer.join().expect("test writer thread must not panic")?;
    assert_eq!(
        storage.load_session()?.manifest().as_value()["generation"],
        64
    );
    Ok(())
}

#[test]
fn test_should_read_session_without_opening_a_writer() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("world.sqlite");
    let world_id = WorldId::new();
    let storage = SqliteWorldStorage::create(&path, world_id)?;
    let read_only = SqliteWorldStorage::load_session_read_only(&path)?;
    assert_eq!(read_only.id(), world_id);
    assert!(read_only.properties().is_empty());
    drop(storage);

    // A read-only connection must also work after the writable one is closed.
    assert_eq!(
        SqliteWorldStorage::load_session_read_only(&path)?.id(),
        world_id,
    );
    Ok(())
}

#[test]
fn test_should_initialize_custom_manifest_in_first_transaction() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("world.sqlite");
    let manifest = rintawa_world::WorldManifest::new(
        serde_json::json!({"metadata": {"title": "Atomic World"}}),
    );
    let world = SqliteWorldStorage::create_with_manifest(&path, WorldId::new(), &manifest)?;
    assert_eq!(world.load_session()?.manifest(), &manifest);
    drop(world);
    assert_eq!(
        SqliteWorldStorage::open(&path)?.load_session()?.manifest(),
        &manifest
    );
    Ok(())
}

#[test]
fn test_should_reject_missing_property_search_index_without_writing() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("world.sqlite");
    drop(SqliteWorldStorage::create(&path, WorldId::new())?);
    Connection::open(&path)?.execute_batch("DROP INDEX properties_by_name_version_item")?;
    let before = std::fs::read(&path)?;
    assert!(matches!(
        SqliteWorldStorage::open(&path),
        Err(crate::StorageError::CorruptData(_))
    ));
    assert_eq!(std::fs::read(&path)?, before);
    Ok(())
}

#[test]
fn test_should_refuse_legacy_world_without_modifying_file() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.sqlite");
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TABLE world_metadata (world_id BLOB NOT NULL);
         INSERT INTO world_metadata VALUES (x'010203');
         PRAGMA user_version = 5;",
    )?;
    drop(connection);
    let before = std::fs::read(&path)?;

    let error = SqliteWorldStorage::open(&path).unwrap_err();
    assert!(matches!(
        error,
        crate::StorageError::UnsupportedStorageSchema {
            found: 5,
            supported: 1
        }
    ));
    assert_eq!(std::fs::read(&path)?, before);
    assert!(
        !directory
            .path()
            .join("legacy.sqlite.pre-four-table.bak")
            .exists()
    );
    Ok(())
}

#[test]
fn test_should_refuse_previous_four_table_version_without_modifying_file() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("world.sqlite");
    {
        let _storage = SqliteWorldStorage::create(&path, WorldId::new())?;
    }
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "user_version", 6_u32)?;
    drop(connection);
    let before = std::fs::read(&path)?;

    let error = SqliteWorldStorage::open(&path).unwrap_err();
    assert!(matches!(
        error,
        crate::StorageError::UnsupportedStorageSchema {
            found: 6,
            supported: 1
        }
    ));
    assert_eq!(std::fs::read(&path)?, before);
    Ok(())
}

#[test]
fn test_should_refuse_unknown_tables_with_current_version() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("unknown.sqlite");
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TABLE unknown_table (id INTEGER PRIMARY KEY);
         PRAGMA user_version = 1;",
    )?;
    drop(connection);
    let before = std::fs::read(&path)?;

    let error = SqliteWorldStorage::open(&path).unwrap_err();
    assert!(matches!(error, crate::StorageError::CorruptData(_)));
    assert_eq!(std::fs::read(&path)?, before);
    Ok(())
}

#[test]
fn test_should_refuse_missing_integrity_trigger() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("world.sqlite");
    {
        let _storage = SqliteWorldStorage::create(&path, WorldId::new())?;
    }
    let connection = Connection::open(&path)?;
    connection.execute_batch("DROP TRIGGER items_relation_endpoints_ins")?;
    drop(connection);
    let error = SqliteWorldStorage::open(&path).unwrap_err();
    assert!(matches!(error, crate::StorageError::CorruptData(_)));
    Ok(())
}

#[test]
fn test_should_serialize_concurrent_property_definition_registrations() -> anyhow::Result<()> {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use crate::StorageError;
    use rintawa_world::WorldError;

    let (_directory, storage) = temp_world();
    let storage = Arc::new(storage);
    let start = Arc::new(Barrier::new(12));
    let property = schema("shared.concurrent@1");

    let workers = (0..12)
        .map(|index| {
            let storage = Arc::clone(&storage);
            let start = Arc::clone(&start);
            let property = property.clone();
            thread::spawn(move || {
                let definition_body = serde_json::json!({
                    "type": if index % 2 == 0 { "string" } else { "integer" }
                });
                let definition = PropertyDefinition::new(
                    property,
                    ExtensionId::new(format!("example.{index}")),
                    definition_body.clone(),
                );
                start.wait();
                (definition_body, storage.register_definition(&definition))
            })
        })
        .collect::<Vec<_>>();

    let mut outcomes = Vec::new();
    for worker in workers {
        outcomes.push(worker.join().expect("registration worker must not panic"));
    }
    let persisted = storage.property_registry()?;
    let winner = persisted
        .get(&property)
        .expect("winning definition must be stored");
    let winner_body = winner.schema();
    let mut winners = 0;

    for (body, outcome) in outcomes {
        if &body == winner_body {
            match outcome? {
                DefinitionRegistration::Registered => winners += 1,
                DefinitionRegistration::AlreadyPresent => {}
            }
        } else {
            assert!(matches!(
                outcome,
                Err(StorageError::World(WorldError::DefinitionConflict { .. }))
            ));
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(persisted.len(), 1);
    Ok(())
}

#[test]
fn test_should_rollback_entire_property_definition_batch_on_conflict() -> anyhow::Result<()> {
    use crate::StorageError;
    use rintawa_world::WorldError;

    let (_directory, storage) = temp_world();
    let first = definition("batch.first@1", serde_json::json!({ "type": "integer" }));
    let second = definition("batch.second@1", serde_json::json!({ "type": "boolean" }));
    let conflicting = definition("batch.first@1", serde_json::json!({ "type": "string" }));

    assert!(matches!(
        storage.register_definitions(&[first, second, conflicting]),
        Err(StorageError::World(WorldError::DefinitionConflict { .. }))
    ));
    assert!(storage.property_registry()?.is_empty());
    Ok(())
}

#[test]
fn test_should_find_properties_by_exact_version_in_bounded_keyset_pages() -> anyhow::Result<()> {
    let (directory, storage) = temp_world();
    let first_key = schema("search.character@1");
    let second_key = schema("search.character@2");
    storage.register_definitions(&[
        definition(
            "search.character@1",
            serde_json::json!({ "type": "object" }),
        ),
        definition(
            "search.character@2",
            serde_json::json!({ "type": "object" }),
        ),
    ])?;

    let mut transaction = WorldTransaction::new();
    let mut first_ids = Vec::new();
    let mut second_ids = Vec::new();
    for index in 0..317 {
        let item = ItemId::new();
        first_ids.push(item);
        transaction.push_change(StateChange::CreateObject { id: item });
        transaction.push_change(StateChange::SetProperty {
            item,
            key: first_key.clone(),
            data: serde_json::json!({ "index": index }),
        });
    }
    for index in 0..11 {
        let item = ItemId::new();
        second_ids.push(item);
        transaction.push_change(StateChange::CreateObject { id: item });
        transaction.push_change(StateChange::SetProperty {
            item,
            key: second_key.clone(),
            data: serde_json::json!({ "index": index }),
        });
    }
    storage.apply(&transaction)?;
    first_ids.sort();
    second_ids.sort();

    let mut after = None;
    let mut observed = Vec::new();
    loop {
        let page = storage.find_properties(&first_key, after, 64)?;
        assert!(page.len() <= 64);
        if page.is_empty() {
            break;
        }
        for property in &page {
            assert_eq!(property.key(), &first_key);
            assert!(property.data()["index"].is_number());
            observed.push(property.item());
        }
        after = page.last().map(|property| property.item());
        if page.len() < 64 {
            break;
        }
    }
    assert_eq!(observed, first_ids);
    assert!(storage.find_properties(&first_key, after, 64)?.is_empty());

    let other = storage.find_properties(&second_key, None, 64)?;
    assert_eq!(
        other
            .iter()
            .map(|property| property.item())
            .collect::<Vec<_>>(),
        second_ids
    );
    assert!(
        storage
            .find_properties(&schema("missing.property@1"), None, 20)?
            .is_empty()
    );

    let connection = Connection::open(directory.path().join("world.sqlite"))?;
    let plan: String = connection.query_row(
        "EXPLAIN QUERY PLAN
         SELECT item, data FROM properties
         WHERE name = ?1 AND version = ?2 AND item > COALESCE(?3, X'')
         ORDER BY item LIMIT ?4",
        rusqlite::params!["search.character", 1_i64, Option::<Vec<u8>>::None, 64_i64],
        |row| row.get(3),
    )?;
    assert!(plan.contains("properties_by_name_version_item"), "{plan}");
    Ok(())
}

#[test]
fn test_should_keep_search_snapshot_stable_during_later_commits() -> anyhow::Result<()> {
    let (directory, storage) = temp_world();
    let key = schema("search.snapshot@1");
    storage.register_definition(&definition(
        "search.snapshot@1",
        serde_json::json!({ "type": "object" }),
    ))?;
    let first = ItemId::new();
    let second = ItemId::new();

    let mut create = WorldTransaction::new();
    create.push_change(StateChange::CreateObject { id: first });
    create.push_change(StateChange::SetProperty {
        item: first,
        key: key.clone(),
        data: serde_json::json!({ "version": "before" }),
    });
    storage.apply(&create)?;

    let snapshot = storage.snapshot()?;
    let initial = snapshot.find_properties(&key, None, 20)?;
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].data()["version"], "before");

    let mut next = WorldTransaction::new();
    next.push_change(StateChange::CreateObject { id: second });
    next.push_change(StateChange::SetProperty {
        item: second,
        key: key.clone(),
        data: serde_json::json!({ "version": "after" }),
    });
    next.push_change(StateChange::SetProperty {
        item: first,
        key: key.clone(),
        data: serde_json::json!({ "version": "changed" }),
    });
    storage.apply(&next)?;

    let pinned = snapshot.find_properties(&key, None, 20)?;
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].data()["version"], "before");
    let current = storage.find_properties(&key, None, 20)?;
    assert_eq!(current.len(), 2);

    drop(snapshot);
    drop(storage);
    let reopened = SqliteWorldStorage::open(directory.path().join("world.sqlite"))?;
    assert_eq!(reopened.find_properties(&key, None, 20)?.len(), 2);
    Ok(())
}

#[test]
fn test_should_batch_graph_relations_once_with_exclusive_cursor() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let a = ItemId::new();
    let b = ItemId::new();
    let c = ItemId::new();
    let unrelated_a = ItemId::new();
    let unrelated_b = ItemId::new();

    let mut initial = WorldTransaction::new();
    for item in [a, b, c, unrelated_a, unrelated_b] {
        initial.push_change(StateChange::CreateObject { id: item });
    }
    let mut relevant = Vec::new();
    for (subject, object) in [(a, b), (b, a), (a, a), (b, c), (c, a)] {
        let id = ItemId::new();
        relevant.push(id);
        initial.push_change(StateChange::CreateRelation {
            id,
            subject,
            object,
        });
    }
    let irrelevant = ItemId::new();
    initial.push_change(StateChange::CreateRelation {
        id: irrelevant,
        subject: unrelated_a,
        object: unrelated_b,
    });
    storage.apply(&initial)?;
    relevant.sort();

    let mut cursor = None;
    let mut observed = Vec::new();
    loop {
        let page = storage.relations_touching_any(&[a, b, a], cursor, 2)?;
        assert!(page.len() <= 2);
        if page.is_empty() {
            break;
        }
        observed.extend(page.iter().map(|relation| relation.id()));
        cursor = page.last().map(|relation| relation.id());
    }
    assert_eq!(observed, relevant);
    assert!(!observed.contains(&irrelevant));
    assert_eq!(
        storage
            .snapshot()?
            .relations_touching_any(&[c], None, 20)?
            .len(),
        2
    );
    assert!(storage.relations_touching_any(&[], None, 20)?.is_empty());
    Ok(())
}

#[test]
fn test_should_bound_graph_and_property_read_parameters() -> anyhow::Result<()> {
    use crate::StorageError;

    let (_directory, storage) = temp_world();
    let key = schema("limits.property@1");
    for size in [0, 257, usize::MAX] {
        assert!(matches!(
            storage.find_properties(&key, None, size),
            Err(StorageError::InvalidQueryPageSize { requested, maximum: 256 })
                if requested == size
        ));
        assert!(matches!(
            storage.relations_touching_any(&[], None, size),
            Err(StorageError::InvalidQueryPageSize { requested, maximum: 256 })
                if requested == size
        ));
    }
    let too_many = (0..65).map(|_| ItemId::new()).collect::<Vec<_>>();
    assert!(matches!(
        storage.relations_touching_any(&too_many, None, 10),
        Err(StorageError::InvalidQueryBatchSize {
            requested: 65,
            maximum: 64
        })
    ));
    let exactly_max = (0..64).map(|_| ItemId::new()).collect::<Vec<_>>();
    assert!(
        storage
            .relations_touching_any(&exactly_max, None, 256)?
            .is_empty()
    );
    assert!(storage.find_properties(&key, None, 256)?.is_empty());
    Ok(())
}

#[test]
fn test_should_report_distinct_cascade_property_keys_from_one_query() -> anyhow::Result<()> {
    let (_directory, storage) = temp_world();
    let first = schema("cascade.first@1");
    let second = schema("cascade.second@1");
    storage.register_definitions(&[
        definition("cascade.first@1", serde_json::json!({ "type": "object" })),
        definition("cascade.second@1", serde_json::json!({ "type": "object" })),
    ])?;

    let a = ItemId::new();
    let b = ItemId::new();
    let c = ItemId::new();
    let r1 = ItemId::new();
    let r2 = ItemId::new();
    let unrelated = ItemId::new();
    let mut seed = WorldTransaction::new();
    for item in [a, b, c] {
        seed.push_change(StateChange::CreateObject { id: item });
    }
    seed.push_change(StateChange::CreateRelation {
        id: r1,
        subject: a,
        object: b,
    });
    seed.push_change(StateChange::CreateRelation {
        id: r2,
        subject: c,
        object: a,
    });
    seed.push_change(StateChange::CreateRelation {
        id: unrelated,
        subject: b,
        object: c,
    });
    for item in [a, r1, r2] {
        seed.push_change(StateChange::SetProperty {
            item,
            key: first.clone(),
            data: serde_json::json!({}),
        });
    }
    seed.push_change(StateChange::SetProperty {
        item: r2,
        key: second.clone(),
        data: serde_json::json!({}),
    });
    seed.push_change(StateChange::SetProperty {
        item: unrelated,
        key: second.clone(),
        data: serde_json::json!({ "unrelated": true }),
    });
    storage.apply(&seed)?;

    let snapshot = storage.snapshot()?;
    let keys = snapshot.property_keys_deleted_with_item(a)?;
    assert_eq!(keys, vec![first.clone(), second.clone()]);
    let unaffected = snapshot.property_keys_deleted_with_item(b)?;
    assert!(unaffected.contains(&first));
    assert!(unaffected.contains(&second));
    assert!(
        snapshot
            .property_keys_deleted_with_item(ItemId::new())?
            .is_empty()
    );
    Ok(())
}

#[test]
fn test_should_reject_oversized_indexed_property_page_without_truncation() -> anyhow::Result<()> {
    use crate::StorageError;

    let (_directory, storage) = temp_world();
    let key = schema("search.large@1");
    storage.register_definition(&definition(
        "search.large@1",
        serde_json::json!({ "type": "object" }),
    ))?;
    let mut seed = WorldTransaction::new();
    for index in 0..3 {
        let item = ItemId::new();
        seed.push_change(StateChange::CreateObject { id: item });
        seed.push_change(StateChange::SetProperty {
            item,
            key: key.clone(),
            data: serde_json::json!({ "content": "x".repeat(3 * 1024 * 1024), "index": index }),
        });
    }
    storage.apply(&seed)?;

    assert_eq!(storage.find_properties(&key, None, 2)?.len(), 2);
    assert!(matches!(
        storage.find_properties(&key, None, 3),
        Err(StorageError::QueryPageTooLarge { bytes, maximum })
            if bytes > maximum && maximum == 8 * 1024 * 1024
    ));
    let snapshot = storage.snapshot()?;
    assert!(matches!(
        snapshot.find_properties(&key, None, 256),
        Err(StorageError::QueryPageTooLarge { .. })
    ));
    // A smaller request is still possible: neither the database nor the
    // result cursor changes after an oversized page is rejected.
    let first = storage.find_properties(&key, None, 1)?;
    assert_eq!(first.len(), 1);
    let second = storage.find_properties(&key, Some(first[0].item()), 1)?;
    assert_eq!(second.len(), 1);
    Ok(())
}
