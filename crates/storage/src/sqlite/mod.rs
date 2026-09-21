//! SQLite-backed storage for one authoritative world.

mod codec;
mod commit;
mod migration;
mod outbox;
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

use rintawa_sdk::world::{EffectJobId, EntityId, RelationId, SchemaKey, UnixTimeMillis, WorldId};
use rintawa_world::{
    CommitReceipt, EntityRecord, FacetRecord, FacetTarget, RelationRecord, SchemaDefinition,
    SchemaRegistration, SchemaRegistry, StoredEffectJob, StoredWorldEvent, StoredWorldMutation,
    WORLD_FORMAT_VERSION, WorldCommand, WorldSessionState, WorldTransaction,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::{ClaimedEffectJob, StorageError, StorageResult};

use self::codec::{load_schema, load_schema_registry, schema_kind_code, world_id_from_blob};

pub use self::snapshot::SqliteWorldSnapshot;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// SQLite storage for exactly one authoritative world.
///
/// One database file embeds one WorldId. The commit path uses SQLite IMMEDIATE
/// transactions, while the host/runtime is still expected to expose one logical
/// writer queue per active world. WAL permits independent read connections later
/// without changing the public world model.
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
    /// Returns StorageError::AlreadyExists when the destination already exists,
    /// or another storage error when initialization cannot complete.
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
    /// Returns a typed error for missing/invalid filesystem entries, unsupported
    /// storage/world versions, corrupted metadata, migration failures, or SQLite errors.
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let path = validate_existing_file(path.as_ref())?;
        let mut connection = Connection::open(&path)?;
        configure_connection(&connection)?;
        migration::migrate_existing(&mut connection)?;

        let (world_id, world_format, _) = read_world_metadata_full(&connection)?;
        ensure_world_format(world_format)?;

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

    /// Restores durable world metadata and registered schema definitions.
    ///
    /// # Errors
    ///
    /// Returns a storage or validation error if persisted data is corrupt.
    pub fn load_session(&self) -> StorageResult<WorldSessionState> {
        let connection = self.reader()?;
        let (_, world_format, commit_position) = read_world_metadata_full(&connection)?;
        ensure_world_format(world_format)?;
        let schemas = load_schema_registry(&connection)?;
        Ok(WorldSessionState::restore(
            self.world_id,
            commit_position,
            schemas,
        ))
    }

    /// Persists one immutable versioned schema definition.
    ///
    /// Exact re-registration is idempotent. Definitions are compiled locally as
    /// JSON Schema before publication; external schema resolution is unavailable.
    ///
    /// # Errors
    ///
    /// Returns a schema conflict/validation error, serialization error, or SQLite error.
    pub fn register_schema(
        &self,
        definition: &SchemaDefinition,
    ) -> StorageResult<SchemaRegistration> {
        let connection = self.writer()?;
        if let Some(existing) = load_schema(&connection, definition.key())? {
            let mut registry = SchemaRegistry::new();
            registry.register(existing)?;
            return Ok(registry.register(definition.clone())?);
        }

        let mut validation_registry = SchemaRegistry::new();
        validation_registry.register(definition.clone())?;
        let definition_json = serde_json::to_string(definition.definition())?;
        connection.execute(
            "INSERT INTO world_schemas (
                schema_id, schema_version, kind, owner_extension_id, definition_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                definition.key().id().as_str(),
                i64::from(definition.key().version().get()),
                schema_kind_code(definition.kind()),
                definition.owner().as_str(),
                definition_json,
            ],
        )?;
        Ok(SchemaRegistration::Registered)
    }

    /// Returns the previous receipt when an identical command already committed.
    ///
    /// This lookup happens before System execution so crash/retry can avoid
    /// regenerating nondeterministic transaction output.
    ///
    /// # Errors
    ///
    /// Returns CommandIdConflict when the same ID was previously used for
    /// different immutable command input, or a storage decoding error.
    pub fn committed_receipt(
        &self,
        command: &WorldCommand,
    ) -> StorageResult<Option<CommitReceipt>> {
        let connection = self.reader()?;
        commit::committed_receipt(&connection, command)
    }

    /// Opens a consistent read-only snapshot of current authoritative state.
    ///
    /// The SQLite read transaction remains pinned for the lifetime of the
    /// returned value while WAL writers may continue committing newer state.
    ///
    /// # Errors
    ///
    /// Returns a storage error when metadata cannot be read or the snapshot
    /// transaction cannot be opened.
    pub fn snapshot(&self) -> StorageResult<SqliteWorldSnapshot> {
        let connection = self.reader()?;
        connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        let (world_id, world_format, position) = read_world_metadata_full(&connection)?;
        ensure_world_format(world_format)?;
        Ok(SqliteWorldSnapshot::new(connection, world_id, position))
    }

    /// Validates a command and pins the exact snapshot it must evaluate.
    ///
    /// Command schema, optimistic position and Actor/ControlGrant checks are
    /// evaluated inside the same SQLite read transaction later used by the
    /// System, so repeated reads cannot drift across authoritative commits.
    ///
    /// # Errors
    ///
    /// Returns command/schema/authority/concurrency or storage errors.
    pub fn snapshot_for_command(
        &self,
        command: &WorldCommand,
    ) -> StorageResult<SqliteWorldSnapshot> {
        let connection = self.reader()?;
        connection.execute_batch("BEGIN DEFERRED TRANSACTION")?;
        let position = commit::validate_command(&connection, command)?;
        Ok(SqliteWorldSnapshot::new(
            connection,
            self.world_id,
            position,
        ))
    }

    /// Atomically commits one validated command result.
    ///
    /// The command identity and immutable input provide idempotency. Current-state
    /// mutations, Core mutation log entries, semantic events, durable effects,
    /// authority changes, provenance, and commit position are one SQLite transaction.
    /// The evaluated position must match the current position unless the command was
    /// already committed and is being replayed idempotently.
    ///
    /// # Errors
    ///
    /// Returns command/schema/authority/concurrency/current-state/storage errors.
    /// No partial world change is committed on failure.
    pub fn commit(
        &self,
        command: &WorldCommand,
        transaction: &WorldTransaction,
        evaluated_position: u64,
    ) -> StorageResult<CommitReceipt> {
        let mut connection = self.writer()?;
        commit::commit(&mut connection, command, transaction, evaluated_position)
    }

    /// Loads one current entity by identity.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_entity(&self, entity_id: EntityId) -> StorageResult<Option<EntityRecord>> {
        let connection = self.reader()?;
        query::load_entity(&connection, entity_id)
    }

    /// Loads one current relation by identity.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_relation(&self, relation_id: RelationId) -> StorageResult<Option<RelationRecord>> {
        let connection = self.reader()?;
        query::load_relation(&connection, relation_id)
    }

    /// Loads one current facet value.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted state cannot be decoded.
    pub fn load_facet(
        &self,
        target: FacetTarget,
        schema: &SchemaKey,
    ) -> StorageResult<Option<FacetRecord>> {
        let connection = self.reader()?;
        query::load_facet(&connection, target, schema)
    }

    /// Reads append-only Core mutations after an exclusive commit position.
    ///
    /// The requested batch is bounded internally to protect host memory.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted mutations cannot be decoded.
    pub fn mutations_after(
        &self,
        position: u64,
        limit: usize,
    ) -> StorageResult<Vec<StoredWorldMutation>> {
        let connection = self.reader()?;
        query::mutations_after(&connection, position, limit)
    }

    /// Reads durable events from one exact authoritative commit position.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted events cannot be decoded.
    pub fn events_at_position(&self, position: u64) -> StorageResult<Vec<StoredWorldEvent>> {
        let connection = self.reader()?;
        query::events_at_position(&connection, self.world_id, position)
    }

    /// Reads durable events after an exclusive commit position.
    ///
    /// The requested batch is bounded internally to protect host memory.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted events cannot be decoded.
    pub fn events_after(
        &self,
        position: u64,
        limit: usize,
    ) -> StorageResult<Vec<StoredWorldEvent>> {
        let connection = self.reader()?;
        query::events_after(&connection, self.world_id, position, limit)
    }

    /// Reads pending durable external effect jobs in deterministic enqueue order.
    ///
    /// Delayed retries remain pending and can appear here before their next
    /// `available_at`; workers should use `claim_next_effect` for due-job selection.
    /// The requested batch is bounded internally to protect host memory.
    ///
    /// # Errors
    ///
    /// Returns a storage error when persisted jobs cannot be decoded.
    pub fn pending_effects(&self, limit: usize) -> StorageResult<Vec<StoredEffectJob>> {
        let connection = self.reader()?;
        query::pending_effects(&connection, self.world_id, limit)
    }

    /// Atomically leases the oldest due or expired durable effect job.
    ///
    /// A reclaimed expired job receives a new attempt number. The returned
    /// attempt is a fencing token: completion/retry from any older claim is
    /// rejected after reclaim. External execution must use the stable job ID as
    /// its idempotency key because an expired worker may still resume outside
    /// the database transaction.
    ///
    /// # Errors
    ///
    /// Returns an invalid-lease, attempt-overflow, corruption, or SQLite error.
    pub fn claim_next_effect(
        &self,
        now: UnixTimeMillis,
        lease_expires_at: UnixTimeMillis,
    ) -> StorageResult<Option<ClaimedEffectJob>> {
        let mut connection = self.writer()?;
        outbox::claim_next(&mut connection, self.world_id, now, lease_expires_at)
    }

    /// Marks one currently leased effect job complete.
    ///
    /// # Errors
    ///
    /// Returns EffectJobClaimLost when the attempt no longer owns the lease, or
    /// EffectJobNotFound when the durable job identity is unknown.
    pub fn complete_effect(&self, job_id: EffectJobId, attempt: u32) -> StorageResult<()> {
        let connection = self.writer()?;
        outbox::complete(&connection, job_id, attempt)
    }

    /// Returns one currently leased effect job to the pending queue.
    ///
    /// `available_at` controls the earliest future claim and therefore lets the
    /// worker implement backoff without sleeping while holding a lease. `diagnostic`
    /// is durable and must already be a sanitized, non-secret summary; storage
    /// bounds it but cannot infer provider-specific secret formats.
    ///
    /// # Errors
    ///
    /// Returns EffectJobClaimLost for stale attempts, a bounded-error failure,
    /// EffectJobNotFound, or another storage error.
    pub fn retry_effect(
        &self,
        job_id: EffectJobId,
        attempt: u32,
        available_at: UnixTimeMillis,
        diagnostic: &str,
    ) -> StorageResult<()> {
        let connection = self.writer()?;
        outbox::retry(&connection, job_id, attempt, available_at, diagnostic)
    }

    /// Cancels a pending or running durable effect job.
    ///
    /// Cancellation is idempotent. Cancelling a running job invalidates its
    /// fencing token so a late completion cannot resurrect the job.
    ///
    /// # Errors
    ///
    /// Returns EffectJobNotFound for an unknown job or EffectJobTerminal when a
    /// completed job can no longer be cancelled.
    pub fn cancel_effect(&self, job_id: EffectJobId) -> StorageResult<bool> {
        let connection = self.writer()?;
        outbox::cancel(&connection, job_id)
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

fn ensure_world_format(found: u32) -> StorageResult<()> {
    if found == WORLD_FORMAT_VERSION {
        return Ok(());
    }
    Err(StorageError::UnsupportedWorldFormat {
        found,
        supported: WORLD_FORMAT_VERSION,
    })
}

fn read_world_metadata_full(connection: &Connection) -> StorageResult<(WorldId, u32, u64)> {
    let row = connection
        .query_row(
            "SELECT world_id, world_format_version, commit_position
             FROM world_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or(StorageError::Uninitialized)?;

    let world_id = world_id_from_blob(row.0)?;
    let world_format = u32::try_from(row.1)
        .map_err(|_| StorageError::CorruptData("invalid world format version".into()))?;
    let commit_position = u64::try_from(row.2)
        .map_err(|_| StorageError::CorruptData("negative world commit position".into()))?;
    Ok((world_id, world_format, commit_position))
}
