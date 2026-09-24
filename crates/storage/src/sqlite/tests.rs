use std::{
    collections::BTreeSet,
    sync::{Arc, Barrier},
};

use anyhow::Result;
use rintawa_sdk::{
    types::ExtensionId,
    world::{
        CommandId, CorrelationId, EffectJobId, EntityId, PrincipalId, RelationId, SchemaKey,
        UnixTimeMillis, WorldId,
    },
};
use rintawa_world::{
    ActorRef, CommitDisposition, CommitReceipt, ControlGrant, ControlScope, EffectJobDraft,
    EntityRecord, FacetRecord, FacetTarget, RelationRecord, SchemaDefinition, SchemaKind,
    SchemaRegistration, WorldCommand, WorldError, WorldEventDraft, WorldMutation, WorldTransaction,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

use super::*;

#[derive(Debug, Clone)]
struct TestSchemas {
    entity: SchemaKey,
    relation: SchemaKey,
    facet: SchemaKey,
    command: SchemaKey,
    other_command: SchemaKey,
    event: SchemaKey,
    effect: SchemaKey,
}

fn schema(key: &str, kind: SchemaKind, definition: serde_json::Value) -> Result<SchemaDefinition> {
    Ok(SchemaDefinition::new(
        key.parse()?,
        kind,
        ExtensionId::new("rintawa.test"),
        definition,
    ))
}

fn register_test_schemas(storage: &SqliteWorldStorage) -> Result<TestSchemas> {
    let command_schema = serde_json::json!({
        "type": "object",
        "properties": { "action": { "type": "string" } },
        "required": ["action"],
        "additionalProperties": false
    });
    let definitions = [
        schema(
            "rintawa.test.entity@1",
            SchemaKind::Entity,
            serde_json::json!({}),
        )?,
        schema(
            "rintawa.test.relation@1",
            SchemaKind::Relation,
            serde_json::json!({}),
        )?,
        schema(
            "rintawa.test.facet@1",
            SchemaKind::Facet,
            serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false
            }),
        )?,
        schema(
            "rintawa.test.command@1",
            SchemaKind::Command,
            command_schema.clone(),
        )?,
        schema(
            "rintawa.test.other-command@1",
            SchemaKind::Command,
            command_schema,
        )?,
        schema(
            "rintawa.test.event@1",
            SchemaKind::Event,
            serde_json::json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"],
                "additionalProperties": false
            }),
        )?,
        schema(
            "rintawa.test.effect@1",
            SchemaKind::Effect,
            serde_json::json!({
                "type": "object",
                "properties": { "input": { "type": "string" } },
                "required": ["input"],
                "additionalProperties": false
            }),
        )?,
    ];

    for definition in &definitions {
        assert_eq!(
            storage.register_schema(definition)?,
            SchemaRegistration::Registered
        );
    }

    Ok(TestSchemas {
        entity: definitions[0].key().clone(),
        relation: definitions[1].key().clone(),
        facet: definitions[2].key().clone(),
        command: definitions[3].key().clone(),
        other_command: definitions[4].key().clone(),
        event: definitions[5].key().clone(),
        effect: definitions[6].key().clone(),
    })
}

fn create_storage() -> Result<(TempDir, SqliteWorldStorage)> {
    let root = TempDir::new()?;
    let path = root.path().join("world.sqlite");
    let storage = SqliteWorldStorage::create(&path, WorldId::new())?;
    Ok((root, storage))
}

fn direct_command(schema: &SchemaKey, principal: PrincipalId, action: &str) -> WorldCommand {
    WorldCommand::new(
        schema.clone(),
        principal,
        ActorRef::Principal(principal),
        serde_json::json!({ "action": action }),
    )
}

fn command_with_id(
    id: CommandId,
    correlation_id: CorrelationId,
    schema: &SchemaKey,
    principal: PrincipalId,
    actor: ActorRef,
    action: &str,
) -> WorldCommand {
    WorldCommand::with_ids(
        id,
        correlation_id,
        schema.clone(),
        principal,
        actor,
        serde_json::json!({ "action": action }),
    )
}

