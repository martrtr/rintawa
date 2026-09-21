//! SQLite storage schema initialization and migrations.

use rintawa_sdk::world::WorldId;
use rintawa_world::WORLD_FORMAT_VERSION;
use rusqlite::{Connection, params};

use crate::{StorageError, StorageResult};

pub(super) const STORAGE_SCHEMA_VERSION: u32 = 2;

pub(super) fn initialize_new(connection: &mut Connection, world_id: WorldId) -> StorageResult<()> {
    let found = storage_version(connection)?;
    if found != 0 {
        return Err(StorageError::UnsupportedStorageSchema {
            found,
            supported: STORAGE_SCHEMA_VERSION,
        });
    }

    migrate_to_v1(connection)?;
    connection.execute(
        "INSERT INTO world_metadata (
            singleton, world_id, world_format_version, commit_position
         ) VALUES (1, ?1, ?2, 0)",
        params![
            world_id.into_bytes().as_slice(),
            i64::from(WORLD_FORMAT_VERSION)
        ],
    )?;
    migrate_to_v2(connection)?;
    Ok(())
}

pub(super) fn migrate_existing(connection: &mut Connection) -> StorageResult<()> {
    let mut found = storage_version(connection)?;
    if found == 0 {
        return Err(StorageError::Uninitialized);
    }
    if found > STORAGE_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedStorageSchema {
            found,
            supported: STORAGE_SCHEMA_VERSION,
        });
    }

    if found == 1 {
        let commit_position: i64 = connection.query_row(
            "SELECT commit_position FROM world_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if commit_position != 0 {
            return Err(StorageError::CorruptData(
                "storage schema v1 cannot contain committed world history".into(),
            ));
        }
        migrate_to_v2(connection)?;
        found = 2;
    }

    if found != STORAGE_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedStorageSchema {
            found,
            supported: STORAGE_SCHEMA_VERSION,
        });
    }
    Ok(())
}

pub(super) fn storage_version(connection: &Connection) -> StorageResult<u32> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

