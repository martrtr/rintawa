//! SQLite-backed storage for one authoritative world.
//!
//! The database holds exactly four domain tables: `world`, `items`,
//! `property_registry` and `properties`. There is no durable command ledger,
//! mutation log, event log, consumer cursor, effect outbox or control grant.

mod apply;
mod codec;
mod query;
mod schema;
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
    WorldManifest, WorldSessionState, WorldTransaction,
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
        Self::create_with_manifest(path, world_id, &WorldManifest::default())
    }

    /// Creates a new database with its initial manifest in the same SQLite transaction.
    ///
    /// # Errors
    ///
    /// Returns an error on an existing destination or a failed initialization.
    pub fn create_with_manifest(
        path: impl AsRef<Path>,
        world_id: WorldId,
        manifest: &WorldManifest,
    ) -> StorageResult<Self> {
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

        match Self::initialize_new(path, world_id, manifest) {
            Ok(storage) => Ok(storage),
            Err(error) => {
                let _ = fs::remove_file(path);
                Err(error)
            }
        }
    }

    /// Opens only current four-table worlds; never converts legacy databases.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing entries, unsupported storage or world
    /// versions, corrupted metadata or SQLite errors. Incompatible files remain unchanged.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = validate_existing_file(path.as_ref())?;
        // Reject unknown worlds before allowing the writer connection to configure WAL.
        let world_id = Self::inspect_read_only(&path)?.id();

        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection(&connection)?;

        Ok(Self {
            path,
            world_id,
            writer: Mutex::new(connection),
        })
    }

    /// Reads the World from an existing database without opening a writer connection.
    ///
    /// The manifest and property registry are read in one consistent SQLite snapshot.
    /// No migration, WAL configuration or metadata creation is performed.
    ///
    /// # Errors
    ///
    /// Returns a storage error if the file, schema or current values are invalid.
    pub fn load_session_read_only(path: impl AsRef<Path>) -> StorageResult<WorldSessionState> {
        let path = validate_existing_file(path.as_ref())?;
        Self::inspect_read_only(&path)
    }

    fn inspect_read_only(path: &Path) -> StorageResult<WorldSessionState> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
        connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        schema::validate_existing(&connection)?;
        let world_id = query::load_world_id(&connection)?;
        let manifest = WorldManifest::new(query::load_manifest(&connection)?);
        let properties = query::load_registry(&connection)?;
        connection.execute_batch("COMMIT")?;
        Ok(WorldSessionState::restore(world_id, manifest, properties))
    }

    fn initialize_new(
        path: &Path,
        world_id: WorldId,
        manifest: &WorldManifest,
    ) -> StorageResult<Self> {
        let path = path.canonicalize()?;
        let mut connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        schema::initialize_new(&mut connection, world_id, manifest)?;

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
        connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        let manifest = WorldManifest::new(query::load_manifest(&connection)?);
        let properties = query::load_registry(&connection)?;
        connection.execute_batch("COMMIT")?;
        Ok(WorldSessionState::restore(
            self.world_id,
            manifest,
            properties,
        ))
    }

    /// Persists one definition through the same atomic batch path.
    ///
    /// The registry is checked after acquiring the SQLite writer lock.
    ///
    /// # Errors
    ///
    /// Returns a schema conflict, validation error or storage failure.
    pub fn register_definition(
        &self,
        definition: &PropertyDefinition,
    ) -> StorageResult<DefinitionRegistration> {
        let mut outcomes = self.register_definitions(std::slice::from_ref(definition))?;
        Ok(outcomes.remove(0))
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

    /// Atomically replaces the authoritative manifest with Host-validated metadata.
    ///
    /// This is a trusted host/storage operation, not a guest permission grant.
    /// Other tables are unaffected by a failed update.
    ///
    /// # Errors
    ///
    /// Returns an error if SQLite rejects the write or commit fails.
    pub fn update_manifest(&self, manifest: &WorldManifest) -> StorageResult<()> {
        let mut connection = self.writer()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE world SET manifest = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(manifest.as_value())?,
                self.world_id.into_bytes().as_slice()
            ],
        )?;
        transaction.commit()?;
        Ok(())
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

    /// Finds a bounded, ordered page of values for one versioned property key.
    ///
    /// The caller is trusted native Host code; guest authorization belongs to
    /// World Runtime. Results are capped at 256 entries and 8 MiB of JSON;
    /// an over-budget page fails instead of returning a truncated page. A
    /// read never changes the database.
    ///
    /// # Errors
    ///
    /// Returns a storage error or a rejected page size.
    pub fn find_properties(
        &self,
        key: &SchemaKey,
        after: Option<ItemId>,
        limit: usize,
    ) -> StorageResult<Vec<PropertyRecord>> {
        let connection = self.reader()?;
        query::find_properties(&connection, key, after, limit)
    }

    /// Reads a bounded page of relation-items next to any supplied items.
    ///
    /// # Errors
    ///
    /// Returns invalid page/batch parameters or a storage error.
    pub fn relations_touching_any(
        &self,
        targets: &[ItemId],
        after: Option<ItemId>,
        limit: usize,
    ) -> StorageResult<Vec<ItemRecord>> {
        let connection = self.reader()?;
        query::relations_touching_any(&connection, targets, after, limit)
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
