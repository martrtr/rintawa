//! Native, read-only seed inspection and trusted four-table copying.

use std::{fs, path::Path};

use rintawa_sdk::world::{SchemaKey, WorldId};
use rintawa_world::{PropertyRegistry, WorldManifest, WorldSessionState};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::{StorageError, StorageResult};

use crate::sqlite::{
    SQLITE_BUSY_TIMEOUT, SqliteWorldStorage, codec, query, schema, validate_existing_file,
};

fn validated_seed(source: &Path) -> StorageResult<(Connection, PropertyRegistry)> {
    let path = validate_existing_file(source)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
    // All subsequent reads use this same pinned SQLite snapshot.
    schema::validate_existing(&connection)?;
    let integrity: String =
        connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(StorageError::CorruptData(
            "seed failed SQLite integrity_check".into(),
        ));
    }
    if !query::load_manifest(&connection)?.is_object() {
        return Err(StorageError::CorruptData(
            "seed manifest must be an object".into(),
        ));
    }
    let registry = query::load_registry(&connection)?;
    validate_items(&connection)?;
    validate_properties(&connection, &registry)?;
    Ok((connection, registry))
}

fn validate_items(connection: &Connection) -> StorageResult<()> {
    let mut statement =
        connection.prepare("SELECT id, type, subject, object FROM items ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id = codec::item_id_from_blob(row.get(0)?, "seed item")?;
        let kind: i64 = row.get(1)?;
        let subject: Option<Vec<u8>> = row.get(2)?;
        let object: Option<Vec<u8>> = row.get(3)?;
        match (kind, subject, object) {
            (1, None, None) => {}
            (2, Some(subject), Some(object)) => {
                let _ = codec::item_id_from_blob(subject, "relation subject")?;
                let _ = codec::item_id_from_blob(object, "relation object")?;
            }
            _ => return Err(StorageError::CorruptData(format!("invalid seed item {id}"))),
        }
    }
    // Foreign keys alone allow a relation to point at another relation. The
    // existing triggers prevent that on writes, but an imported file is untrusted.
    let invalid: i64 = connection.query_row(
        "SELECT COUNT(*) FROM items AS r
         LEFT JOIN items AS s ON s.id = r.subject
         LEFT JOIN items AS o ON o.id = r.object
         WHERE r.type = 2 AND (s.type IS NOT 1 OR o.type IS NOT 1)",
        [],
        |row| row.get(0),
    )?;
    if invalid != 0 {
        return Err(StorageError::CorruptData(
            "seed relation endpoint is not an object".into(),
        ));
    }
    Ok(())
}

fn validate_properties(connection: &Connection, registry: &PropertyRegistry) -> StorageResult<()> {
    let mut statement = connection
        .prepare("SELECT item, name, version, data FROM properties ORDER BY item, name")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let _ = codec::item_id_from_blob(row.get(0)?, "seed property item")?;
        let key: SchemaKey = codec::decode_property_key(row.get(1)?, row.get(2)?)?;
        let value: serde_json::Value = serde_json::from_str(&row.get::<_, String>(3)?)?;
        registry.validate(&key, &value)?;
    }
    Ok(())
}

fn copy_table(
    source: &Connection,
    destination: &Connection,
    select: &str,
    insert: &str,
    columns: usize,
) -> StorageResult<()> {
    let mut read = source.prepare(select)?;
    let mut write = destination.prepare(insert)?;
    let mut rows = read.query([])?;
    while let Some(row) = rows.next()? {
        let values = (0..columns)
            .map(|index| row.get::<_, rusqlite::types::Value>(index))
            .collect::<Result<Vec<_>, _>>()?;
        write.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(())
}

impl SqliteWorldStorage {
    /// Validates a stopped, current-format seed without changing its bytes.
    ///
    /// The inspection checks exact DDL, SQLite integrity and foreign keys, graph
    /// endpoints and every property payload against its persisted JSON Schema.
    /// Schema ownership is checked by Host against the selected composition.
    ///
    /// # Errors
    /// Returns a storage or validation error for unsafe paths or corrupt state.
    pub fn validate_seed(path: impl AsRef<Path>) -> StorageResult<WorldSessionState> {
        let (source, registry) = validated_seed(path.as_ref())?;
        let id = query::load_world_id(&source)?;
        let manifest = WorldManifest::new(query::load_manifest(&source)?);
        Ok(WorldSessionState::restore(id, manifest, registry))
    }

    /// Copies a validated seed into fresh trusted DDL under a new World ID.
    ///
    /// Item and relation IDs and property bytes remain logically unchanged.
    /// The supplied manifest represents the new World, including Host metadata.
    /// The caller must publish the containing directory only after its composition
    /// and assets have also been validated. Existing destinations are never replaced.
    ///
    /// # Errors
    /// Returns a validation/storage error; a created incomplete destination is removed.
    pub fn create_from_seed(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        world_id: WorldId,
        manifest: &WorldManifest,
    ) -> StorageResult<Self> {
        let (source, _) = validated_seed(source.as_ref())?;
        let storage = Self::create_with_manifest(destination.as_ref(), world_id, manifest)?;
        let result: StorageResult<()> = (|| {
            let mut writer = storage.writer()?;
            let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
            copy_table(
                &source,
                &tx,
                "SELECT id, type, subject, object FROM items WHERE type = 1 ORDER BY id",
                "INSERT INTO items(id,type,subject,object) VALUES (?1,?2,?3,?4)",
                4,
            )?;
            copy_table(
                &source,
                &tx,
                "SELECT id, type, subject, object FROM items WHERE type = 2 ORDER BY id",
                "INSERT INTO items(id,type,subject,object) VALUES (?1,?2,?3,?4)",
                4,
            )?;
            copy_table(
                &source,
                &tx,
                "SELECT name, version, schema FROM property_registry ORDER BY name, version",
                "INSERT INTO property_registry(name,version,schema) VALUES (?1,?2,?3)",
                3,
            )?;
            copy_table(
                &source,
                &tx,
                "SELECT item, name, version, data FROM properties ORDER BY item, name",
                "INSERT INTO properties(item,name,version,data) VALUES (?1,?2,?3,?4)",
                4,
            )?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            storage.delete()?;
            return Err(error);
        }
        Ok(storage)
    }

    /// Writes one consistent copy of committed state while this object owns the writer.
    ///
    /// SQLite VACUUM INTO includes committed WAL data. A new destination is
    /// reserved exclusively so even an existing empty file is never overwritten.
    ///
    /// # Errors
    /// Returns a storage error; incomplete output created by this call is removed.
    pub fn export_snapshot(&self, destination: impl AsRef<Path>) -> StorageResult<()> {
        let destination = destination.as_ref();
        let destination_name = destination
            .to_str()
            .ok_or_else(|| StorageError::InvalidEntry {
                path: destination.to_path_buf(),
                reason: "snapshot path must be valid UTF-8 for SQLite VACUUM INTO",
            })?;
        let _writer = self.writer()?;
        crate::sqlite::verify_owned_path(&self._ownership, &self.path)?;
        // VACUUM INTO accepts an already existing empty file, so reserve using
        // create_new and keep cleanup responsibility for only this pathname.
        let reservation = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        drop(reservation);
        let result = (|| {
            let connection = self.reader()?;
            connection.execute("VACUUM INTO ?1", params![destination_name])?;
            let copy = validated_seed(destination)?;
            drop(copy);
            Ok(())
        })();
        if result.is_err() {
            fs::remove_file(destination)?;
        }
        result
    }
}
