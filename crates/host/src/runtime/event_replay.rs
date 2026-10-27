//! Host-owned, bounded catch-up from committed WorldEvents.
//!
//! A trusted policy chooses which source events produce enqueue commands and
//! which controller may submit them. An extension cannot supply a Principal or
//! acknowledge another consumer's checkpoint through this module.
//!
//! The enqueue command must be idempotent and its System must apply ordinary
//! World authorization. Actual AI inference and effect claim fencing remain in
//! the existing World effect worker; this module never calls an AI provider.

use sha2::{Digest, Sha256};

use rintawa_sdk::world::{CommandId, PrincipalId, SchemaKey, WorldId};
use rintawa_world::{ActorRef, CausationRef, CommitDisposition, StoredWorldEvent, WorldCommand};

use crate::runtime::HostRuntime;
use crate::{HostError, HostResult};

/// Maximum committed WorldEvents examined by one cooperative replay pump.
pub const MAX_WORLD_EVENT_REPLAY_BATCH: usize = 16;
const MAX_REPLAY_RULE_ID_BYTES: usize = 96;
const ENQUEUE_COMMAND_DOMAIN: &[u8] = b"rintawa.replay.enqueue.command.v1\0";
const CONSUMER_DOMAIN: &[u8] = b"rintawa.replay.consumer.v1\0";

/// A Host-approved and pinned event-to-command routing rule.
///
/// Construction must happen in trusted composition policy. This binding does
/// not mint a controller Principal or ControlGrant; the ordinary World Runtime
/// authorizes the final command for the exact actor and schema on each commit.
#[derive(Debug, Clone)]
pub struct TrustedWorldEventConsumer {
    world_id: WorldId,
    storage_consumer_id: String,
    principal: PrincipalId,
    actor: ActorRef,
    enqueue_schema: SchemaKey,
    batch_limit: usize,
}

impl TrustedWorldEventConsumer {
    /// Binds an exact World, versioned Host rule ID, controller, actor and
    /// command schema to a stable, private checkpoint namespace.
    ///
    /// The caller must authenticate and authorize the controller using Host
    /// policy. The constructor is *not* a way for guests to grant control.
    ///
    /// # Errors
    ///
    /// Refuses malformed rule IDs, invalid batch limits, and an actor claiming
    /// to be a different Principal.
    pub fn new(
        world_id: WorldId,
        rule_id: &str,
        principal: PrincipalId,
        actor: ActorRef,
        enqueue_schema: SchemaKey,
        batch_limit: usize,
    ) -> HostResult<Self> {
        if rule_id.is_empty()
            || rule_id.len() > MAX_REPLAY_RULE_ID_BYTES
            || !rule_id.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_' | b'/' | b'@')
            })
            || matches!(actor, ActorRef::Principal(id) if id != principal)
            || batch_limit == 0
            || batch_limit > MAX_WORLD_EVENT_REPLAY_BATCH
        {
            return Err(HostError::InvalidWorldEventReplayPolicy);
        }
        let mut digest = Sha256::new();
        digest.update(CONSUMER_DOMAIN);
        digest.update(world_id.into_bytes());
        digest.update(principal.into_bytes());
        digest.update(actor_identity(actor));
        let schema_text = enqueue_schema.to_string();
        digest.update((schema_text.len() as u16).to_be_bytes());
        digest.update(schema_text.as_bytes());
        digest.update((rule_id.len() as u16).to_be_bytes());
        digest.update(rule_id.as_bytes());
        let key: [u8; 32] = digest.finalize().into();
        let mut consumer_id = String::from("rintawa.replay.v1:");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in key {
            consumer_id.push(char::from(HEX[usize::from(byte >> 4)]));
            consumer_id.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Ok(Self {
            world_id,
            storage_consumer_id: consumer_id,
            principal,
            actor,
            enqueue_schema,
            batch_limit,
        })
    }

    /// Opaque storage-only checkpoint namespace owned by the bound Host rule.
    pub fn storage_consumer_id(&self) -> &str {
        &self.storage_consumer_id
    }

    /// Exact World whose committed events may be consumed.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Maximum events processed in a single cooperative call.
    pub const fn batch_limit(&self) -> usize {
        self.batch_limit
    }

    /// Stable command identity for one source event in this bound consumer.
    ///
    /// Re-running a rule after a crash produces the same CommandId, and a
    /// different controller or versioned rule receives a distinct identity.
    pub fn enqueue_command_id(&self, event: &StoredWorldEvent) -> CommandId {
        let mut digest = Sha256::new();
        digest.update(ENQUEUE_COMMAND_DOMAIN);
        digest.update(self.storage_consumer_id.as_bytes());
        digest.update(self.world_id.into_bytes());
        digest.update(event.id.into_bytes());
        let bytes: [u8; 32] = digest.finalize().into();
        let mut command_id = [0_u8; 16];
        command_id.copy_from_slice(&bytes[..16]);
        CommandId::from_bytes(command_id)
    }

    fn validate_command(
        &self,
        event: &StoredWorldEvent,
        expected_command_id: CommandId,
        command: &WorldCommand,
    ) -> HostResult<()> {
        if command.id() != expected_command_id
            || command.principal() != self.principal
            || command.actor() != self.actor
            || command.schema() != &self.enqueue_schema
            || command.causation() != Some(CausationRef::Event(event.id))
            || command.correlation_id() != event.provenance.correlation_id
            || command.expected_position().is_some()
            || command.effective_at_timestamp().is_some()
        {
            return Err(HostError::InvalidWorldEventReplayCommand);
        }
        Ok(())
    }
}

