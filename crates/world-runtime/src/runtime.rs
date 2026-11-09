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

use rintawa_sdk::{
    types::ExtensionId,
    world::{CommandId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    DefinitionRegistration, PropertyDefinition, RuntimeSchemaDefinition, SchemaKind,
    SchemaRegistry, StateChange, WorldCommand, WorldError, WorldSessionState, WorldTransaction,
};

use crate::{SystemError, WorldRuntimeError, WorldRuntimeResult, WorldSnapshot, WorldSystem};

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
    /// Command schemas come from exact installed packages and are compiled in
    /// memory, independently of the durable world property vocabulary.
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
        let schemas = Arc::new(self.schemas);
        let session = Arc::new(Mutex::new(session));
        // Host-installed property declarations, not the on-disk JSON vocabulary,
        // determine the current extension-to-property authority.
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

        Ok(WorldRuntime {
            world_id,
            storage,
            session,
            property_owners,
            sender: Arc::new(Mutex::new(Some(sender))),
            worker: Some(worker),
        })
    }
}

/// Running single-writer command runtime for one authoritative world.
pub struct WorldRuntime {
    world_id: WorldId,
    storage: Arc<SqliteWorldStorage>,
    session: Arc<Mutex<WorldSessionState>>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionId>>>>,
    sender: Arc<Mutex<Option<SyncSender<CommandJob>>>>,
    worker: Option<JoinHandle<()>>,
}

/// Cloneable, lifecycle-revocable access to one active World runtime.
///
/// This handle shares the runtime's bounded single-writer queue and snapshots;
/// it never opens SQLite directly or creates another writer.
#[derive(Clone)]
pub struct WorldRuntimeClient {
    world_id: WorldId,
    storage: Arc<SqliteWorldStorage>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionId>>>>,
    sender: Arc<Mutex<Option<SyncSender<CommandJob>>>>,
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
            storage: self.storage.clone(),
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
    /// access. Conflicting owners for one key are rejected before any disk write.
    ///
    /// # Errors
    ///
    /// Returns a definition/owner conflict, unavailable runtime state or a
    /// storage error without partially publishing a batch.
    pub fn register_property_definitions(
        &self,
        definitions: &[PropertyDefinition],
    ) -> WorldRuntimeResult<Vec<DefinitionRegistration>> {
        let mut owners = self
            .property_owners
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
        let mut candidate_owners = (**owners).clone();
        for definition in definitions {
            if let Some(existing) =
                candidate_owners.insert(definition.key().clone(), definition.owner().clone())
                && existing != *definition.owner()
            {
                return Err(WorldError::DefinitionConflict {
                    key: definition.key().clone(),
                    existing_owner: existing.to_string(),
                    incoming_owner: definition.owner().to_string(),
                }
                .into());
            }
        }

        let mut session = self
            .session
            .lock()
            .map_err(|_| WorldRuntimeError::RuntimePoisoned)?;
        let mut candidate_session = session.clone();
        for definition in definitions {
            candidate_session.register_property(definition.clone())?;
        }

        let registrations = self.storage.register_definitions(definitions)?;
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
        match sender.try_send(CommandJob {
            command,
            response: response_sender,
        }) {
            Ok(()) => Ok(WorldCommandTicket {
                command_id,
                receiver: response_receiver,
            }),
            Err(TrySendError::Full(_)) => Err(WorldRuntimeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(WorldRuntimeError::RuntimeStopped),
        }
    }

    /// Captures an owner-scoped snapshot for a verified extension identity.
    pub fn snapshot_for_extension(&self, owner: ExtensionId) -> WorldRuntimeResult<WorldSnapshot> {
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
            self.storage.snapshot()?,
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
    schemas: Arc<SchemaRegistry>,
    session: Arc<Mutex<WorldSessionState>>,
    property_owners: Arc<Mutex<Arc<BTreeMap<SchemaKey, ExtensionId>>>>,
    receiver: Receiver<CommandJob>,
) {
    while let Ok(job) = receiver.recv() {
        let result = process_command(
            &storage,
            &systems,
            &schemas,
            &session,
            &property_owners,
            job.command,
        );
        let _ = job.response.send(result);
    }
}

fn process_command(
    storage: &Arc<SqliteWorldStorage>,
    systems: &BTreeMap<SchemaKey, RegisteredSystem>,
    schemas: &SchemaRegistry,
    session: &Mutex<WorldSessionState>,
    property_owners: &Mutex<Arc<BTreeMap<SchemaKey, ExtensionId>>>,
    command: WorldCommand,
) -> WorldRuntimeResult<WorldCommandOutcome> {
    schemas.validate_payload(command.schema(), SchemaKind::Command, command.payload())?;
    let registered = systems
        .get(command.schema())
        .ok_or_else(|| WorldRuntimeError::SystemUnavailable(command.schema().clone()))?;

    let owner = schemas
        .get(command.schema())
        .ok_or_else(|| WorldError::DefinitionNotRegistered {
            key: command.schema().clone(),
        })?
        .owner()
        .clone();
    let bindings = property_owners
        .lock()
        .map_err(|_| WorldRuntimeError::RuntimePoisoned)?
        .clone();
    let snapshot = WorldSnapshot::for_system(storage.snapshot()?, owner, bindings);
    let output = catch_unwind(AssertUnwindSafe(|| {
        registered.system.evaluate(&snapshot, &command)
    }))
    .map_err(|_| WorldRuntimeError::SystemPanicked(command.schema().clone()))?
    .map_err(|source| WorldRuntimeError::SystemFailed {
        schema: command.schema().clone(),
        source,
    })?;

    validate_output(command.schema(), session, &snapshot, &output)?;

    let command_id = command.id();
    storage.apply(&output)?;
    Ok(WorldCommandOutcome::new(command_id))
}

fn validate_output(
    schema: &SchemaKey,
    session: &Mutex<WorldSessionState>,
    snapshot: &WorldSnapshot,
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

    for change in transaction.changes() {
        let denied = match change {
            StateChange::SetProperty { key, .. } if !snapshot.permits_property(key) => {
                Some((key.clone(), "set"))
            }
            StateChange::RemoveProperty { key, .. } if !snapshot.permits_property(key) => {
                Some((key.clone(), "remove"))
            }
            StateChange::DeleteItem { id } => snapshot
                .foreign_property_on_delete(*id)
                .map_err(|error| WorldRuntimeError::SystemFailed {
                    schema: schema.clone(),
                    source: SystemError::Read(error),
                })?
                .map(|key| (key, "delete")),
            _ => None,
        };
        if let Some((key, operation)) = denied {
            return Err(WorldRuntimeError::InvalidTransaction {
                schema: schema.clone(),
                source: WorldError::PropertyAccessDenied { key, operation },
            });
        }
    }
    Ok(())
}