fn populated_transaction(
    schemas: &TestSchemas,
) -> (
    WorldTransaction,
    EntityId,
    EntityId,
    RelationId,
    rintawa_sdk::world::WorldEventId,
    rintawa_sdk::world::EffectJobId,
) {
    let first = EntityId::new();
    let second = EntityId::new();
    let relation_id = RelationId::new();
    let event = WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "created" }),
    );
    let effect = EffectJobDraft::new(
        schemas.effect.clone(),
        serde_json::json!({ "input": "generate" }),
    );
    let event_id = event.id();
    let effect_id = effect.id();

    let mut transaction = WorldTransaction::new();
    transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(first, schemas.entity.clone()),
    });
    transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(second, schemas.entity.clone()),
    });
    transaction.push_mutation(WorldMutation::CreateRelation {
        relation: RelationRecord::new(relation_id, schemas.relation.clone(), first, second),
    });
    transaction.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(first),
            schemas.facet.clone(),
            serde_json::json!({ "name": "Alice" }),
        ),
    });
    transaction.push_event(event);
    transaction.push_effect(effect);

    (transaction, first, second, relation_id, event_id, effect_id)
}

fn commit_current(
    storage: &SqliteWorldStorage,
    command: &WorldCommand,
    transaction: &WorldTransaction,
) -> Result<CommitReceipt> {
    let snapshot = storage.snapshot_for_command(command)?;
    let evaluated_position = snapshot.position();
    Ok(storage.commit(command, transaction, evaluated_position)?)
}

fn commit_entity(
    storage: &SqliteWorldStorage,
    schemas: &TestSchemas,
    principal: PrincipalId,
) -> Result<EntityId> {
    let entity_id = EntityId::new();
    let command = direct_command(&schemas.command, principal, "create-entity");
    let mut transaction = WorldTransaction::new();
    transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    commit_current(storage, &command, &transaction)?;
    Ok(entity_id)
}

fn enqueue_effect(
    storage: &SqliteWorldStorage,
    schemas: &TestSchemas,
    principal: PrincipalId,
    input: &str,
) -> Result<EffectJobId> {
    let command = direct_command(&schemas.command, principal, "enqueue-effect");
    let effect = EffectJobDraft::new(
        schemas.effect.clone(),
        serde_json::json!({ "input": input }),
    );
    let effect_id = effect.id();
    let mut transaction = WorldTransaction::new();
    transaction.push_effect(effect);
    commit_current(storage, &command, &transaction)?;
    Ok(effect_id)
}

#[test]
fn test_should_create_reopen_and_restore_world() -> Result<()> {
    let (root, storage) = create_storage()?;
    let world_id = storage.world_id();
    let path = root.path().join("world.sqlite");

    assert_eq!(storage.load_session()?.commit_position(), 0);
    drop(storage);

    let reopened = SqliteWorldStorage::open(&path)?;
    assert_eq!(reopened.world_id(), world_id);
    assert_eq!(reopened.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_enable_wal_and_separate_read_connection() -> Result<()> {
    let (_root, storage) = create_storage()?;

    let writer = storage.writer()?;
    let writer_mode: String = writer.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    drop(writer);

    let reader = storage.reader()?;
    let reader_mode: String = reader.pragma_query_value(None, "journal_mode", |row| row.get(0))?;

    assert!(writer_mode.eq_ignore_ascii_case("wal"));
    assert!(reader_mode.eq_ignore_ascii_case("wal"));
    Ok(())
}

#[test]
fn test_should_persist_schema_registry_idempotently() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let definition = schema(
        "rintawa.test.facet@1",
        SchemaKind::Facet,
        serde_json::json!({ "type": "string" }),
    )?;

    assert_eq!(
        storage.register_schema(&definition)?,
        SchemaRegistration::Registered
    );
    assert_eq!(
        storage.register_schema(&definition)?,
        SchemaRegistration::AlreadyPresent
    );
    assert_eq!(
        storage.load_session()?.schemas().get(definition.key()),
        Some(&definition)
    );
    Ok(())
}

#[test]
fn test_should_register_schema_batch_atomically() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let existing = schema(
        "rintawa.test.existing@1",
        SchemaKind::Facet,
        serde_json::json!({ "type": "string" }),
    )?;
    storage.register_schema(&existing)?;

    let staged = schema(
        "rintawa.test.staged@1",
        SchemaKind::Event,
        serde_json::json!({ "type": "object" }),
    )?;
    let conflicting = SchemaDefinition::new(
        existing.key().clone(),
        SchemaKind::Facet,
        ExtensionId::new("rintawa.other-owner"),
        existing.definition().clone(),
    );

    assert!(matches!(
        storage.register_schemas(&[staged.clone(), conflicting]),
        Err(StorageError::World(WorldError::SchemaConflict { .. }))
    ));
    let session = storage.load_session()?;
    assert!(session.schemas().get(staged.key()).is_none());
    assert_eq!(session.schemas().get(existing.key()), Some(&existing));

    assert_eq!(
        storage.register_schemas(&[existing.clone(), staged.clone()])?,
        vec![
            SchemaRegistration::AlreadyPresent,
            SchemaRegistration::Registered,
        ]
    );
    assert_eq!(
        storage.load_session()?.schemas().get(staged.key()),
        Some(&staged)
    );
    Ok(())
}

