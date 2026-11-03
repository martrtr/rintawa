//! SQLite storage schema initialization and legacy migration.
//!
//! The target schema is the four-table item/property model. New databases are
//! created directly with those four tables; legacy databases (PRAGMA
//! `user_version` 1 through 5, the event-sourced model) are converted into the
//! same four tables and the legacy tables are dropped.
//!
//! Conversion keeps a byte-for-byte backup next to the original file, performs
//! every change inside one exclusive SQLite transaction (so SQLite itself is the
//! atomic replacement step), validates the result before committing, and fails
//! closed when it cannot represent legacy data faithfully.

use std::path::{Path, PathBuf};

use rintawa_sdk::world::{SchemaId, SchemaKey, SchemaVersion, WorldId};
use rintawa_world::WorldManifest;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{StorageError, StorageResult};

/// Storage schema version stored in SQLite `user_version` for the four-table model.
pub(super) const STORAGE_SCHEMA_VERSION: u32 = 6;

/// Unversioned property name recording the retired entity/relation type schema of
/// a migrated item. Its only registered version is 1.
///
/// Legacy type schemas described identity/attributes rather than one property
/// value, so they are preserved as data instead of being reinterpreted as a JSON
/// Schema that every legacy row would have to satisfy.
pub(super) const LEGACY_SCHEMA_PROPERTY: &str = "migration.legacy-schema";

pub(super) const FOUR_TABLE_SCHEMA: &str = "PRAGMA foreign_keys = ON;

CREATE TABLE world (
    id       BLOB NOT NULL CHECK (length(id) = 16),
    manifest TEXT NOT NULL CHECK (json_valid(manifest)),
    PRIMARY KEY (id)
) STRICT, WITHOUT ROWID;

