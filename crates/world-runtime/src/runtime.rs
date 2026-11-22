//! Bounded single-writer command queue and worker lifecycle.

use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicUsize, Ordering},
    sync::{
        Arc, Mutex, Weak,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rintawa_sdk::{
    types::ExtensionInstanceId,
    world::{CommandId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    DefinitionRegistration, PropertyDefinition, RuntimeSchemaDefinition, SchemaKind,
    SchemaRegistry, WorldCommand, WorldError, WorldManifest, WorldSessionState, WorldTransaction,
};

use crate::{WorldRuntimeError, WorldRuntimeResult, WorldSnapshot, WorldSystem};

/// Default maximum number of commands waiting for one world worker.
pub const DEFAULT_COMMAND_QUEUE_CAPACITY: usize = 128;

/// Hard upper bound for one in-process world command queue.
pub const MAX_COMMAND_QUEUE_CAPACITY: usize = 4096;

/// Host-owned runtime resource policy for one active world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldRuntimePolicy {
    command_queue_capacity: usize,
    pending_command_bytes: usize,
}

impl WorldRuntimePolicy {
    /// Creates a runtime policy.
    ///
    /// Validation occurs when the runtime starts.
    pub const fn new(command_queue_capacity: usize) -> Self {
        Self {
            command_queue_capacity,
            pending_command_bytes: 8 * 1024 * 1024,
        }
    }

    /// Replaces the total serialized command budget, including evaluating work.
    pub const fn with_pending_command_bytes(mut self, maximum: usize) -> Self {
        self.pending_command_bytes = maximum;
        self
    }

    /// Returns the maximum number of commands waiting in the worker queue.
    pub const fn command_queue_capacity(self) -> usize {
        self.command_queue_capacity
    }
}

impl Default for WorldRuntimePolicy {
    fn default() -> Self {
        Self::new(DEFAULT_COMMAND_QUEUE_CAPACITY)
    }
}

#[derive(Clone)]
struct RegisteredSystem {
    system: Arc<dyn WorldSystem>,
    owner: ExtensionInstanceId,
}

/// Builder that binds one storage boundary to its command Systems.
pub struct WorldRuntimeBuilder {
    storage: SqliteWorldStorage,
    policy: WorldRuntimePolicy,
    systems: BTreeMap<SchemaKey, Arc<dyn WorldSystem>>,
    schemas: SchemaRegistry,
    schema_owners: BTreeMap<SchemaKey, ExtensionInstanceId>,
}

impl WorldRuntimeBuilder {
    /// Creates a builder for one already-open persistent world.
    pub fn new(storage: SqliteWorldStorage) -> Self {
        Self {
            storage,
            policy: WorldRuntimePolicy::default(),
            systems: BTreeMap::new(),
            schemas: SchemaRegistry::new(),
            schema_owners: BTreeMap::new(),
        }
    }

    /// Replaces host-owned runtime resource policy.
    pub fn set_policy(&mut self, policy: WorldRuntimePolicy) {
        self.policy = policy;
    }

    /// Registers one transient extension runtime-API schema.
    ///
    /// Command schemas come from exact installed packages and are compiled in
    /// memory, independently of the durable world property vocabulary. The
    /// separately supplied owner is the Host-attested instance, not the logical
    /// package ID carried by the schema metadata.
    ///
    /// # Errors
    ///
    /// Returns a definition conflict or an invalid JSON Schema error.
    pub fn register_runtime_schema(
        &mut self,
        owner: ExtensionInstanceId,
        definition: RuntimeSchemaDefinition,
    ) -> WorldRuntimeResult<()> {
        if let Some(existing) = self.schema_owners.get(definition.key())
            && existing != &owner
        {
            return Err(WorldError::DefinitionConflict {
                key: definition.key().clone(),
                existing_owner: existing.to_string(),
                incoming_owner: owner.to_string(),
            }
            .into());
        }
        let key = definition.key().clone();
        self.schemas.register(definition)?;
        self.schema_owners.insert(key, owner);
        Ok(())
    }

    /// Registers one System for one exact command schema.
    ///
    /// # Errors
    ///
    /// Returns DuplicateSystem when another System already owns the schema.
    pub fn register_system<S>(&mut self, system: S) -> WorldRuntimeResult<()>
    where
        S: WorldSystem,
    {
        self.register_shared_system(Arc::new(system))
    }

    /// Registers one shared System implementation.
    ///
    /// # Errors
    ///
    /// Returns DuplicateSystem when another System already owns the schema.
    pub fn register_shared_system(
        &mut self,
        system: Arc<dyn WorldSystem>,
    ) -> WorldRuntimeResult<()> {
        let schema = system.command_schema().clone();
        if self.systems.contains_key(&schema) {
            return Err(WorldRuntimeError::DuplicateSystem(schema));
        }
        self.systems.insert(schema, system);
        Ok(())
    }

    /// Starts one bounded worker for this authoritative world.
    ///
    /// # Errors
    ///
    /// Returns invalid policy, missing command schema, storage, or thread-spawn errors.
    pub fn start(self) -> WorldRuntimeResult<WorldRuntime> {
        validate_policy(self.policy)?;

        let session = self.storage.load_session()?;
        for schema in self.systems.keys() {
            self.schemas.require_kind(schema, SchemaKind::Command)?;
        }

        let world_id = self.storage.world_id();
        let storage = Arc::new(self.storage);
        let systems = Arc::new(
            self.systems
                .into_iter()
                .map(|(key, system)| {
                    let owner = self.schema_owners[&key].clone();
                    (key, RegisteredSystem { system, owner })
                })
                .collect(),
        );
        let schemas = Arc::new(self.schemas);
        let session = Arc::new(Mutex::new(session));
        // Host-installed property declarations, not the on-disk JSON vocabulary,
        // determine the current exact-instance property authority.
        let property_owners = Arc::new(Mutex::new(Arc::new(BTreeMap::new())));
        let (sender, receiver) = mpsc::sync_channel(self.policy.command_queue_capacity());

        let worker_storage = Arc::clone(&storage);
        let worker_systems = Arc::clone(&systems);
        let worker_schemas = Arc::clone(&schemas);
        let worker_session = Arc::clone(&session);
        let worker_property_owners = Arc::clone(&property_owners);
        let worker = thread::Builder::new()
            .name(format!("rintawa-world-{world_id}"))
            .spawn(move || {
                worker_loop(
                    worker_storage,
                    worker_systems,
                    worker_schemas,
                    worker_session,
                    worker_property_owners,
                    receiver,
                )
            })
            .map_err(WorldRuntimeError::WorkerSpawn)?;

        let pending = Arc::new(PendingCommands {
            used: AtomicUsize::new(0),
            maximum: self.policy.pending_command_bytes,
        });
        Ok(WorldRuntime {
            world_id,
            storage,
            session,
            property_owners,
            sender: Arc::new(Mutex::new(Some(sender))),
            worker: Some(worker),
            pending,
        })
    }
}

/// Running single-writer command runtime for one authoritative world.
pub struct WorldRuntime {
    world_id: WorldId,
    storage: Arc<SqliteWorldStorage>,
    session: Arc<Mutex<WorldSessionState>>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionInstanceId>>>>,
    sender: Arc<Mutex<Option<SyncSender<WorldJob>>>>,
    worker: Option<JoinHandle<()>>,
    pending: Arc<PendingCommands>,
}

/// Cloneable, lifecycle-revocable access to one active World runtime.
///
/// This handle shares the runtime's bounded single-writer queue and snapshots;
/// it never opens SQLite directly or creates another writer.
#[derive(Clone)]
pub struct WorldRuntimeClient {
    world_id: WorldId,
    storage: Weak<SqliteWorldStorage>,
    pending: Arc<PendingCommands>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionInstanceId>>>>,
    sender: Arc<Mutex<Option<SyncSender<WorldJob>>>>,
}

impl WorldRuntime {
    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns a revocable client sharing this runtime's authoritative queue.
    pub fn client(&self) -> WorldRuntimeClient {
        WorldRuntimeClient {
            world_id: self.world_id,
            storage: Arc::downgrade(&self.storage),
            pending: self.pending.clone(),
            property_owners: self.property_owners.clone(),
            sender: self.sender.clone(),
        }
    }

    /// Captures a trusted native Host snapshot of current state.
    ///
    /// This unrestricted API must never be exposed directly to an extension.
    /// Command Systems receive a separately owner-scoped snapshot.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the snapshot cannot be opened.
    pub fn snapshot(&self) -> WorldRuntimeResult<WorldSnapshot> {
        Ok(WorldSnapshot::new(self.storage.snapshot()?))
    }

    /// Persists Host-approved property definitions and binds their active owner.
    ///
    /// The SQLite vocabulary deliberately contains no owner field. On every
    /// activation Host rebinds identical stored definitions from exact installed
    /// artifacts. A persisted JSON Schema alone never grants a running System
    /// access. Conflicting owners for one property name are rejected before any disk write.
    ///
    /// # Errors
    ///
    /// Returns a definition/owner conflict, unavailable runtime state or a
    /// storage error without partially publishing a batch.
    pub fn register_property_definitions(
        &self,
        definitions: &[(ExtensionInstanceId, PropertyDefinition)],
    ) -> WorldRuntimeResult<Vec<DefinitionRegistration>> {
        let mut owners = self
            .property_owners
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
        let mut candidate_owners = (**owners).clone();
        // SQLite stores one value per (item, name), so every active version of
        // a name must share one exact owner. Validate the full batch first.
        let mut name_owners: BTreeMap<_, _> = candidate_owners
            .iter()
            .map(|(key, owner)| (key.id().clone(), owner.clone()))
            .collect();
        for (owner, definition) in definitions {
            if let Some(existing) = name_owners.insert(definition.key().id().clone(), owner.clone())
                && existing != *owner
            {
                return Err(WorldError::DefinitionConflict {
                    key: definition.key().clone(),
                    existing_owner: existing.to_string(),
                    incoming_owner: owner.to_string(),
                }
                .into());
            }
            candidate_owners.insert(definition.key().clone(), owner.clone());
        }

        let mut session = self
            .session
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
        let mut candidate_session = session.clone();
        for (_, definition) in definitions {
            candidate_session.register_property(definition.clone())?;
        }

        let definitions: Vec<_> = definitions
            .iter()
            .map(|(_, definition)| definition.clone())
            .collect();
        let registrations = self.storage.register_definitions(&definitions)?;
        *session = candidate_session;
        *owners = Arc::new(candidate_owners);
        Ok(registrations)
    }

    /// Enqueues one command without waiting for System execution or commit.
    ///
    /// # Errors
    ///
    /// Returns QueueFull when bounded capacity is exhausted, or RuntimeStopped
    /// after shutdown/worker disconnect.
    pub fn submit(&self, command: WorldCommand) -> WorldRuntimeResult<WorldCommandTicket> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .clone()
            .ok_or(WorldRuntimeError::RuntimeStopped)?;
        let command_id = command.id();
        let (response_sender, response_receiver) = mpsc::channel();
        let reservation = PendingReservation::new(self.pending.clone(), &command)?;
        let job = CommandJob {
            reservation,
            command,
            response: response_sender,
        };

        match sender.try_send(WorldJob::Command(job)) {
            Ok(()) => Ok(WorldCommandTicket {
                command_id,
                receiver: response_receiver,
            }),
            Err(TrySendError::Full(_)) => Err(WorldRuntimeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(WorldRuntimeError::RuntimeStopped),
        }
    }

    /// Stops accepting commands, drains the queued work, and joins the worker.
    ///
    /// # Errors
    ///
    /// Returns WorkerPanicked when an internal runtime panic escaped the worker.
    pub fn shutdown(mut self) -> WorldRuntimeResult<()> {
        self.stop_worker()
    }

    /// Closes queue admission while retaining Systems and writer for graceful drain.
    pub fn close_admission(&self) -> WorldRuntimeResult<()> {
        self.sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .take();
        Ok(())
    }

    /// Closes admission and waits at most `timeout` for graceful drain.
    /// A busy result retains the worker and writer for a later retry.
    pub fn shutdown_timeout(&mut self, timeout: Duration) -> WorldRuntimeResult<()> {
        self.close_admission()?;
        let deadline = Instant::now() + timeout;
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if Instant::now() >= deadline {
                return Err(WorldRuntimeError::ShutdownBusy);
            }
            thread::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        self.stop_worker()
    }

    fn stop_worker(&mut self) -> WorldRuntimeResult<()> {
        self.sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .take();
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker.join().map_err(|_| WorldRuntimeError::WorkerPanicked)
    }
}

impl WorldRuntimeClient {
    /// Returns the bound World identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Enqueues one command without waiting for its commit.
    pub fn submit(&self, command: WorldCommand) -> WorldRuntimeResult<WorldCommandTicket> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .clone()
            .ok_or(WorldRuntimeError::RuntimeStopped)?;
        let command_id = command.id();
        let (response_sender, response_receiver) = mpsc::channel();
        let reservation = PendingReservation::new(self.pending.clone(), &command)?;
        match sender.try_send(WorldJob::Command(CommandJob {
            reservation,
            command,
            response: response_sender,
        })) {
            Ok(()) => Ok(WorldCommandTicket {
                command_id,
                receiver: response_receiver,
            }),
            Err(TrySendError::Full(_)) => Err(WorldRuntimeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(WorldRuntimeError::RuntimeStopped),
        }
    }

    /// Enqueues a Host-only metadata replacement and waits for COMMIT.
    /// Uses the same bounded writer queue as domain commands.
    pub fn set_manifest_metadata(
        &self,
        metadata: serde_json::Value,
    ) -> WorldRuntimeResult<WorldManifest> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .clone()
            .ok_or(WorldRuntimeError::RuntimeStopped)?;
        let (response, receiver) = mpsc::channel();
        match sender.try_send(WorldJob::Metadata { metadata, response }) {
            Ok(()) => receiver
                .recv()
                .map_err(|_| WorldRuntimeError::WorkerStopped)?,
            Err(TrySendError::Full(_)) => Err(WorldRuntimeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(WorldRuntimeError::RuntimeStopped),
        }
    }

    /// Captures an owner-scoped snapshot for a verified extension identity.
    pub fn snapshot_for_instance(
        &self,
        owner: ExtensionInstanceId,
    ) -> WorldRuntimeResult<WorldSnapshot> {
        if self
            .sender
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .is_none()
        {
            return Err(WorldRuntimeError::RuntimeStopped);
        }
        let bindings = self
            .property_owners
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
            .clone();
        Ok(WorldSnapshot::for_system(
            self.storage
                .upgrade()
                .ok_or(WorldRuntimeError::RuntimeStopped)?
                .snapshot()?,
            owner,
            bindings,
        ))
    }
}

impl Drop for WorldRuntime {
    fn drop(&mut self) {
        let _ = self.stop_worker();
    }
}

/// Result of one committed command.
///
/// The change set is already durable when this value exists.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldCommandOutcome {
    command_id: CommandId,
}