#[test]
fn test_should_commit_state_event_and_effect_atomically_across_restart() -> Result<()> {
    let (root, storage) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "populate").expecting_position(0);
    let command_id = command.id();
    let (transaction, first, second, relation_id, event_id, effect_id) =
        populated_transaction(&schemas);

    let receipt = commit_current(&storage, &command, &transaction)?;
    assert_eq!(receipt.disposition(), CommitDisposition::Committed);
    assert_eq!(receipt.position(), 1);
    assert_eq!(receipt.event_ids(), &[event_id]);
    assert_eq!(receipt.effect_job_ids(), &[effect_id]);
    assert_eq!(
        storage.load_entity(first)?,
        Some(EntityRecord::new(first, schemas.entity.clone()))
    );
    assert!(storage.load_entity(second)?.is_some());
    assert!(storage.load_relation(relation_id)?.is_some());
    assert_eq!(
        storage
            .load_facet(FacetTarget::Entity(first), &schemas.facet)?
            .map(|facet| facet.payload().clone()),
        Some(serde_json::json!({ "name": "Alice" }))
    );

    let mutations = storage.mutations_after(0, 10)?;
    assert_eq!(mutations.len(), 4);
    assert_eq!(
        mutations[0].format_version,
        rintawa_world::WORLD_MUTATION_FORMAT_VERSION
    );

    let events = storage.events_after(0, 10)?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, event_id);
    assert_eq!(events[0].world_id, storage.world_id());
    assert_eq!(events[0].provenance.command_id, command_id);
    assert_eq!(events[0].provenance.principal, principal);
    assert_eq!(events[0].provenance.actor, ActorRef::Principal(principal));
    assert_eq!(
        events[0].provenance.correlation_id,
        command.correlation_id()
    );

    let effects = storage.pending_effects(10)?;
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].id, effect_id);
    assert_eq!(effects[0].world_id, storage.world_id());
    assert_eq!(effects[0].provenance.command_id, command_id);
    assert_eq!(
        effects[0].provenance.correlation_id,
        command.correlation_id()
    );
    drop(storage);

    let reopened = SqliteWorldStorage::open(&path)?;
    assert_eq!(reopened.load_session()?.commit_position(), 1);
    assert!(reopened.load_entity(first)?.is_some());
    assert_eq!(reopened.mutations_after(0, 10)?.len(), 4);
    assert_eq!(reopened.events_after(0, 10)?[0].id, event_id);
    assert_eq!(reopened.pending_effects(10)?[0].id, effect_id);
    Ok(())
}

#[test]
fn test_should_reject_unknown_core_mutation_format() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "populate");
    let (transaction, _, _, _, _, _) = populated_transaction(&schemas);
    commit_current(&storage, &command, &transaction)?;

    {
        let writer = storage.writer()?;
        writer.execute(
            "UPDATE world_mutations
             SET format_version = ?1
             WHERE commit_position = 1 AND mutation_index = 0",
            [i64::from(rintawa_world::WORLD_MUTATION_FORMAT_VERSION + 1)],
        )?;
    }

    let error = storage.mutations_after(0, 10).unwrap_err();
    assert!(matches!(
        error,
        StorageError::UnsupportedMutationFormat {
            found,
            supported
        } if found == rintawa_world::WORLD_MUTATION_FORMAT_VERSION + 1
            && supported == rintawa_world::WORLD_MUTATION_FORMAT_VERSION
    ));
    Ok(())
}

#[test]
fn test_should_skip_system_regeneration_for_identical_command_retry() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "populate");
    let (transaction, _, _, _, event_id, effect_id) = populated_transaction(&schemas);

    let first = commit_current(&storage, &command, &transaction)?;
    let pre_system = storage.committed_receipt(&command)?;
    let replay = pre_system.expect("committed command must have receipt");

    assert_eq!(first.disposition(), CommitDisposition::Committed);
    assert_eq!(replay.disposition(), CommitDisposition::AlreadyCommitted);
    assert_eq!(replay.position(), first.position());
    assert_eq!(replay.event_ids(), &[event_id]);
    assert_eq!(replay.effect_job_ids(), &[effect_id]);
    assert_eq!(storage.events_after(0, 10)?.len(), 1);
    assert_eq!(storage.pending_effects(10)?.len(), 1);
    Ok(())
}

