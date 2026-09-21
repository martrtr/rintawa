//! Bounded single-writer command queue and worker lifecycle.

use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

use rintawa_sdk::world::{CommandId, SchemaKey, WorldId};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{CommitReceipt, SchemaKind, WorldCommand, WorldMutation, WorldTransaction};

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

/// Host-owned privileges granted to one registered World System.
///
/// These privileges are not package self-declarations. A future extension bridge
/// must derive them from host policy before registering a System.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorldSystemPrivileges {
    can_mutate_authority: bool,
}

impl WorldSystemPrivileges {
    /// Creates ordinary unprivileged System policy.
    pub const fn ordinary() -> Self {
        Self {
            can_mutate_authority: false,
        }
    }

    /// Creates policy for a trusted authority-management System.
    ///
    /// Only host policy should grant this privilege.
    pub const fn authority_manager() -> Self {
        Self {
            can_mutate_authority: true,
        }
    }

    /// Returns whether the System may create or revoke Core ControlGrants.
    pub const fn can_mutate_authority(self) -> bool {
        self.can_mutate_authority
    }
}

#[derive(Clone)]
struct RegisteredSystem {
    system: Arc<dyn WorldSystem>,
    privileges: WorldSystemPrivileges,
}

/// Builder that binds one storage boundary to its command Systems.
pub struct WorldRuntimeBuilder {
    storage: SqliteWorldStorage,
    policy: WorldRuntimePolicy,
    systems: BTreeMap<SchemaKey, RegisteredSystem>,
}

impl WorldRuntimeBuilder {
    /// Creates a builder for one already-open persistent world.
    pub fn new(storage: SqliteWorldStorage) -> Self {
        Self {
            storage,
            policy: WorldRuntimePolicy::default(),
            systems: BTreeMap::new(),
        }
    }

    /// Replaces host-owned runtime resource policy.
    pub fn set_policy(&mut self, policy: WorldRuntimePolicy) {
        self.policy = policy;
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
        self.register_system_with_privileges(system, WorldSystemPrivileges::ordinary())
    }

    /// Registers one System with explicit host-owned privileges.
    ///
    /// Privileges must come from host policy, never directly from a package
    /// declaration. Ordinary extension Systems should use register_system().
    ///
    /// # Errors
    ///
    /// Returns DuplicateSystem when another System already owns the schema.
    pub fn register_system_with_privileges<S>(
        &mut self,
        system: S,
        privileges: WorldSystemPrivileges,
    ) -> WorldRuntimeResult<()>
    where
        S: WorldSystem,
    {
        self.register_shared_system_with_privileges(Arc::new(system), privileges)
    }

    /// Registers one shared ordinary System implementation.
    ///
    /// # Errors
    ///
    /// Returns DuplicateSystem when another System already owns the schema.
    pub fn register_shared_system(
        &mut self,
        system: Arc<dyn WorldSystem>,
    ) -> WorldRuntimeResult<()> {
        self.register_shared_system_with_privileges(system, WorldSystemPrivileges::ordinary())
    }

    /// Registers one shared System with explicit host-owned privileges.
    ///
    /// # Errors
    ///
    /// Returns DuplicateSystem when another System already owns the schema.
    pub fn register_shared_system_with_privileges(
        &mut self,
        system: Arc<dyn WorldSystem>,
        privileges: WorldSystemPrivileges,
    ) -> WorldRuntimeResult<()> {
        let schema = system.command_schema().clone();
        if self.systems.contains_key(&schema) {
            return Err(WorldRuntimeError::DuplicateSystem(schema));
        }
        self.systems
            .insert(schema, RegisteredSystem { system, privileges });
        Ok(())
    }

