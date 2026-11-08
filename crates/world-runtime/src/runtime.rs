//! Bounded single-writer command queue and worker lifecycle.

use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use rintawa_sdk::world::{CommandId, SchemaKey, WorldId};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    DefinitionRegistration, PropertyDefinition, RuntimeSchemaDefinition, RuntimeSignal, SchemaKind,
    SchemaRegistry, WorldCommand, WorldSessionState, WorldTransaction,
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
}

impl WorldRuntimePolicy {
    /// Creates a runtime policy.
    ///
    /// Validation occurs when the runtime starts.
    pub const fn new(command_queue_capacity: usize) -> Self {
        Self {
            command_queue_capacity,
        }
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
}

/// Builder that binds one storage boundary to its command Systems.
pub struct WorldRuntimeBuilder {
    storage: SqliteWorldStorage,
    policy: WorldRuntimePolicy,
    systems: BTreeMap<SchemaKey, RegisteredSystem>,
    schemas: SchemaRegistry,
}

impl WorldRuntimeBuilder {
    /// Creates a builder for one already-open persistent world.
    pub fn new(storage: SqliteWorldStorage) -> Self {
        Self {
            storage,
            policy: WorldRuntimePolicy::default(),
            systems: BTreeMap::new(),
            schemas: SchemaRegistry::new(),
        }
    }

    /// Replaces host-owned runtime resource policy.
    pub fn set_policy(&mut self, policy: WorldRuntimePolicy) {
        self.policy = policy;
    }

    /// Registers one transient extension runtime-API schema.
    ///
    /// Command and projection schemas come from exact installed packages; they are
    /// never persisted in the world property vocabulary.
    ///
    /// # Errors
    ///
    /// Returns a definition conflict or an invalid JSON Schema error.
    pub fn register_runtime_schema(
        &mut self,
        definition: RuntimeSchemaDefinition,
    ) -> WorldRuntimeResult<()> {
        self.schemas.register(definition)?;
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
        self.systems.insert(schema, RegisteredSystem { system });
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
        let systems = Arc::new(self.systems);
        let session = Arc::new(Mutex::new(session));
        let (sender, receiver) = mpsc::sync_channel(self.policy.command_queue_capacity());

        let worker_storage = Arc::clone(&storage);
        let worker_systems = Arc::clone(&systems);
        let worker_session = Arc::clone(&session);
        let worker = thread::Builder::new()
            .name(format!("rintawa-world-{world_id}"))
            .spawn(move || worker_loop(worker_storage, worker_systems, worker_session, receiver))
            .map_err(WorldRuntimeError::WorkerSpawn)?;

        Ok(WorldRuntime {
            world_id,
            storage,
            session,
            sender: Some(sender),
            worker: Some(worker),
        })
    }
}

/// Running single-writer command runtime for one authoritative world.
pub struct WorldRuntime {
    world_id: WorldId,
    storage: Arc<SqliteWorldStorage>,
    session: Arc<Mutex<WorldSessionState>>,
    sender: Option<SyncSender<CommandJob>>,
    worker: Option<JoinHandle<()>>,
}

impl WorldRuntime {
    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Captures a pinned read-side snapshot of current state.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the snapshot cannot be opened.
    pub fn snapshot(&self) -> WorldRuntimeResult<WorldSnapshot> {
        Ok(WorldSnapshot::new(self.storage.snapshot()?))
    }

    /// Persists property definitions from exact installed packages.
    ///
    /// # Errors
    ///
    /// Returns a conflict/validation error or a storage error.
    pub fn register_property_definitions(
        &self,
        definitions: &[PropertyDefinition],
    ) -> WorldRuntimeResult<Vec<DefinitionRegistration>> {
        let registrations = self.storage.register_definitions(definitions)?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
        for definition in definitions {
            session.register_property(definition.clone())?;
        }
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
            .as_ref()
            .ok_or(WorldRuntimeError::RuntimeStopped)?;
        let command_id = command.id();
        let (response_sender, response_receiver) = mpsc::channel();
        let job = CommandJob {
            command,
            response: response_sender,
        };

        match sender.try_send(job) {
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

    fn stop_worker(&mut self) -> WorldRuntimeResult<()> {
        self.sender.take();
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker.join().map_err(|_| WorldRuntimeError::WorkerPanicked)
    }
}

impl Drop for WorldRuntime {
    fn drop(&mut self) {
        let _ = self.stop_worker();
    }
}

/// Result of one committed command.
///
/// The change set is already durable when this value exists. Attached signals are
/// best-effort runtime notifications drained after COMMIT; they are never replayed.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldCommandOutcome {
    command_id: CommandId,
    signals: Vec<RuntimeSignal>,
}

impl WorldCommandOutcome {
    fn new(command_id: CommandId, signals: Vec<RuntimeSignal>) -> Self {
        Self {
            command_id,
            signals,
        }
    }

    /// Returns the command identity that produced this outcome.
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    /// Returns post-COMMIT best-effort signals produced by the command.
    pub fn signals(&self) -> &[RuntimeSignal] {
        &self.signals
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
}

struct CommandJob {
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
    session: Arc<Mutex<WorldSessionState>>,
    receiver: Receiver<CommandJob>,
) {
    while let Ok(job) = receiver.recv() {
        let result = process_command(&storage, &systems, &session, job.command);
        let _ = job.response.send(result);
    }
}

fn process_command(
    storage: &Arc<SqliteWorldStorage>,
    systems: &BTreeMap<SchemaKey, RegisteredSystem>,
    session: &Mutex<WorldSessionState>,
    command: WorldCommand,
) -> WorldRuntimeResult<WorldCommandOutcome> {
    let registered = systems
        .get(command.schema())
        .ok_or_else(|| WorldRuntimeError::SystemUnavailable(command.schema().clone()))?;

    let snapshot = WorldSnapshot::new(storage.snapshot()?);
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
    let signals = output.signals().to_vec();
    storage.apply(&output)?;
    Ok(WorldCommandOutcome::new(command_id, signals))
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