#[test]
fn test_should_replay_same_command_even_if_system_output_would_change() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "nondeterministic");
    let (first_transaction, _, _, _, first_event_id, _) = populated_transaction(&schemas);
    let first = commit_current(&storage, &command, &first_transaction)?;

    let mut regenerated = WorldTransaction::new();
    let regenerated_event = WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "different generated output" }),
    );
    let regenerated_event_id = regenerated_event.id();
    regenerated.push_event(regenerated_event);

    let replay = storage.commit(&command, &regenerated, u64::MAX)?;

    assert_eq!(replay.disposition(), CommitDisposition::AlreadyCommitted);
    assert_eq!(replay.position(), first.position());
    assert_eq!(replay.event_ids(), &[first_event_id]);
    assert_ne!(first_event_id, regenerated_event_id);
    assert_eq!(storage.events_after(0, 10)?.len(), 1);
    Ok(())
}

#[test]
fn test_should_reject_command_id_reuse_with_other_input() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "first");
    let command_id = command.id();
    let correlation_id = command.correlation_id();
    let (transaction, _, _, _, _, _) = populated_transaction(&schemas);
    commit_current(&storage, &command, &transaction)?;

    let conflicting = command_with_id(
        command_id,
        correlation_id,
        &schemas.command,
        principal,
        ActorRef::Principal(principal),
        "different",
    );

    let error = storage
        .committed_receipt(&conflicting)
        .expect_err("same command id with different input must conflict");
    assert!(matches!(
        error,
        StorageError::World(WorldError::CommandIdConflict(id)) if id == command_id
    ));
    assert_eq!(storage.load_session()?.commit_position(), 1);
    assert_eq!(storage.events_after(0, 10)?.len(), 1);
    Ok(())
}

#[test]
fn test_should_roll_back_all_state_when_later_mutation_fails() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "rollback");
    let created = EntityId::new();
    let missing = EntityId::new();
    let relation_id = RelationId::new();

    let mut transaction = WorldTransaction::new();
    transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(created, schemas.entity.clone()),
    });
    transaction.push_mutation(WorldMutation::CreateRelation {
        relation: RelationRecord::new(relation_id, schemas.relation.clone(), created, missing),
    });
    transaction.push_event(WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "must rollback" }),
    ));

    let snapshot = storage.snapshot_for_command(&command)?;
    let evaluated_position = snapshot.position();
    let error = storage
        .commit(&command, &transaction, evaluated_position)
        .unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::RelationEndpointNotFound {
            relation_id: id,
            entity_id
        }) if id == relation_id && entity_id == missing
    ));
    assert!(storage.load_entity(created)?.is_none());
    assert!(storage.mutations_after(0, 10)?.is_empty());
    assert!(storage.events_after(0, 10)?.is_empty());
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_reject_invalid_payload_before_committing_state() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "invalid-facet");
    let entity_id = EntityId::new();

    let mut transaction = WorldTransaction::new();
    transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    transaction.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(entity_id),
            schemas.facet.clone(),
            serde_json::json!({ "name": 42 }),
        ),
    });

    let snapshot = storage.snapshot_for_command(&command)?;
    let evaluated_position = snapshot.position();
    let error = storage
        .commit(&command, &transaction, evaluated_position)
        .unwrap_err();
    let rendered = error.to_string();
    assert!(matches!(
        error,
        StorageError::World(WorldError::PayloadValidationFailed { .. })
    ));
    assert!(!rendered.contains("42"));
    assert!(storage.load_entity(entity_id)?.is_none());
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_reject_stale_position_without_creating_event() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let first_command = direct_command(&schemas.command, principal, "first").expecting_position(0);
    let (first, _, _, _, _, _) = populated_transaction(&schemas);
    commit_current(&storage, &first_command, &first)?;

    let stale_command = direct_command(&schemas.command, principal, "stale").expecting_position(0);

    let error = storage.snapshot_for_command(&stale_command).unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::StaleWorldPosition {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(storage.events_after(0, 10)?.len(), 1);
    assert_eq!(storage.load_session()?.commit_position(), 1);
    Ok(())
}