impl WorldCommandOutcome {
    const fn new(command_id: CommandId) -> Self {
        Self { command_id }
    }

    /// Returns the command identity that produced this outcome.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }
}

/// Handle for one queued command result.
pub struct WorldCommandTicket {
    command_id: CommandId,
    receiver: Receiver<WorldRuntimeResult<WorldCommandOutcome>>,
}

impl WorldCommandTicket {
    /// Returns the command identity represented by this ticket.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Waits for command evaluation and authoritative commit.
    ///
    /// # Errors
    ///
    /// Returns the command/runtime error, or WorkerStopped if the worker ended
    /// before producing a result.
    pub fn wait(self) -> WorldRuntimeResult<WorldCommandOutcome> {
        self.receiver
            .recv()
            .map_err(|_| WorldRuntimeError::WorkerStopped)?
    }

    /// Returns a completed result without blocking, or `None` while queued.
    pub fn try_wait(&self) -> WorldRuntimeResult<Option<WorldCommandOutcome>> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(WorldRuntimeError::WorkerStopped),
        }
    }
}

enum WorldJob {
    Command(CommandJob),
    Metadata {
        metadata: serde_json::Value,
        response: mpsc::Sender<WorldRuntimeResult<WorldManifest>>,
    },
}

struct PendingCommands {
    used: AtomicUsize,
    maximum: usize,
}
struct PendingReservation {
    pending: Arc<PendingCommands>,
    bytes: usize,
}
impl PendingReservation {
    fn new(pending: Arc<PendingCommands>, command: &WorldCommand) -> WorldRuntimeResult<Self> {
        // Count the complete serialized envelope without retaining a second buffer.
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len())
                    .ok_or_else(|| std::io::Error::other("command size overflow"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, command)
            .map_err(|_| WorldRuntimeError::PendingCommandBytesExceeded)?;
        let bytes = counter.0;
        pending
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= pending.maximum)
            })
            .map_err(|_| WorldRuntimeError::PendingCommandBytesExceeded)?;
        Ok(Self { pending, bytes })
    }
}
impl Drop for PendingReservation {
    fn drop(&mut self) {
        self.pending.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct CommandJob {
    reservation: PendingReservation,
    command: WorldCommand,
    response: mpsc::Sender<WorldRuntimeResult<WorldCommandOutcome>>,
}

fn validate_policy(policy: WorldRuntimePolicy) -> WorldRuntimeResult<()> {
    let capacity = policy.command_queue_capacity();
    if capacity == 0 || capacity > MAX_COMMAND_QUEUE_CAPACITY {
        return Err(WorldRuntimeError::InvalidQueueCapacity {
            capacity,
            maximum: MAX_COMMAND_QUEUE_CAPACITY,
        });
    }
    Ok(())
}

fn worker_loop(
    storage: Arc<SqliteWorldStorage>,
    systems: Arc<BTreeMap<SchemaKey, RegisteredSystem>>,
    schemas: Arc<SchemaRegistry>,
    session: Arc<Mutex<WorldSessionState>>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionInstanceId>>>>,
    receiver: Receiver<WorldJob>,
) {
    while let Ok(job) = receiver.recv() {
        match job {
            WorldJob::Command(job) => {
                let result = process_command(
                    &storage,
                    &systems,
                    &schemas,
                    &session,
                    &property_owners,
                    job.command,
                );
                drop(job.reservation);
                let _ = job.response.send(result);
            }
            WorldJob::Metadata { metadata, response } => {
                let result = storage
                    .set_manifest_metadata(metadata)
                    .map_err(WorldRuntimeError::Storage);
                let _ = response.send(result);
            }
        }
    }
}

fn process_command(
    storage: &Arc<SqliteWorldStorage>,
    systems: &BTreeMap<SchemaKey, RegisteredSystem>,
    schemas: &SchemaRegistry,
    session: &Mutex<WorldSessionState>,
    property_owners: &Mutex<Arc<BTreeMap<SchemaKey, ExtensionInstanceId>>>,
    command: WorldCommand,
) -> WorldRuntimeResult<WorldCommandOutcome> {
    schemas.validate_payload(command.schema(), SchemaKind::Command, command.payload())?;
    let registered = systems
        .get(command.schema())
        .ok_or_else(|| WorldRuntimeError::SystemUnavailable(command.schema().clone()))?;

    let bindings = property_owners
        .lock()
        .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
        .clone();
    let snapshot =
        WorldSnapshot::for_system(storage.snapshot()?, registered.owner.clone(), bindings);
    let output = catch_unwind(AssertUnwindSafe(|| {
        registered.system.evaluate(&snapshot, &command)
    }))
    .map_err(|_| WorldRuntimeError::SystemPanicked(command.schema().clone()))?
    .map_err(|source| WorldRuntimeError::SystemFailed {
        schema: command.schema().clone(),
        source,
    })?;

    validate_output(command.schema(), session, &output)?;

    let command_id = command.id();
    storage
        .apply_with_property_access(&output, |key| snapshot.permits_property(key))
        .map_err(|error| match error {
            rintawa_storage::StorageError::World(
                source @ WorldError::PropertyAccessDenied { .. },
            ) => WorldRuntimeError::InvalidTransaction {
                schema: command.schema().clone(),
                source,
            },
            other => WorldRuntimeError::Storage(other),
        })?;
    Ok(WorldCommandOutcome::new(command_id))
}

fn validate_output(
    schema: &SchemaKey,
    session: &Mutex<WorldSessionState>,
    transaction: &WorldTransaction,
) -> WorldRuntimeResult<()> {
    let session = session
        .lock()
        .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
    transaction
        .validate(session.properties())
        .map_err(|source| WorldRuntimeError::InvalidTransaction {
            schema: schema.clone(),
            source,
        })?;

    Ok(())
}