    /// Starts one bounded worker for this authoritative world.
    ///
    /// # Errors
    ///
    /// Returns invalid policy, schema validation, storage, or thread-spawn errors.
    pub fn start(self) -> WorldRuntimeResult<WorldRuntime> {
        validate_policy(self.policy)?;

        let session = self.storage.load_session()?;
        for schema in self.systems.keys() {
            session
                .schemas()
                .require_kind(schema, SchemaKind::Command)?;
        }

        let world_id = self.storage.world_id();
        let storage = Arc::new(self.storage);
        let systems = Arc::new(self.systems);
        let (sender, receiver) = mpsc::sync_channel(self.policy.command_queue_capacity());

        let worker_storage = Arc::clone(&storage);
        let worker_systems = Arc::clone(&systems);
        let worker = thread::Builder::new()
            .name(format!("rintawa-world-{world_id}"))
            .spawn(move || worker_loop(worker_storage, worker_systems, receiver))
            .map_err(WorldRuntimeError::WorkerSpawn)?;

        Ok(WorldRuntime {
            world_id,
            storage,
            sender: Some(sender),
            worker: Some(worker),
        })
    }
}

/// Running single-writer command runtime for one authoritative world.
pub struct WorldRuntime {
    world_id: WorldId,
    storage: Arc<SqliteWorldStorage>,
    sender: Option<SyncSender<CommandJob>>,
    worker: Option<JoinHandle<()>>,
}

impl WorldRuntime {
    /// Returns the authoritative world identity.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Captures the current read-side world position.
    ///
    /// # Errors
    ///
    /// Returns a storage error when world metadata cannot be restored.
    pub fn snapshot(&self) -> WorldRuntimeResult<WorldSnapshot> {
        Ok(WorldSnapshot::new(self.storage.snapshot()?))
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

/// Handle for one queued command result.
pub struct WorldCommandTicket {
    command_id: CommandId,
    receiver: Receiver<WorldRuntimeResult<CommitReceipt>>,
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
    pub fn wait(self) -> WorldRuntimeResult<CommitReceipt> {
        self.receiver
            .recv()
            .map_err(|_| WorldRuntimeError::WorkerStopped)?
    }
}

struct CommandJob {
    command: WorldCommand,
    response: mpsc::Sender<WorldRuntimeResult<CommitReceipt>>,
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
    receiver: Receiver<CommandJob>,
) {
    while let Ok(job) = receiver.recv() {
        let result = process_command(&storage, &systems, job.command);
        let _ = job.response.send(result);
    }
}

fn process_command(
    storage: &Arc<SqliteWorldStorage>,
    systems: &BTreeMap<SchemaKey, RegisteredSystem>,
    command: WorldCommand,
) -> WorldRuntimeResult<CommitReceipt> {
    if let Some(receipt) = storage.committed_receipt(&command)? {
        return Ok(receipt);
    }

    let registered = systems
        .get(command.schema())
        .ok_or_else(|| WorldRuntimeError::SystemUnavailable(command.schema().clone()))?;

    let snapshot = WorldSnapshot::new(storage.snapshot_for_command(&command)?);
    let evaluated_position = snapshot.position();

    let output = catch_unwind(AssertUnwindSafe(|| {
        registered.system.evaluate(&snapshot, &command)
    }))
    .map_err(|_| WorldRuntimeError::SystemPanicked(command.schema().clone()))?
    .map_err(|source| WorldRuntimeError::SystemFailed {
        schema: command.schema().clone(),
        source,
    })?;

    reject_privileged_authority_output(command.schema(), &output, registered.privileges)?;

    Ok(storage.commit(&command, &output, evaluated_position)?)
}

fn reject_privileged_authority_output(
    schema: &SchemaKey,
    transaction: &WorldTransaction,
    privileges: WorldSystemPrivileges,
) -> WorldRuntimeResult<()> {
    if privileges.can_mutate_authority() {
        return Ok(());
    }

    let has_authority_mutation = transaction.mutations().iter().any(|mutation| {
        matches!(
            mutation,
            WorldMutation::GrantControl { .. } | WorldMutation::RevokeControl { .. }
        )
    });
    if has_authority_mutation {
        return Err(WorldRuntimeError::AuthorityMutationDenied(schema.clone()));
    }
    Ok(())
}