#[test]
fn test_should_keep_reads_consistent_inside_pinned_snapshot() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let entity_id = EntityId::new();

    let create_command = direct_command(&schemas.command, principal, "create");
    let mut create_transaction = WorldTransaction::new();
    create_transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    create_transaction.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(entity_id),
            schemas.facet.clone(),
            serde_json::json!({ "name": "before" }),
        ),
    });
    commit_current(&storage, &create_command, &create_transaction)?;

    let pinned = storage.snapshot()?;
    assert_eq!(pinned.position(), 1);
    assert_eq!(
        pinned
            .load_facet(FacetTarget::Entity(entity_id), &schemas.facet)?
            .map(|facet| facet.payload().clone()),
        Some(serde_json::json!({ "name": "before" }))
    );

    let update_command = direct_command(&schemas.command, principal, "update");
    let mut update_transaction = WorldTransaction::new();
    update_transaction.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(entity_id),
            schemas.facet.clone(),
            serde_json::json!({ "name": "after" }),
        ),
    });
    commit_current(&storage, &update_command, &update_transaction)?;

    assert_eq!(
        pinned
            .load_facet(FacetTarget::Entity(entity_id), &schemas.facet)?
            .map(|facet| facet.payload().clone()),
        Some(serde_json::json!({ "name": "before" }))
    );

    let fresh = storage.snapshot()?;
    assert_eq!(fresh.position(), 2);
    assert_eq!(
        fresh
            .load_facet(FacetTarget::Entity(entity_id), &schemas.facet)?
            .map(|facet| facet.payload().clone()),
        Some(serde_json::json!({ "name": "after" }))
    );
    Ok(())
}

#[test]
fn test_should_reject_output_evaluated_against_stale_snapshot() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();

    let delayed_command = direct_command(&schemas.command, principal, "delayed");
    let delayed_snapshot = storage.snapshot_for_command(&delayed_command)?;
    let evaluated_position = delayed_snapshot.position();
    assert_eq!(evaluated_position, 0);

    let mut delayed_transaction = WorldTransaction::new();
    delayed_transaction.push_event(WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "stale-output" }),
    ));

    let advancing_command = direct_command(&schemas.command, principal, "advance");
    let mut advancing_transaction = WorldTransaction::new();
    advancing_transaction.push_event(WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "advance" }),
    ));
    commit_current(&storage, &advancing_command, &advancing_transaction)?;

    let error = storage
        .commit(&delayed_command, &delayed_transaction, evaluated_position)
        .unwrap_err();

    assert!(matches!(
        error,
        StorageError::World(WorldError::WorldChangedSinceEvaluation {
            evaluated: 0,
            actual: 1
        })
    ));
    let events = storage.events_after(0, 10)?;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload,
        serde_json::json!({ "message": "advance" })
    );
    Ok(())
}

#[test]
fn test_should_reject_entity_actor_without_control_grant() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let actor_entity = commit_entity(&storage, &schemas, principal)?;
    let command = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "act" }),
    );
    let error = storage.snapshot_for_command(&command).unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::ActorUnauthorized { .. })
    ));
    assert_eq!(storage.load_session()?.commit_position(), 1);
    assert!(storage.events_after(0, 10)?.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_manually_constructed_empty_control_scope() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let command = direct_command(&schemas.command, principal, "invalid-grant");

    let mut transaction = WorldTransaction::new();
    transaction.push_mutation(WorldMutation::GrantControl {
        grant: ControlGrant::with_id(
            rintawa_sdk::world::ControlGrantId::new(),
            principal,
            EntityId::new(),
            ControlScope::ExactSchemas(BTreeSet::new()),
            None,
        ),
    });

    let snapshot = storage.snapshot_for_command(&command)?;
    let error = storage
        .commit(&command, &transaction, snapshot.position())
        .unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::EmptyControlScope)
    ));
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_authorize_exact_control_grant_and_revoke_it() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let actor_entity = commit_entity(&storage, &schemas, principal)?;

    let grant = ControlGrant::with_id(
        rintawa_sdk::world::ControlGrantId::new(),
        principal,
        actor_entity,
        ControlScope::exact([schemas.command.clone()])?,
        None,
    );
    let grant_id = grant.id();
    let grant_command = direct_command(&schemas.command, principal, "grant");
    let mut grant_transaction = WorldTransaction::new();
    grant_transaction.push_mutation(WorldMutation::GrantControl { grant });
    commit_current(&storage, &grant_command, &grant_transaction)?;

    let actor_command = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "act" }),
    );
    let mut actor_transaction = WorldTransaction::new();
    actor_transaction.push_event(WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "authorized" }),
    ));
    commit_current(&storage, &actor_command, &actor_transaction)?;

    let wrong_scope_command = WorldCommand::new(
        schemas.other_command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "wrong-scope" }),
    );
    let error = storage
        .snapshot_for_command(&wrong_scope_command)
        .unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::ActorUnauthorized { .. })
    ));

    let revoke_command = direct_command(&schemas.command, principal, "revoke");
    let mut revoke_transaction = WorldTransaction::new();
    revoke_transaction.push_mutation(WorldMutation::RevokeControl { grant_id });
    commit_current(&storage, &revoke_command, &revoke_transaction)?;

    let after_revoke = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "after-revoke" }),
    );
    let error = storage.snapshot_for_command(&after_revoke).unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::ActorUnauthorized { .. })
    ));
    assert_eq!(storage.load_session()?.commit_position(), 4);
    Ok(())
}

