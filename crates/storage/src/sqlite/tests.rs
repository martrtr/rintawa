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
    assert_eq!(version, 6);
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

/// Builds a legacy v5 (event-sourced) database with one entity, one relation,
/// one entity facet and one world facet.
fn legacy_v5(path: &std::path::Path) -> anyhow::Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE world_metadata (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            world_id BLOB NOT NULL,
            world_format_version INTEGER NOT NULL,
            commit_position INTEGER NOT NULL
         );
         CREATE TABLE world_schemas (
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            kind INTEGER NOT NULL,
            owner_extension_id TEXT NOT NULL,
            definition_json TEXT NOT NULL,
            PRIMARY KEY (schema_id, schema_version)
         );
         CREATE TABLE world_commits (
            commit_position INTEGER PRIMARY KEY,
            command_id BLOB NOT NULL UNIQUE,
            command_schema_id TEXT NOT NULL,
            command_schema_version INTEGER NOT NULL,
            principal_id BLOB NOT NULL,
            actor_kind INTEGER NOT NULL,
            actor_id BLOB NOT NULL,
            causation_kind INTEGER,
            causation_id BLOB,
            correlation_id BLOB NOT NULL,
            expected_position INTEGER,
            effective_at_ms INTEGER,
            recorded_at_ms INTEGER NOT NULL,
            command_payload_json TEXT NOT NULL,
            command_digest BLOB NOT NULL,
            admission_kind INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE entities (
            entity_id BLOB PRIMARY KEY,
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            created_position INTEGER NOT NULL
         );
         CREATE TABLE relations (
            relation_id BLOB PRIMARY KEY,
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            from_entity_id BLOB NOT NULL,
            to_entity_id BLOB NOT NULL,
            created_position INTEGER NOT NULL
         );
         CREATE TABLE entity_facets (
            entity_id BLOB NOT NULL,
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (entity_id, schema_id, schema_version)
         );
         CREATE TABLE relation_facets (
            relation_id BLOB NOT NULL,
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (relation_id, schema_id, schema_version)
         );
         CREATE TABLE world_facets (
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (schema_id, schema_version)
         );
         PRAGMA user_version = 5;",
    )?;

    let world_id = WorldId::new();
    let alice = [7_u8; 16];
    let forest = [8_u8; 16];
    let link = [9_u8; 16];

    connection.execute(
        "INSERT INTO world_metadata VALUES (1, ?1, 1, 3)",
        [world_id.into_bytes().as_slice()],
    )?;
    connection.execute(
        "INSERT INTO world_commits (
            commit_position, command_id, command_schema_id, command_schema_version,
            principal_id, actor_kind, actor_id, correlation_id, recorded_at_ms,
            command_payload_json, command_digest
         ) VALUES (1, ?1, 'example.create', 1, ?2, 1, ?2, ?2, 0, '{}', ?3)",
        rusqlite::params![[1_u8; 16], [2_u8; 16], [3_u8; 32]],
    )?;
    for (name, kind, payload) in [
        ("example.entity", 1_i64, "{}"),
        ("example.relation", 2, "{}"),
        ("example.identity", 3, "{\"type\":\"object\"}"),
        ("example.timeline", 3, "{\"type\":\"object\"}"),
    ] {
        connection.execute(
            "INSERT INTO world_schemas VALUES (?1, 1, ?2, 'example.extension', ?3)",
            rusqlite::params![name, kind, payload],
        )?;
    }
    for (id, name) in [(alice, "example.entity"), (forest, "example.entity")] {
        connection.execute(
            "INSERT INTO entities VALUES (?1, ?2, 1, 1)",
            rusqlite::params![id.as_slice(), name],
        )?;
    }
    connection.execute(
        "INSERT INTO relations VALUES (?1, 'example.relation', 1, ?2, ?3, 1)",
        rusqlite::params![link.as_slice(), alice.as_slice(), forest.as_slice()],
    )?;
    connection.execute(
        "INSERT INTO entity_facets VALUES (?1, 'example.identity', 1, '{\"name\":\"Alice\"}', 1)",
        [alice.as_slice()],
    )?;
    connection.execute(
        "INSERT INTO relation_facets VALUES (?1, 'example.identity', 1, '{\"name\":\"located\"}', 1)",
        [link.as_slice()],
    )?;
    connection.execute(
        "INSERT INTO world_facets VALUES ('example.timeline', 1, '{\"turn\":4}', 1)",
        [],
    )?;
    Ok(())
}

#[test]
fn test_should_migrate_legacy_v5_world_into_four_tables() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.sqlite");
    legacy_v5(&path)?;

    let storage = SqliteWorldStorage::open(&path)?;
    let connection = Connection::open(&path)?;
    assert_eq!(
        table_names(&connection),
        vec![
            "items".to_string(),
            "properties".to_string(),
            "property_registry".to_string(),
            "world".to_string()
        ]
    );

    let alice = ItemId::from_bytes([7_u8; 16]);
    let link = ItemId::from_bytes([9_u8; 16]);
    let record = storage.load_item(alice)?.expect("migrated entity");
    assert_eq!(record.kind(), ItemKind::Object);
    let relation = storage.load_item(link)?.expect("migrated relation");
    assert_eq!(relation.subject(), Some(alice));
    assert_eq!(
        relation.object_target(),
        Some(ItemId::from_bytes([8_u8; 16]))
    );

    let identity = storage
        .load_property(alice, &schema("example.identity@1"))?
        .expect("migrated entity facet");
    assert_eq!(identity.data()["name"], "Alice");
    let relation_facet = storage
        .load_property(link, &schema("example.identity@1"))?
        .expect("migrated relation facet");
    assert_eq!(relation_facet.data()["name"], "located");

    let session = storage.load_session()?;
    assert_eq!(
        session.manifest().as_value()["world_facets"]["example.timeline@1"]["turn"],
        4
    );
    assert!(
        directory
            .path()
            .join("legacy.sqlite.pre-four-table.bak")
            .exists()
    );
    Ok(())
}

#[test]
fn test_should_refuse_legacy_migration_with_pending_effect_jobs() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.sqlite");
    legacy_v5(&path)?;
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "CREATE TABLE effect_jobs (job_id BLOB PRIMARY KEY, status INTEGER NOT NULL);
         INSERT INTO effect_jobs VALUES (x'01', 0);",
    )?;
    drop(connection);

    let error = SqliteWorldStorage::open(&path).unwrap_err();
    assert!(matches!(
        error,
        crate::StorageError::MigrationBlocked { .. }
    ));
    Ok(())
}
