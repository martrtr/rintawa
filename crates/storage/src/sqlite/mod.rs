//! SQLite-backed storage for one authoritative world.
//!
//! The database holds exactly four domain tables: `world`, `items`,
//! `property_registry` and `properties`. There is no durable command ledger,
//! mutation log, event log, consumer cursor, effect outbox or control grant.

mod apply;
mod codec;
mod migration;
mod query;
mod snapshot;

#[cfg(test)]
mod tests;

use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_world::{
    DefinitionRegistration, ItemRecord, PropertyDefinition, PropertyRecord, PropertyRegistry,
    WORLD_FORMAT_VERSION, WorldManifest, WorldSessionState, WorldTransaction,
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::{StorageError, StorageResult};

pub use self::snapshot::SqliteWorldSnapshot;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite storage for exactly one authoritative world.
///
/// One database file embeds one WorldId. Writes use SQLite `IMMEDIATE`
/// transactions and the runtime exposes one logical writer queue per active
/// world, so no persisted revision counter is needed. Readers open their own WAL
/// connections and pin a snapshot.
#[derive(Debug)]
pub struct SqliteWorldStorage {
    path: PathBuf,
    world_id: WorldId,
    writer: Mutex<Connection>,
}

impl SqliteWorldStorage {
    /// Creates a new SQLite file for one empty authoritative world.
    ///
    /// # Errors
    ///
    /// Returns StorageError::AlreadyExists when the destination exists, or another
    /// storage error when initialization cannot complete.
    pub fn create(path: impl AsRef<Path>, world_id: WorldId) -> StorageResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => drop(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StorageError::AlreadyExists(path.to_path_buf()));
            }
            Err(error) => return Err(error.into()),
        }

        match Self::initialize_new(path, world_id) {
            Ok(storage) => Ok(storage),
            Err(error) => {
                let _ = fs::remove_file(path);
                Err(error)
            }
        }
    }

    /// Opens an existing world database and applies supported storage migrations.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing entries, unsupported storage or world
    /// versions, corrupted metadata, refused legacy conversions or SQLite errors.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = validate_existing_file(path.as_ref())?;
        let mut connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        migration::migrate_existing(&mut connection, &path)?;

        let world_id = query::load_world_id(&connection)?;
        ensure_world_format(&connection)?;

        Ok(Self {
            path,
            world_id,
            writer: Mutex::new(connection),
        })
    }

    fn initialize_new(path: &Path, world_id: WorldId) -> StorageResult<Self> {
        let path = path.canonicalize()?;
        let mut connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        migration::initialize_new(&mut connection, world_id)?;

        Ok(Self {
            path,
            world_id,
            writer: Mutex::new(connection),
        })
    }

    /// Returns the canonical database file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the authoritative identity embedded in this database.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Restores durable world metadata and the property vocabulary.
    ///
    /// # Errors
    ///
    /// Returns a storage error if persisted data is corrupt or a definition cannot
    /// be compiled.
    pub fn load_session(&self) -> StorageResult<WorldSessionState> {
        let connection = self.reader()?;
        ensure_world_format(&connection)?;
        let manifest = WorldManifest::new(query::load_manifest(&connection)?);
        let properties = query::load_registry(&connection)?;
        Ok(WorldSessionState::restore(
            self.world_id,
            manifest,
            properties,
        ))
    }

    /// Persists one immutable property definition.
    ///
    /// Exact re-registration is idempotent; mutating an existing `(name, version)`
    /// is rejected by SQLite. The definition is compiled locally as JSON Schema
    /// before publication.
    ///
    /// # Errors
    ///
    /// Returns a definition conflict/validation error or a SQLite/storage error.
    pub fn register_definition(
        &self,
        definition: &PropertyDefinition,
    ) -> StorageResult<DefinitionRegistration> {
        let mut registry = self.property_registry()?;
        let registration = registry.register(definition.clone())?;
        let connection = self.writer()?;
        connection.execute(
            "INSERT INTO property_registry (name, version, schema) VALUES (?1, ?2, ?3)
             ON CONFLICT (name, version) DO NOTHING",
            params![
                definition.key().id().as_str(),
                i64::from(definition.key().version().get()),
                serde_json::to_string(definition.schema())?,
            ],
        )?;
        Ok(registration)
    }

    /// Persists a set of property definitions atomically.
    ///
    /// # Errors
    ///
    /// Returns a conflict/validation error, a serialization error or a SQLite error.
    pub fn register_definitions(
        &self,
        definitions: &[PropertyDefinition],
    ) -> StorageResult<Vec<DefinitionRegistration>> {
        if definitions.is_empty() {
            return Ok(Vec::new());
        }

        let mut connection = self.writer()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut registry = query::load_registry(&transaction)?;
        let mut registrations = Vec::with_capacity(definitions.len());

        for definition in definitions {
            let registration = registry.register(definition.clone())?;
            if registration == DefinitionRegistration::Registered {
                transaction.execute(
                    "INSERT INTO property_registry (name, version, schema) VALUES (?1, ?2, ?3)
                     ON CONFLICT (name, version) DO NOTHING",
                    params![
                        definition.key().id().as_str(),
                        i64::from(definition.key().version().get()),
                        serde_json::to_string(definition.schema())?,
                    ],
                )?;
            }
            registrations.push(registration);
        }

        transaction.commit()?;
        Ok(registrations)
    }

    /// Opens a consistent read-only snapshot of current authoritative state.
    ///
    /// # Errors
    ///
    /// Returns a storage error when metadata cannot be read or the snapshot
    /// transaction cannot be opened.
    pub fn snapshot(&self) -> StorageResult<SqliteWorldSnapshot> {
        let connection = self.reader()?;
        connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        ensure_world_format(&connection)?;
        Ok(SqliteWorldSnapshot::new(connection, self.world_id))
    }

    /// Applies one validated change set atomically and returns after COMMIT.
    ///
    /// The change set has already been validated against the property vocabulary
    /// by the caller; SQLite enforces identity, endpoint and foreign-key
    /// invariants. Callers must not report success before this returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns a world or storage error. No partial change is committed.
    pub fn apply(&self, transaction: &WorldTransaction) -> StorageResult<()> {
        let mut connection = self.writer()?;
        apply::apply(&mut connection, transaction)
    }

    /// Loads one item.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_item(&self, id: ItemId) -> StorageResult<Option<ItemRecord>> {
        let connection = self.reader()?;
        query::load_item(&connection, id)
    }

    /// Loads one property value.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_property(
        &self,
        item: ItemId,
        key: &SchemaKey,
    ) -> StorageResult<Option<PropertyRecord>> {
        let connection = self.reader()?;
        query::load_property(&connection, item, key)
    }

    /// Loads every current property of one item.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn properties_of(&self, item: ItemId) -> StorageResult<Vec<PropertyRecord>> {
        let connection = self.reader()?;
        query::list_properties(&connection, item)
    }

    /// Loads the property vocabulary persisted in this world.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted definitions cannot be decoded.
    pub fn property_registry(&self) -> StorageResult<PropertyRegistry> {
        let connection = self.reader()?;
        query::load_registry(&connection)
    }

    fn writer(&self) -> StorageResult<MutexGuard<'_, Connection>> {
        self.writer
            .lock()
            .map_err(|_| StorageError::ConnectionUnavailable)
    }

    fn reader(&self) -> StorageResult<Connection> {
        let connection = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
        Ok(connection)
    }
}

fn validate_existing_file(path: &Path) -> StorageResult<PathBuf> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StorageError::NotFound(path.to_path_buf()));
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StorageError::InvalidEntry {
            path: path.to_path_buf(),
            reason: "world database path must be a regular file",
        });
    }
    Ok(path.canonicalize()?)
}

fn configure_connection(connection: &Connection) -> StorageResult<()> {
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StorageError::CorruptData(
            "SQLite refused WAL journal mode".into(),
        ));
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn ensure_world_format(connection: &Connection) -> StorageResult<()> {
    let manifest = query::load_manifest(connection)?;
    let found = manifest
        .get("manifest_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(u64::from(WORLD_FORMAT_VERSION));
    if found > u64::from(WORLD_FORMAT_VERSION) {
        return Err(StorageError::UnsupportedWorldFormat {
            found: u32::try_from(found).unwrap_or(u32::MAX),
            supported: WORLD_FORMAT_VERSION,
        });
    }
    Ok(())
}