#[test]
fn test_should_reject_expired_control_grant() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let principal = PrincipalId::new();
    let actor_entity = commit_entity(&storage, &schemas, principal)?;

    let grant = ControlGrant::with_id(
        rintawa_sdk::world::ControlGrantId::new(),
        principal,
        actor_entity,
        ControlScope::AnyCommand,
        Some(2),
    );
    let grant_command = direct_command(&schemas.command, principal, "grant");
    let mut grant_transaction = WorldTransaction::new();
    grant_transaction.push_mutation(WorldMutation::GrantControl { grant });
    commit_current(&storage, &grant_command, &grant_transaction)?;

    let actor_command = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "last-valid" }),
    );
    let mut actor_transaction = WorldTransaction::new();
    actor_transaction.push_event(WorldEventDraft::new(
        schemas.event.clone(),
        serde_json::json!({ "message": "valid" }),
    ));
    commit_current(&storage, &actor_command, &actor_transaction)?;

    let expired = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(actor_entity),
        serde_json::json!({ "action": "expired" }),
    );
    let error = storage.snapshot_for_command(&expired).unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::ActorUnauthorized { .. })
    ));
    assert_eq!(storage.load_session()?.commit_position(), 3);
    Ok(())
}

#[test]
fn test_should_reject_mismatched_direct_principal_actor() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let authenticated = PrincipalId::new();
    let other = PrincipalId::new();
    let command = WorldCommand::new(
        schemas.command.clone(),
        authenticated,
        ActorRef::Principal(other),
        serde_json::json!({ "action": "spoof" }),
    );
    let error = storage.snapshot_for_command(&command).unwrap_err();
    assert!(matches!(
        error,
        StorageError::World(WorldError::ActorUnauthorized { .. })
    ));
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_should_reclaim_expired_effect_with_fencing_token() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "lease")?;

    assert!(matches!(
        storage.claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(100)),
        Err(StorageError::InvalidEffectLease)
    ));

    let first = storage
        .claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(200))?
        .expect("pending effect must be claimable");
    assert_eq!(first.job().id, effect_id);
    assert_eq!(first.attempt(), 1);
    assert_eq!(first.lease_expires_at(), UnixTimeMillis::new(200));
    assert!(
        storage
            .claim_next_effect(UnixTimeMillis::new(199), UnixTimeMillis::new(300))?
            .is_none()
    );

    let second = storage
        .claim_next_effect(UnixTimeMillis::new(200), UnixTimeMillis::new(300))?
        .expect("expired lease must be reclaimable");
    assert_eq!(second.job().id, effect_id);
    assert_eq!(second.attempt(), 2);

    assert!(matches!(
        storage.complete_effect(effect_id, first.attempt()),
        Err(StorageError::EffectJobClaimLost(id)) if id == effect_id
    ));
    storage.complete_effect(effect_id, second.attempt())?;
    assert!(
        storage
            .claim_next_effect(UnixTimeMillis::new(400), UnixTimeMillis::new(500))?
            .is_none()
    );
    assert!(matches!(
        storage.cancel_effect(effect_id),
        Err(StorageError::EffectJobTerminal(id)) if id == effect_id
    ));
    Ok(())
}

#[test]
fn test_should_fence_worker_side_effect_cancellation() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "cancel-fence")?;

    let first = storage
        .claim_next_effect(UnixTimeMillis::new(10), UnixTimeMillis::new(20))?
        .expect("effect must be claimable");
    let second = storage
        .claim_next_effect(UnixTimeMillis::new(20), UnixTimeMillis::new(30))?
        .expect("expired claim must be reclaimable");

    assert!(matches!(
        storage.cancel_claimed_effect(effect_id, first.attempt(), "stale cancel"),
        Err(StorageError::EffectJobClaimLost(id)) if id == effect_id
    ));
    storage.cancel_claimed_effect(effect_id, second.attempt(), "provider rejected request")?;
    assert!(
        storage
            .claim_next_effect(UnixTimeMillis::new(40), UnixTimeMillis::new(50))?
            .is_none()
    );
    Ok(())
}

#[test]
fn test_should_reject_effect_attempt_counter_overflow() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "overflow")?;

    {
        let connection = storage.writer()?;
        connection.execute(
            "UPDATE effect_jobs SET attempt_count = ?1 WHERE job_id = ?2",
            params![i64::from(u32::MAX), effect_id.into_bytes().as_slice()],
        )?;
    }

    assert!(matches!(
        storage.claim_next_effect(UnixTimeMillis::new(1), UnixTimeMillis::new(2)),
        Err(StorageError::EffectAttemptOverflow(id)) if id == effect_id
    ));
    Ok(())
}