fn actor_identity(actor: ActorRef) -> [u8; 17] {
    let mut value = [0_u8; 17];
    match actor {
        ActorRef::Principal(principal) => {
            value[0] = 0;
            value[1..].copy_from_slice(&principal.into_bytes());
        }
        ActorRef::Entity(entity) => {
            value[0] = 1;
            value[1..].copy_from_slice(&entity.into_bytes());
        }
    }
    value
}

/// Trusted routing decision for a stored WorldEvent.
///
/// Skipping a source event means its checkpoint advances; only a trusted Host
/// policy may return `Skip`, including when the event is irrelevant to a rule.
/// A concrete extension cannot call the public Host method through guest WIT.
#[derive(Debug)]
pub enum WorldEventReplayDecision {
    /// Ignore this event under the bound deterministic Host routing policy.
    Skip,
    /// Commit this command before acknowledging the source event.
    Enqueue(WorldCommand),
}

impl HostRuntime {
    /// Replays a bounded page of exact committed events into an authorized
    /// enqueue WorldCommand, then checkpoints only successfully handled events.
    ///
    /// The supplied callback is **trusted Host code**, not a guest service.
    /// Its output cannot change the controller, target schema, causation or
    /// stable CommandId. The World System still checks actor grants at commit.
    /// A failed authoritative command leaves the checkpoint untouched. Failure
    /// to deliver an ephemeral event callback after a committed enqueue does
    /// not turn that durable commit into a failure.
    ///
    /// The callback must be deterministic for the same stored event. A replay
    /// with a changed payload and unchanged CommandId is rejected by storage's
    /// committed command digest validation rather than silently re-enqueued.
    ///
    /// # Errors
    ///
    /// Returns an inactive-World, policy, command, storage or authoritative
    /// World Runtime error without acknowledging the failing source event.
    pub fn pump_world_event_consumer<F>(
        &mut self,
        consumer: &TrustedWorldEventConsumer,
        mut route: F,
    ) -> HostResult<usize>
    where
        F: FnMut(&StoredWorldEvent, CommandId) -> HostResult<WorldEventReplayDecision>,
    {
        let world_id = consumer.world_id;
        let (mut cursor, events) = {
            let active = self
                .active_worlds
                .get(&world_id)
                .ok_or(HostError::WorldNotActive(world_id))?;
            let cursor = active
                .outbox
                .event_consumer_cursor(consumer.storage_consumer_id())?;
            let events = active
                .outbox
                .events_after_cursor(cursor, consumer.batch_limit)?;
            (cursor, events)
        };

        let mut acknowledged = 0_usize;
        for event in events {
            if event.world_id != world_id {
                return Err(HostError::MixedWorldEventBatch);
            }
            let command_id = consumer.enqueue_command_id(&event);
            match route(&event, command_id)? {
                WorldEventReplayDecision::Skip => {}
                WorldEventReplayDecision::Enqueue(command) => {
                    consumer.validate_command(&event, command_id, &command)?;
                    let dispatched = self.submit_world_command(world_id, command)?;
                    match dispatched.authoritative().receipt().disposition() {
                        CommitDisposition::Committed | CommitDisposition::AlreadyCommitted => {}
                    }
                    // Ignore `live_delivery` here. Its error concerns only an
                    // ephemeral callback, never the durable authoritative receipt.
                }
            }
            cursor = Some(
                self.active_worlds
                    .get(&world_id)
                    .ok_or(HostError::WorldNotActive(world_id))?
                    .outbox
                    .acknowledge_next_world_event(consumer.storage_consumer_id(), cursor, &event)?,
            );
            acknowledged += 1;
        }
        Ok(acknowledged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::world::UnixTimeMillis;
    use rintawa_sdk::world::{CorrelationId, WorldEventId};
    use rintawa_world::CommandProvenance;

    fn event(world_id: WorldId, principal: PrincipalId) -> StoredWorldEvent {
        let schema: SchemaKey = "rintawa.test.replay-event@1"
            .parse()
            .expect("test schema must be valid");
        StoredWorldEvent {
            world_id,
            id: WorldEventId::new(),
            commit_position: 3,
            event_index: 0,
            provenance: CommandProvenance {
                command_id: CommandId::new(),
                command_schema: schema.clone(),
                principal,
                actor: ActorRef::Principal(principal),
                causation: None,
                correlation_id: CorrelationId::new(),
                effective_at: None,
                admission: rintawa_world::CommandAdmission::Unknown,
                recorded_at: UnixTimeMillis::new(1),
            },
            schema,
            payload: serde_json::json!({"source": "event"}),
        }
    }

    fn policy(world_id: WorldId, principal: PrincipalId, rule: &str) -> TrustedWorldEventConsumer {
        TrustedWorldEventConsumer::new(
            world_id,
            rule,
            principal,
            ActorRef::Principal(principal),
            "rintawa.test.replay-enqueue@1"
                .parse()
                .expect("test schema must be valid"),
            2,
        )
        .expect("valid Host policy")
    }

    fn enqueue(consumer: &TrustedWorldEventConsumer, source: &StoredWorldEvent) -> WorldCommand {
        WorldCommand::with_ids(
            consumer.enqueue_command_id(source),
            source.provenance.correlation_id,
            consumer.enqueue_schema.clone(),
            consumer.principal,
            consumer.actor,
            serde_json::json!({"source-event-id": source.id}),
        )
        .caused_by(CausationRef::Event(source.id))
    }

    #[test]
    fn test_should_bind_replay_identity_to_world_rule_and_controller() {
        let world = WorldId::new();
        let principal = PrincipalId::new();
        let source = event(world, principal);
        let selected = policy(world, principal, "rintawa.test.reaction-rule@1");
        assert_eq!(
            selected.enqueue_command_id(&source),
            selected.enqueue_command_id(&source)
        );
        assert_ne!(
            selected.enqueue_command_id(&source),
            policy(world, principal, "rintawa.test.reaction-rule@2").enqueue_command_id(&source)
        );
        assert_ne!(
            selected.enqueue_command_id(&source),
            policy(world, PrincipalId::new(), "rintawa.test.reaction-rule@1")
                .enqueue_command_id(&source)
        );
        assert_ne!(
            selected.storage_consumer_id(),
            policy(WorldId::new(), principal, "rintawa.test.reaction-rule@1").storage_consumer_id()
        );
    }

    #[test]
    fn test_should_reject_forged_controller_schema_causation_and_id() {
        let world = WorldId::new();
        let principal = PrincipalId::new();
        let selected = policy(world, principal, "test.replay@1");
        let source = event(world, principal);
        let command_id = selected.enqueue_command_id(&source);
        assert!(
            selected
                .validate_command(&source, command_id, &enqueue(&selected, &source))
                .is_ok()
        );
        let forged = WorldCommand::with_ids(
            command_id,
            source.provenance.correlation_id,
            selected.enqueue_schema.clone(),
            PrincipalId::new(),
            selected.actor,
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Event(source.id));
        assert!(matches!(
            selected.validate_command(&source, command_id, &forged),
            Err(HostError::InvalidWorldEventReplayCommand)
        ));
        let wrong_id = WorldCommand::with_ids(
            CommandId::new(),
            source.provenance.correlation_id,
            selected.enqueue_schema.clone(),
            selected.principal,
            selected.actor,
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Event(source.id));
        assert!(
            selected
                .validate_command(&source, command_id, &wrong_id)
                .is_err()
        );
        let wrong_causation = WorldCommand::with_ids(
            command_id,
            source.provenance.correlation_id,
            selected.enqueue_schema.clone(),
            selected.principal,
            selected.actor,
            serde_json::json!({}),
        );
        assert!(
            selected
                .validate_command(&source, command_id, &wrong_causation)
                .is_err()
        );
    }

    #[test]
    fn test_should_refuse_invalid_rule_and_batch_before_touching_storage() {
        let world = WorldId::new();
        let principal = PrincipalId::new();
        let schema: SchemaKey = "rintawa.test.enqueue@1"
            .parse()
            .expect("test schema must be valid");
        for invalid_rule in ["", "BadCaps", "bad space", "é"] {
            assert!(matches!(
                TrustedWorldEventConsumer::new(
                    world,
                    invalid_rule,
                    principal,
                    ActorRef::Principal(principal),
                    schema.clone(),
                    1
                ),
                Err(HostError::InvalidWorldEventReplayPolicy)
            ));
        }
        assert!(matches!(
            TrustedWorldEventConsumer::new(
                world,
                "valid.rule@1",
                principal,
                ActorRef::Principal(PrincipalId::new()),
                schema.clone(),
                1
            ),
            Err(HostError::InvalidWorldEventReplayPolicy)
        ));
        assert!(matches!(
            TrustedWorldEventConsumer::new(
                world,
                "valid.rule@1",
                principal,
                ActorRef::Principal(principal),
                schema,
                MAX_WORLD_EVENT_REPLAY_BATCH + 1
            ),
            Err(HostError::InvalidWorldEventReplayPolicy)
        ));
    }
}

#[cfg(test)]
mod sqlite_integration_tests {
    use std::collections::BTreeMap;

    use rintawa_extension_engine::ExtensionEngine;
    use rintawa_sdk::{
        types::ExtensionId,
        world::{EffectJobId, EntityId},
    };
    use rintawa_storage::SqliteWorldStorage;
    use rintawa_world::{
        EffectJobDraft, SchemaDefinition, SchemaKind, WorldEventDraft, WorldTransaction,
    };
    use rintawa_world_runtime::{SystemResult, WorldRuntimeBuilder, WorldSnapshot, WorldSystem};

    use super::*;
    use crate::runtime::{ActiveWorld, RuntimeSignalQueue};

    struct SourceSystem {
        command_schema: SchemaKey,
        event_schema: SchemaKey,
    }

    impl WorldSystem for SourceSystem {
        fn command_schema(&self) -> &SchemaKey {
            &self.command_schema
        }

        fn evaluate(
            &self,
            _snapshot: &WorldSnapshot,
            _command: &WorldCommand,
        ) -> SystemResult<WorldTransaction> {
            let mut transaction = WorldTransaction::new();
            for ordinal in 0..2 {
                transaction.push_event(WorldEventDraft::new(
                    self.event_schema.clone(),
                    serde_json::json!({"ordinal": ordinal}),
                ));
            }
            Ok(transaction)
        }
    }

    struct EnqueueSystem {
        command_schema: SchemaKey,
        effect_schema: SchemaKey,
    }

    impl WorldSystem for EnqueueSystem {
        fn command_schema(&self) -> &SchemaKey {
            &self.command_schema
        }

        fn evaluate(
            &self,
            _snapshot: &WorldSnapshot,
            command: &WorldCommand,
        ) -> SystemResult<WorldTransaction> {
            let mut transaction = WorldTransaction::new();
            // The future effect completion uses CommandId(effect_id) and
            // therefore MUST NOT collide with this enqueue command's ID.
            let mut digest = Sha256::new();
            digest.update(b"rintawa.test.replay.effect.v1\0");
            digest.update(command.id().into_bytes());
            let hash: [u8; 32] = digest.finalize().into();
            let mut effect_bytes = [0_u8; 16];
            effect_bytes.copy_from_slice(&hash[..16]);
            transaction.push_effect(EffectJobDraft::with_id(
                EffectJobId::from_bytes(effect_bytes),
                self.effect_schema.clone(),
                command.payload().clone(),
            ));
            Ok(transaction)
        }
    }

    fn fixture(
        root: &std::path::Path,
        world_id: WorldId,
    ) -> anyhow::Result<(HostRuntime, SchemaKey, SchemaKey)> {
        let path = root.join("world.sqlite");
        let existing_world = path.exists();
        let storage = if existing_world {
            SqliteWorldStorage::open(&path)?
        } else {
            SqliteWorldStorage::create(&path, world_id)?
        };
        let source_schema: SchemaKey = "rintawa.test.replay-source@1".parse()?;
        let event_schema: SchemaKey = "rintawa.test.replay-source-event@1".parse()?;
        let enqueue_schema: SchemaKey = "rintawa.test.replay-effect-enqueue@1".parse()?;
        let effect_schema: SchemaKey = "rintawa.test.replay-durable-effect@1".parse()?;
        let owner = ExtensionId::new("rintawa.test.replay");
        if !existing_world {
            for (schema, kind) in [
                (&source_schema, SchemaKind::Command),
                (&event_schema, SchemaKind::Event),
                (&enqueue_schema, SchemaKind::Command),
                (&effect_schema, SchemaKind::Effect),
            ] {
                storage.register_schema(&SchemaDefinition::new(
                    schema.clone(),
                    kind,
                    owner.clone(),
                    serde_json::json!({"type": "object"}),
                ))?;
            }
        }
        let mut builder = WorldRuntimeBuilder::new(storage);
        builder.register_system(SourceSystem {
            command_schema: source_schema.clone(),
            event_schema: event_schema.clone(),
        })?;
        builder.register_system(EnqueueSystem {
            command_schema: enqueue_schema.clone(),
            effect_schema,
        })?;
        let world_runtime = builder.start()?;
        let outbox = SqliteWorldStorage::open(&path)?;
        let host = HostRuntime {
            active_worlds: BTreeMap::from([(
                world_id,
                ActiveWorld {
                    deferred_effects: BTreeMap::new(),
                    runtime: world_runtime,
                    outbox,
                    effect_handlers: BTreeMap::new(),
                    projections: BTreeMap::new(),
                    signals: RuntimeSignalQueue::default(),
                    registered_instances: Vec::new(),
                    started_instances: Vec::new(),
                },
            )]),
            engine: ExtensionEngine::new(),
            started_instances: Vec::new(),
            host_shell_provider: None,
            ui_layer_provider: None,
            host_access: None,
        };
        Ok((host, source_schema, enqueue_schema))
    }

    fn policy_for(
        world: WorldId,
        principal: PrincipalId,
        schema: SchemaKey,
        actor: ActorRef,
    ) -> TrustedWorldEventConsumer {
        TrustedWorldEventConsumer::new(world, "rintawa.test.replay@1", principal, actor, schema, 1)
            .expect("valid trusted replay policy")
    }

    fn enqueue(
        policy: &TrustedWorldEventConsumer,
        event: &StoredWorldEvent,
        id: CommandId,
    ) -> WorldEventReplayDecision {
        WorldEventReplayDecision::Enqueue(
            WorldCommand::with_ids(
                id,
                event.provenance.correlation_id,
                policy.enqueue_schema.clone(),
                policy.principal,
                policy.actor,
                serde_json::json!({"source": event.id}),
            )
            .caused_by(CausationRef::Event(event.id)),
        )
    }

    fn send_source(
        host: &mut HostRuntime,
        world: WorldId,
        source_schema: SchemaKey,
        principal: PrincipalId,
    ) -> anyhow::Result<()> {
        let command = WorldCommand::new(
            source_schema,
            principal,
            ActorRef::Principal(principal),
            serde_json::json!({"send": true}),
        );
        let result = host.submit_world_command(world, command)?;
        assert_eq!(
            result.authoritative().receipt().disposition(),
            CommitDisposition::Committed
        );
        Ok(())
    }

    #[test]
    fn test_should_resume_multievent_commit_without_skips_after_restart() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let world = WorldId::new();
        let controller = PrincipalId::new();
        let human = PrincipalId::new();
        let (mut host, source_schema, enqueue_schema) = fixture(dir.path(), world)?;
        send_source(&mut host, world, source_schema, human)?;
        let policy = policy_for(
            world,
            controller,
            enqueue_schema,
            ActorRef::Principal(controller),
        );
        assert_eq!(
            host.pump_world_event_consumer(&policy, |event, id| Ok(enqueue(&policy, event, id)))?,
            1
        );
        assert_eq!(
            host.active_worlds[&world].outbox.pending_effects(8)?.len(),
            1
        );
        drop(host);

        let (mut resumed, _, _) = fixture(dir.path(), world)?;
        assert_eq!(
            resumed.pump_world_event_consumer(&policy, |event, id| {
                Ok(enqueue(&policy, event, id))
            })?,
            1
        );
        assert_eq!(
            resumed.active_worlds[&world]
                .outbox
                .pending_effects(8)?
                .len(),
            2
        );
        assert_eq!(
            resumed.pump_world_event_consumer(&policy, |_event, _id| {
                Ok(WorldEventReplayDecision::Skip)
            })?,
            0
        );
        Ok(())
    }

    #[test]
    fn test_should_idempotently_replay_after_effect_commit_before_ack() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let world = WorldId::new();
        let controller = PrincipalId::new();
        let (mut host, source_schema, enqueue_schema) = fixture(dir.path(), world)?;
        send_source(&mut host, world, source_schema, PrincipalId::new())?;
        let policy = policy_for(
            world,
            controller,
            enqueue_schema,
            ActorRef::Principal(controller),
        );
        let event = host.active_worlds[&world]
            .outbox
            .events_after_cursor(None, 1)?
            .remove(0);
        let command_id = policy.enqueue_command_id(&event);
        if let WorldEventReplayDecision::Enqueue(command) = enqueue(&policy, &event, command_id) {
            host.submit_world_command(world, command)?;
        }
        assert_eq!(
            host.active_worlds[&world]
                .outbox
                .event_consumer_cursor(policy.storage_consumer_id())?,
            None
        );
        drop(host);
        let (mut restarted, _, _) = fixture(dir.path(), world)?;
        assert_eq!(
            restarted.pump_world_event_consumer(&policy, |event, id| {
                Ok(enqueue(&policy, event, id))
            })?,
            1
        );
        assert_eq!(
            restarted.active_worlds[&world]
                .outbox
                .pending_effects(8)?
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn test_should_leave_checkpoint_unchanged_if_entity_control_is_unauthorized()
    -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let world = WorldId::new();
        let principal = PrincipalId::new();
        let (mut host, source_schema, enqueue_schema) = fixture(dir.path(), world)?;
        send_source(&mut host, world, source_schema, PrincipalId::new())?;
        let policy = policy_for(
            world,
            principal,
            enqueue_schema,
            ActorRef::Entity(EntityId::new()),
        );
        assert!(
            host.pump_world_event_consumer(&policy, |event, id| Ok(enqueue(&policy, event, id)))
                .is_err()
        );
        assert_eq!(
            host.active_worlds[&world]
                .outbox
                .event_consumer_cursor(policy.storage_consumer_id())?,
            None
        );
        assert!(
            host.active_worlds[&world]
                .outbox
                .pending_effects(8)?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn test_should_reject_bad_route_before_ack_and_resume_after_skip() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let world_id = WorldId::new();
        let controller = PrincipalId::new();
        let (mut host, source_schema, enqueue_schema) = fixture(directory.path(), world_id)?;
        send_source(&mut host, world_id, source_schema, PrincipalId::new())?;
        let policy = policy_for(
            world_id,
            controller,
            enqueue_schema,
            ActorRef::Principal(controller),
        );

        assert!(
            host.pump_world_event_consumer(&policy, |_event, _id| {
                Err(HostError::InvalidWorldEventReplayPolicy)
            })
            .is_err()
        );
        assert_eq!(
            host.active_worlds[&world_id]
                .outbox
                .event_consumer_cursor(policy.storage_consumer_id())?,
            None
        );

        // Same trusted identity but different source correlation is rejected.
        assert!(
            host.pump_world_event_consumer(&policy, |event, id| {
                let invalid = WorldCommand::with_ids(
                    id,
                    rintawa_sdk::world::CorrelationId::new(),
                    policy.enqueue_schema.clone(),
                    controller,
                    ActorRef::Principal(controller),
                    serde_json::json!({"source": event.id}),
                )
                .caused_by(CausationRef::Event(event.id));
                Ok(WorldEventReplayDecision::Enqueue(invalid))
            })
            .is_err()
        );
        assert_eq!(
            host.active_worlds[&world_id]
                .outbox
                .event_consumer_cursor(policy.storage_consumer_id())?,
            None
        );
        assert!(
            host.active_worlds[&world_id]
                .outbox
                .pending_effects(8)?
                .is_empty()
        );

        // A trusted policy can skip one event, but subsequent events remain
        // ordered and are still enqueued through normal World authority.
        assert_eq!(
            host.pump_world_event_consumer(&policy, |_event, _id| {
                Ok(WorldEventReplayDecision::Skip)
            })?,
            1
        );
        assert_eq!(
            host.pump_world_event_consumer(&policy, |event, id| {
                Ok(enqueue(&policy, event, id))
            })?,
            1
        );
        assert_eq!(
            host.active_worlds[&world_id]
                .outbox
                .pending_effects(8)?
                .len(),
            1
        );
        Ok(())
    }
}