CREATE TABLE items (
    id      BLOB NOT NULL CHECK (length(id) = 16),
    type    INTEGER NOT NULL CHECK (type IN (1, 2)),
    subject BLOB CHECK (subject IS NULL OR length(subject) = 16),
    object  BLOB CHECK (object  IS NULL OR length(object)  = 16),
    PRIMARY KEY (id),
    CHECK ((type = 1 AND subject IS NULL     AND object IS NULL)
        OR (type = 2 AND subject IS NOT NULL AND object IS NOT NULL)),
    FOREIGN KEY (subject) REFERENCES items(id) ON DELETE CASCADE,
    FOREIGN KEY (object)  REFERENCES items(id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE property_registry (
    name    TEXT    NOT NULL CHECK (length(name) > 0),
    version INTEGER NOT NULL CHECK (version > 0),
    schema  TEXT    NOT NULL CHECK (json_valid(schema)),
    PRIMARY KEY (name, version)
) STRICT, WITHOUT ROWID;

CREATE TABLE properties (
    item    BLOB    NOT NULL CHECK (length(item) = 16),
    name    TEXT    NOT NULL,
    version INTEGER NOT NULL,
    data    TEXT    NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (item, name),
    FOREIGN KEY (item) REFERENCES items(id) ON DELETE CASCADE,
    FOREIGN KEY (name, version)
        REFERENCES property_registry(name, version) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;

CREATE INDEX items_by_subject ON items(subject) WHERE subject IS NOT NULL;
CREATE INDEX items_by_object  ON items(object)  WHERE object  IS NOT NULL;

CREATE TRIGGER world_single_row BEFORE INSERT ON world
WHEN EXISTS (SELECT 1 FROM world)
BEGIN SELECT RAISE(ABORT, 'world already exists in this database'); END;

CREATE TRIGGER items_relation_endpoints_ins BEFORE INSERT ON items
WHEN NEW.type = 2
 AND ((SELECT type FROM items WHERE id = NEW.subject) IS NOT 1
   OR (SELECT type FROM items WHERE id = NEW.object)  IS NOT 1)
BEGIN SELECT RAISE(ABORT, 'relation endpoints must be object items'); END;

CREATE TRIGGER items_relation_endpoints_upd BEFORE UPDATE OF subject, object ON items
WHEN NEW.type = 2
 AND ((SELECT type FROM items WHERE id = NEW.subject) IS NOT 1
   OR (SELECT type FROM items WHERE id = NEW.object)  IS NOT 1)
BEGIN SELECT RAISE(ABORT, 'relation endpoints must be object items'); END;

CREATE TRIGGER items_type_immutable BEFORE UPDATE OF type ON items
WHEN OLD.type <> NEW.type
BEGIN SELECT RAISE(ABORT, 'items.type is immutable'); END;

CREATE TRIGGER property_registry_immutable BEFORE UPDATE ON property_registry
WHEN OLD.name <> NEW.name OR OLD.version <> NEW.version OR OLD.schema <> NEW.schema
BEGIN SELECT RAISE(ABORT, 'property definition is immutable'); END;
";

pub(super) fn initialize_new(connection: &mut Connection, world_id: WorldId) -> StorageResult<()> {
    let found = storage_version(connection)?;
    if found != 0 {
        return Err(StorageError::UnsupportedStorageSchema {
            found,
            supported: STORAGE_SCHEMA_VERSION,
        });
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    transaction.execute_batch(FOUR_TABLE_SCHEMA)?;
    transaction.execute(
        "INSERT INTO world (id, manifest) VALUES (?1, ?2)",
        params![
            world_id.into_bytes().as_slice(),
            serde_json::to_string(WorldManifest::default().as_value())?,
        ],
    )?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_existing(connection: &mut Connection, path: &Path) -> StorageResult<()> {
    let found = storage_version(connection)?;
    match found {
        0 => Err(StorageError::Uninitialized),
        STORAGE_SCHEMA_VERSION => {
            ensure_four_tables(connection)?;
            Ok(())
        }
        1..=5 => migrate_legacy(connection, path, found),
        other => Err(StorageError::UnsupportedStorageSchema {
            found: other,
            supported: STORAGE_SCHEMA_VERSION,
        }),
    }
}

pub(super) fn storage_version(connection: &Connection) -> StorageResult<u32> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

fn ensure_four_tables(connection: &Connection) -> StorageResult<()> {
    let mut expected = vec!["items", "properties", "property_registry", "world"];
    let mut found = Vec::new();
    for name in &expected {
        let exists: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(StorageError::CorruptData(format!(
                "four-table world storage is missing the {name} table"
            )));
        }
        found.push(*name);
    }
    expected.sort_unstable();
    found.sort_unstable();
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".pre-four-table.bak");
    PathBuf::from(backup)
}

/// Converts one legacy event-sourced database into the four-table model.
fn migrate_legacy(
    connection: &mut Connection,
    path: &Path,
    legacy_version: u32,
) -> StorageResult<()> {
    let backup = backup_path(path);
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }

    let world_id = read_legacy_world_id(connection)?;
    let legacy = read_legacy_state(connection, legacy_version)?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    drop_legacy_tables(&transaction)?;
    transaction.execute_batch(FOUR_TABLE_SCHEMA)?;

    let manifest = serde_json::json!({
        "world_adapter": "four-table-v6",
        "migrated_from_storage_version": legacy_version,
        "world_facets": legacy.world_facets,
    });
    transaction.execute(
        "INSERT INTO world (id, manifest) VALUES (?1, ?2)",
        params![
            world_id.into_bytes().as_slice(),
            serde_json::to_string(&manifest)?,
        ],
    )?;

    write_legacy_definitions(&transaction, &legacy)?;
    write_legacy_items(&transaction, &legacy)?;
    write_legacy_properties(&transaction, &legacy)?;

    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;

    let fk_violations: i64 =
        transaction.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if fk_violations != 0 {
        return Err(StorageError::CorruptData(format!(
            "migrated world violates {fk_violations} foreign keys"
        )));
    }
    let integrity: String =
        transaction.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if !integrity.eq_ignore_ascii_case("ok") {
        return Err(StorageError::CorruptData(format!(
            "migrated world failed integrity_check: {integrity}"
        )));
    }

    transaction.commit()?;
    Ok(())
}

struct LegacyEntity {
    id: Vec<u8>,
    schema: SchemaKey,
}

struct LegacyRelation {
    id: Vec<u8>,
    schema: SchemaKey,
    subject: Vec<u8>,
    object: Vec<u8>,
}

struct LegacyProperty {
    item: Vec<u8>,
    key: SchemaKey,
    data: String,
}

#[derive(Default)]
struct LegacyState {
    world_facets: serde_json::Value,
    definitions: Vec<(SchemaKey, String)>,
    entities: Vec<LegacyEntity>,
    relations: Vec<LegacyRelation>,
    properties: Vec<LegacyProperty>,
}

fn read_legacy_world_id(connection: &Connection) -> StorageResult<WorldId> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT world_id FROM world_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StorageError::Uninitialized)?;
    super::codec::world_id_from_blob(bytes)
}