#[test]
fn test_should_claim_effect_once_across_independent_storage_handles() -> Result<()> {
    let (root, first_storage) = create_storage()?;
    let schemas = register_test_schemas(&first_storage)?;
    let effect_id = enqueue_effect(
        &first_storage,
        &schemas,
        PrincipalId::new(),
        "concurrent-claim",
    )?;
    let second_storage = SqliteWorldStorage::open(root.path().join("world.sqlite"))?;
    let barrier = Arc::new(Barrier::new(2));

    let first_barrier = Arc::clone(&barrier);
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        first_storage.claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(200))
    });
    let second_barrier = Arc::clone(&barrier);
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        second_storage.claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(200))
    });

    let first = first.join().expect("first claim thread must not panic")?;
    let second = second.join().expect("second claim thread must not panic")?;
    let claims = [first, second].into_iter().flatten().collect::<Vec<_>>();

    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job().id, effect_id);
    assert_eq!(claims[0].attempt(), 1);
    Ok(())
}

#[test]
fn test_should_retry_effect_at_due_time_and_cancel_running_claim() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "retry")?;

    let first = storage
        .claim_next_effect(UnixTimeMillis::new(10), UnixTimeMillis::new(20))?
        .expect("effect must be claimable");
    storage.retry_effect(
        effect_id,
        first.attempt(),
        UnixTimeMillis::new(50),
        "temporary provider failure",
    )?;

    assert!(
        storage
            .claim_next_effect(UnixTimeMillis::new(49), UnixTimeMillis::new(60))?
            .is_none()
    );
    let second = storage
        .claim_next_effect(UnixTimeMillis::new(50), UnixTimeMillis::new(60))?
        .expect("retry must become claimable at available_at");
    assert_eq!(second.attempt(), 2);

    assert!(storage.cancel_effect(effect_id)?);
    assert!(!storage.cancel_effect(effect_id)?);
    assert!(matches!(
        storage.complete_effect(effect_id, second.attempt()),
        Err(StorageError::EffectJobClaimLost(id)) if id == effect_id
    ));
    assert!(
        storage
            .claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(110))?
            .is_none()
    );
    Ok(())
}

#[test]
fn test_should_preserve_retry_schedule_and_attempt_across_restart() -> Result<()> {
    let (root, storage) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "restart-retry")?;

    let first = storage
        .claim_next_effect(UnixTimeMillis::new(10), UnixTimeMillis::new(20))?
        .expect("effect must be claimable");
    storage.retry_effect(
        effect_id,
        first.attempt(),
        UnixTimeMillis::new(50),
        "sanitized transient failure",
    )?;
    drop(storage);

    let reopened = SqliteWorldStorage::open(&path)?;
    assert!(
        reopened
            .claim_next_effect(UnixTimeMillis::new(49), UnixTimeMillis::new(60))?
            .is_none()
    );
    let second = reopened
        .claim_next_effect(UnixTimeMillis::new(50), UnixTimeMillis::new(60))?
        .expect("persisted retry must become due after restart");
    assert_eq!(second.job().id, effect_id);
    assert_eq!(second.attempt(), 2);
    reopened.complete_effect(effect_id, second.attempt())?;
    drop(reopened);

    let reopened = SqliteWorldStorage::open(&path)?;
    assert!(
        reopened
            .claim_next_effect(UnixTimeMillis::new(100), UnixTimeMillis::new(110))?
            .is_none()
    );
    Ok(())
}

#[test]
fn test_should_keep_claim_authoritative_when_retry_error_is_rejected() -> Result<()> {
    let (_root, storage) = create_storage()?;
    let schemas = register_test_schemas(&storage)?;
    let effect_id = enqueue_effect(&storage, &schemas, PrincipalId::new(), "bounded-error")?;
    let claim = storage
        .claim_next_effect(UnixTimeMillis::new(1), UnixTimeMillis::new(10))?
        .expect("effect must be claimable");

    let error = "x".repeat(8 * 1024 + 1);
    assert!(matches!(
        storage.retry_effect(effect_id, claim.attempt(), UnixTimeMillis::new(20), &error),
        Err(StorageError::EffectErrorTooLarge { .. })
    ));

    storage.complete_effect(effect_id, claim.attempt())?;
    Ok(())
}

