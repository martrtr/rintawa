//! Four-table SQLite schema; new-world initialization only.

use rintawa_sdk::world::WorldId;
use rintawa_world::WorldManifest;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::{StorageError, StorageResult};

pub(super) const STORAGE_SCHEMA_VERSION: u32 = 1;

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
CREATE INDEX properties_by_name_version_item ON properties(name, version, item);

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

pub(super) fn initialize_new(
    connection: &mut Connection,
    world_id: WorldId,
    manifest: &WorldManifest,
) -> StorageResult<()> {
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
            serde_json::to_string(manifest.as_value())?,
        ],
    )?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn storage_version(connection: &Connection) -> StorageResult<u32> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

pub(super) fn validate_existing(connection: &Connection) -> StorageResult<()> {
    let found = storage_version(connection)?;
    if found == 0 {
        return Err(StorageError::Uninitialized);
    }
    if found != STORAGE_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedStorageSchema {
            found,
            supported: STORAGE_SCHEMA_VERSION,
        });
    }

    // Let SQLite parse the one supported DDL. Compare definitions by name, never
    // sqlite_master row order (which differs after backup/VACUUM).
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(FOUR_TABLE_SCHEMA)?;
    if schema_objects(connection)? != schema_objects(&expected)? {
        return Err(StorageError::CorruptData(
            "unrecognized world schema: definitions differ from current DDL".into(),
        ));
    }

    let world_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM world", [], |row| row.get(0))?;
    if world_count != 1 {
        return Err(StorageError::CorruptData(
            "four-table world must contain exactly one world record".into(),
        ));
    }
    let violations: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(StorageError::CorruptData(
            "four-table world has foreign-key violations".into(),
        ));
    }
    Ok(())
}

fn schema_objects(connection: &Connection) -> StorageResult<Vec<(String, String, String, String)>> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_master
         WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}