pub(super) fn migrate_to_v1(connection: &mut Connection) -> StorageResult<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE world_metadata (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            world_id BLOB NOT NULL CHECK (length(world_id) = 16),
            world_format_version INTEGER NOT NULL CHECK (world_format_version > 0),
            commit_position INTEGER NOT NULL CHECK (commit_position >= 0)
        );

        CREATE TABLE world_schemas (
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            kind INTEGER NOT NULL,
            owner_extension_id TEXT NOT NULL,
            definition_json TEXT NOT NULL,
            PRIMARY KEY (schema_id, schema_version)
        );",
    )?;
    transaction.pragma_update(None, "user_version", 1_u32)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_to_v2(connection: &mut Connection) -> StorageResult<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE world_commits (
            commit_position INTEGER PRIMARY KEY CHECK (commit_position > 0),
            command_id BLOB NOT NULL UNIQUE CHECK (length(command_id) = 16),
            command_schema_id TEXT NOT NULL,
            command_schema_version INTEGER NOT NULL CHECK (command_schema_version > 0),
            principal_id BLOB NOT NULL CHECK (length(principal_id) = 16),
            actor_kind INTEGER NOT NULL CHECK (actor_kind IN (1, 2)),
            actor_id BLOB NOT NULL CHECK (length(actor_id) = 16),
            causation_kind INTEGER CHECK (causation_kind IN (1, 2, 3)),
            causation_id BLOB CHECK (causation_id IS NULL OR length(causation_id) = 16),
            correlation_id BLOB NOT NULL CHECK (length(correlation_id) = 16),
            expected_position INTEGER CHECK (expected_position IS NULL OR expected_position >= 0),
            effective_at_ms INTEGER,
            recorded_at_ms INTEGER NOT NULL,
            command_payload_json TEXT NOT NULL,
            command_digest BLOB NOT NULL CHECK (length(command_digest) = 32),
            CHECK (
                (causation_kind IS NULL AND causation_id IS NULL)
                OR (causation_kind IS NOT NULL AND causation_id IS NOT NULL)
            ),
            FOREIGN KEY (command_schema_id, command_schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE entities (
            entity_id BLOB PRIMARY KEY CHECK (length(entity_id) = 16),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            created_position INTEGER NOT NULL,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (created_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE control_grants (
            grant_id BLOB PRIMARY KEY CHECK (length(grant_id) = 16),
            principal_id BLOB NOT NULL CHECK (length(principal_id) = 16),
            actor_entity_id BLOB NOT NULL CHECK (length(actor_entity_id) = 16),
            scope_kind INTEGER NOT NULL CHECK (scope_kind IN (1, 2)),
            valid_through_position INTEGER
                CHECK (valid_through_position IS NULL OR valid_through_position >= 0),
            created_position INTEGER NOT NULL,
            FOREIGN KEY (actor_entity_id)
                REFERENCES entities(entity_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (created_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE INDEX control_grants_lookup_idx
            ON control_grants(principal_id, actor_entity_id, scope_kind);

        CREATE TABLE control_grant_schemas (
            grant_id BLOB NOT NULL CHECK (length(grant_id) = 16),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            PRIMARY KEY (grant_id, schema_id, schema_version),
            FOREIGN KEY (grant_id)
                REFERENCES control_grants(grant_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE relations (
            relation_id BLOB PRIMARY KEY CHECK (length(relation_id) = 16),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            from_entity_id BLOB NOT NULL CHECK (length(from_entity_id) = 16),
            to_entity_id BLOB NOT NULL CHECK (length(to_entity_id) = 16),
            created_position INTEGER NOT NULL,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (from_entity_id)
                REFERENCES entities(entity_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (to_entity_id)
                REFERENCES entities(entity_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (created_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE INDEX relations_from_entity_idx ON relations(from_entity_id);
        CREATE INDEX relations_to_entity_idx ON relations(to_entity_id);

        CREATE TABLE world_facets (
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (schema_id, schema_version),
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (updated_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE entity_facets (
            entity_id BLOB NOT NULL CHECK (length(entity_id) = 16),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (entity_id, schema_id, schema_version),
            FOREIGN KEY (entity_id)
                REFERENCES entities(entity_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (updated_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE relation_facets (
            relation_id BLOB NOT NULL CHECK (length(relation_id) = 16),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            payload_json TEXT NOT NULL,
            updated_position INTEGER NOT NULL,
            PRIMARY KEY (relation_id, schema_id, schema_version),
            FOREIGN KEY (relation_id)
                REFERENCES relations(relation_id)
                ON UPDATE RESTRICT ON DELETE CASCADE,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (updated_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE TABLE world_mutations (
            commit_position INTEGER NOT NULL,
            mutation_index INTEGER NOT NULL CHECK (mutation_index >= 0),
            format_version INTEGER NOT NULL CHECK (format_version > 0),
            mutation_json TEXT NOT NULL,
            PRIMARY KEY (commit_position, mutation_index),
            FOREIGN KEY (commit_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE INDEX world_mutations_position_idx
            ON world_mutations(commit_position, mutation_index);

        CREATE TABLE world_events (
            event_id BLOB PRIMARY KEY CHECK (length(event_id) = 16),
            commit_position INTEGER NOT NULL,
            event_index INTEGER NOT NULL CHECK (event_index >= 0),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            payload_json TEXT NOT NULL,
            UNIQUE (commit_position, event_index),
            FOREIGN KEY (commit_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE INDEX world_events_position_idx
            ON world_events(commit_position, event_index);

        CREATE TABLE effect_jobs (
            job_id BLOB PRIMARY KEY CHECK (length(job_id) = 16),
            commit_position INTEGER NOT NULL,
            job_index INTEGER NOT NULL CHECK (job_index >= 0),
            schema_id TEXT NOT NULL,
            schema_version INTEGER NOT NULL CHECK (schema_version > 0),
            payload_json TEXT NOT NULL,
            status INTEGER NOT NULL DEFAULT 0 CHECK (status IN (0, 1, 2)),
            attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
            last_error TEXT,
            UNIQUE (commit_position, job_index),
            FOREIGN KEY (commit_position)
                REFERENCES world_commits(commit_position)
                ON UPDATE RESTRICT ON DELETE RESTRICT,
            FOREIGN KEY (schema_id, schema_version)
                REFERENCES world_schemas(schema_id, schema_version)
                ON UPDATE RESTRICT ON DELETE RESTRICT
        );

        CREATE INDEX effect_jobs_pending_idx
            ON effect_jobs(status, commit_position, job_index);",
    )?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}