fn read_legacy_state(connection: &Connection, legacy_version: u32) -> StorageResult<LegacyState> {
    ensure_no_blocking_legacy_state(connection, legacy_version)?;

    let mut state = LegacyState {
        world_facets: serde_json::json!({}),
        ..LegacyState::default()
    };

    let mut facets = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT schema_id, schema_version, definition_json
             FROM world_schemas ORDER BY schema_id, schema_version",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (name, version, definition) = row?;
            let key = decode_key(name, version)?;
            state.definitions.push((key, definition));
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT entity_id, schema_id, schema_version FROM entities ORDER BY entity_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, version) = row?;
            state.entities.push(LegacyEntity {
                id,
                schema: decode_key(name, version)?,
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT relation_id, schema_id, schema_version, from_entity_id, to_entity_id
             FROM relations ORDER BY relation_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        for row in rows {
            let (id, name, version, subject, object) = row?;
            state.relations.push(LegacyRelation {
                id,
                schema: decode_key(name, version)?,
                subject,
                object,
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT entity_id, schema_id, schema_version, payload_json
             FROM entity_facets ORDER BY entity_id, schema_id, schema_version",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (item, name, version, data) = row?;
            state.properties.push(LegacyProperty {
                item,
                key: decode_key(name, version)?,
                data,
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT relation_id, schema_id, schema_version, payload_json
             FROM relation_facets ORDER BY relation_id, schema_id, schema_version",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (item, name, version, data) = row?;
            state.properties.push(LegacyProperty {
                item,
                key: decode_key(name, version)?,
                data,
            });
        }
    }

    {
        let mut statement = connection.prepare(
            "SELECT schema_id, schema_version, payload_json
             FROM world_facets ORDER BY schema_id, schema_version",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (name, version, data) = row?;
            let key = decode_key(name, version)?;
            facets.push((key, serde_json::from_str::<serde_json::Value>(&data)?));
        }
    }
    let mut world_facets = serde_json::Map::new();
    for (key, value) in facets {
        world_facets.insert(key.to_string(), value);
    }
    state.world_facets = serde_json::Value::Object(world_facets);

    Ok(state)
}

fn ensure_no_blocking_legacy_state(
    connection: &Connection,
    _legacy_version: u32,
) -> StorageResult<()> {
    let outstanding_jobs: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'effect_jobs'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if outstanding_jobs > 0 {
        let pending: i64 = connection.query_row(
            "SELECT count(*) FROM effect_jobs WHERE status != 2",
            [],
            |row| row.get(0),
        )?;
        if pending > 0 {
            return Err(StorageError::MigrationBlocked {
                reason: "durable effect jobs are pending or running; drain or export them before \
                         migrating, because the four-table model has no durable effect queue"
                    .into(),
            });
        }
    }

    let grants: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'control_grants'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if grants > 0 {
        let active: i64 =
            connection.query_row("SELECT count(*) FROM control_grants", [], |row| row.get(0))?;
        if active > 0 {
            return Err(StorageError::MigrationBlocked {
                reason: "durable control grants exist; export or revoke them explicitly, because \
                         the four-table model has no ControlGrant table"
                    .into(),
            });
        }
    }
    Ok(())
}

fn drop_legacy_tables(transaction: &rusqlite::Transaction<'_>) -> StorageResult<()> {
    transaction.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS world_event_consumers;
         DROP TABLE IF EXISTS effect_jobs;
         DROP TABLE IF EXISTS world_events;
         DROP TABLE IF EXISTS world_mutations;
         DROP TABLE IF EXISTS relation_facets;
         DROP TABLE IF EXISTS entity_facets;
         DROP TABLE IF EXISTS world_facets;
         DROP TABLE IF EXISTS relations;
         DROP TABLE IF EXISTS control_grant_schemas;
         DROP TABLE IF EXISTS control_grants;
         DROP TABLE IF EXISTS entities;
         DROP TABLE IF EXISTS world_commits;
         DROP TABLE IF EXISTS world_schemas;
         DROP TABLE IF EXISTS world_metadata;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(())
}

fn write_legacy_definitions(
    transaction: &rusqlite::Transaction<'_>,
    state: &LegacyState,
) -> StorageResult<()> {
    transaction.execute(
        "INSERT INTO property_registry (name, version, schema) VALUES (?1, ?2, ?3)",
        params![
            LEGACY_SCHEMA_PROPERTY,
            1_i64,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "schema": { "type": "string" },
                    "kind": { "type": "string" }
                },
                "required": ["schema", "kind"],
                "additionalProperties": false
            })
            .to_string()
        ],
    )?;

    for (key, definition) in &state.definitions {
        let name = key.id().as_str();
        if name == LEGACY_SCHEMA_PROPERTY {
            continue;
        }
        transaction.execute(
            "INSERT INTO property_registry (name, version, schema) VALUES (?1, ?2, ?3)",
            params![name, i64::from(key.version().get()), definition],
        )?;
    }
    Ok(())
}

fn write_legacy_items(
    transaction: &rusqlite::Transaction<'_>,
    state: &LegacyState,
) -> StorageResult<()> {
    for entity in &state.entities {
        transaction.execute(
            "INSERT INTO items (id, type, subject, object) VALUES (?1, 1, NULL, NULL)",
            params![entity.id],
        )?;
        transaction.execute(
            "INSERT INTO properties (item, name, version, data) VALUES (?1, ?2, 1, ?3)",
            params![
                entity.id,
                LEGACY_SCHEMA_PROPERTY,
                serde_json::json!({
                    "schema": entity.schema.to_string(),
                    "kind": "entity"
                })
                .to_string()
            ],
        )?;
    }

    for relation in &state.relations {
        transaction
            .execute(
                "INSERT INTO items (id, type, subject, object) VALUES (?1, 2, ?2, ?3)",
                params![relation.id, relation.subject, relation.object],
            )
            .map_err(|error| StorageError::MigrationBlocked {
                reason: format!(
                    "relation {} references an entity that does not exist: {error}",
                    String::from_utf8_lossy(&relation.id)
                ),
            })?;
        transaction.execute(
            "INSERT INTO properties (item, name, version, data) VALUES (?1, ?2, 1, ?3)",
            params![
                relation.id,
                LEGACY_SCHEMA_PROPERTY,
                serde_json::json!({
                    "schema": relation.schema.to_string(),
                    "kind": "relation"
                })
                .to_string()
            ],
        )?;
    }
    Ok(())
}

fn write_legacy_properties(
    transaction: &rusqlite::Transaction<'_>,
    state: &LegacyState,
) -> StorageResult<()> {
    for property in &state.properties {
        transaction.execute(
            "INSERT INTO properties (item, name, version, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                property.item,
                property.key.id().as_str(),
                i64::from(property.key.version().get()),
                property.data
            ],
        )?;
    }
    Ok(())
}

fn decode_key(name: String, version: i64) -> StorageResult<SchemaKey> {
    let name =
        SchemaId::parse(name).map_err(|error| StorageError::CorruptData(error.to_string()))?;
    let version = u32::try_from(version)
        .ok()
        .and_then(|value| SchemaVersion::new(value).ok())
        .ok_or_else(|| StorageError::CorruptData("invalid schema version".into()))?;
    Ok(SchemaKey::new(name, version))
}