#[test]
fn test_should_migrate_v2_running_effect_back_to_claimable_pending_state() -> Result<()> {
    let root = TempDir::new()?;
    let path = root.path().join("world.sqlite");
    let world_id = WorldId::new();
    let command_id = CommandId::new();
    let principal = PrincipalId::new();
    let correlation_id = CorrelationId::new();
    let effect_id = EffectJobId::new();

    let mut connection = Connection::open(&path)?;
    configure_connection(&connection)?;
    migration::migrate_to_v1(&mut connection)?;
    connection.execute(
        "INSERT INTO world_metadata (
            singleton, world_id, world_format_version, commit_position
         ) VALUES (1, ?1, ?2, 1)",
        params![
            world_id.into_bytes().as_slice(),
            i64::from(WORLD_FORMAT_VERSION)
        ],
    )?;
    migration::migrate_to_v2(&mut connection)?;
    connection.execute(
        "INSERT INTO world_schemas (
            schema_id, schema_version, kind, owner_extension_id, definition_json
         ) VALUES
            ('rintawa.test.command', 1, 4, 'rintawa.test', '{}'),
            ('rintawa.test.effect', 1, 6, 'rintawa.test', '{}')",
        [],
    )?;
    connection.execute(
        "INSERT INTO world_commits (
            commit_position, command_id, command_schema_id, command_schema_version,
            principal_id, actor_kind, actor_id, causation_kind, causation_id,
            correlation_id, expected_position, effective_at_ms, recorded_at_ms,
            command_payload_json, command_digest
         ) VALUES (
            1, ?1, 'rintawa.test.command', 1,
            ?2, 1, ?2, NULL, NULL,
            ?3, NULL, NULL, 123, '{}', ?4
         )",
        params![
            command_id.into_bytes().as_slice(),
            principal.into_bytes().as_slice(),
            correlation_id.into_bytes().as_slice(),
            [7_u8; 32].as_slice(),
        ],
    )?;
    connection.execute(
        "INSERT INTO effect_jobs (
            job_id, commit_position, job_index,
            schema_id, schema_version, payload_json,
            status, attempt_count, last_error
         ) VALUES (?1, 1, 0, 'rintawa.test.effect', 1, '{}', 1, 4, 'legacy-running')",
        [effect_id.into_bytes().as_slice()],
    )?;
    drop(connection);

    let storage = SqliteWorldStorage::open(&path)?;
    let reader = storage.reader()?;
    assert_eq!(
        migration::storage_version(&reader)?,
        migration::STORAGE_SCHEMA_VERSION
    );
    let migrated: (i64, i64, i64, Option<i64>, Option<String>) = reader.query_row(
        "SELECT status, attempt_count, available_at_ms, lease_expires_at_ms, last_error
         FROM effect_jobs WHERE job_id = ?1",
        [effect_id.into_bytes().as_slice()],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    assert_eq!(
        migrated,
        (0, 4, 0, None, Some(String::from("legacy-running")))
    );
    drop(reader);

    let claim = storage
        .claim_next_effect(UnixTimeMillis::new(1), UnixTimeMillis::new(10))?
        .expect("legacy running job must be safely reclaimable after migration");
    assert_eq!(claim.job().id, effect_id);
    assert_eq!(claim.attempt(), 5);
    Ok(())
}

#[test]
fn test_should_migrate_storage_v1_to_current_schema() -> Result<()> {
    let root = TempDir::new()?;
    let path = root.path().join("world.sqlite");
    let world_id = WorldId::new();

    let mut connection = Connection::open(&path)?;
    configure_connection(&connection)?;
    migration::migrate_to_v1(&mut connection)?;
    connection.execute(
        "INSERT INTO world_metadata (
            singleton, world_id, world_format_version, commit_position
         ) VALUES (1, ?1, ?2, 0)",
        params![
            world_id.into_bytes().as_slice(),
            i64::from(WORLD_FORMAT_VERSION)
        ],
    )?;
    drop(connection);

    let storage = SqliteWorldStorage::open(&path)?;
    assert_eq!(storage.world_id(), world_id);
    let reader = storage.reader()?;
    assert_eq!(
        migration::storage_version(&reader)?,
        migration::STORAGE_SCHEMA_VERSION
    );
    let table_count: i64 = reader.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table'
           AND name IN (
               'world_commits', 'control_grants', 'control_grant_schemas',
               'world_mutations', 'world_events', 'effect_jobs'
           )",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(table_count, 6);
    Ok(())
}

#[test]
fn test_should_refuse_to_overwrite_existing_world_file() -> Result<()> {
    let (root, _storage) = create_storage()?;
    let path = root.path().join("world.sqlite");

    let error = SqliteWorldStorage::create(&path, WorldId::new()).unwrap_err();
    assert!(matches!(error, StorageError::AlreadyExists(_)));
    Ok(())
}
