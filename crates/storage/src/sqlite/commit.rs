//! Atomic SQLite commit implementation for world commands and transactions.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use rintawa_sdk::world::{
    CommandId, EffectJobId, EntityId, RelationId, UnixTimeMillis, WorldEventId,
};
use rintawa_world::{
    ActorRef, CausationRef, CommitDisposition, CommitReceipt, ControlScope, FacetRecord,
    FacetTarget, WORLD_MUTATION_FORMAT_VERSION, WorldCommand, WorldError, WorldMutation,
    WorldTransaction,
};

use crate::sqlite::codec::{
    digest_from_blob, effect_job_id_from_blob, event_id_from_blob, load_schema_registry,
};
use crate::sqlite::query;
use crate::{StorageError, StorageResult};

pub(super) fn command_digest(command: &WorldCommand) -> StorageResult<[u8; 32]> {
    let canonical = canonical_json(serde_json::to_value(command)?);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(Sha256::digest(bytes).into())
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));

            let mut canonical = serde_json::Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonical_json(value));
            }
            serde_json::Value::Object(canonical)
        }
        scalar => scalar,
    }
}

pub(super) fn committed_receipt(
    connection: &Connection,
    command: &WorldCommand,
) -> StorageResult<Option<CommitReceipt>> {
    let digest = command_digest(command)?;
    load_existing_receipt(connection, command.id(), &digest)
}

pub(super) fn validate_command(
    connection: &Connection,
    command: &WorldCommand,
) -> StorageResult<u64> {
    let schemas = load_schema_registry(connection)?;
    command.validate_schema(&schemas)?;
    let position = current_position(connection)?;
    if let Some(expected) = command.expected_position()
        && expected != position
    {
        return Err(WorldError::StaleWorldPosition {
            expected,
            actual: position,
        }
        .into());
    }
    authorize_command(connection, command, position)?;
    Ok(position)
}

pub(super) fn commit(
    connection: &mut Connection,
    command: &WorldCommand,
    world_transaction: &WorldTransaction,
    evaluated_position: u64,
) -> StorageResult<CommitReceipt> {
    let digest = command_digest(command)?;
    let sql = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    if let Some(receipt) = load_existing_receipt(&sql, command.id(), &digest)? {
        sql.commit()?;
        return Ok(receipt);
    }

    let schemas = load_schema_registry(&sql)?;
    command.validate_schema(&schemas)?;
    world_transaction.validate_schemas(&schemas)?;
    reject_reused_event_or_effect_ids(&sql, world_transaction)?;

    let current_position = current_position(&sql)?;
    if current_position != evaluated_position {
        return Err(WorldError::WorldChangedSinceEvaluation {
            evaluated: evaluated_position,
            actual: current_position,
        }
        .into());
    }
    if let Some(expected) = command.expected_position()
        && expected != current_position
    {
        return Err(WorldError::StaleWorldPosition {
            expected,
            actual: current_position,
        }
        .into());
    }
    authorize_command(&sql, command, current_position)?;

    let position = current_position
        .checked_add(1)
        .ok_or(WorldError::CommitPositionOverflow)?;
    let position_sql = position_to_sql(position)?;
    insert_commit(&sql, command, &digest, position_sql)?;

    for (index, mutation) in world_transaction.mutations().iter().enumerate() {
        let mutation_json = serde_json::to_string(mutation)?;
        sql.execute(
            "INSERT INTO world_mutations (
                commit_position, mutation_index, format_version, mutation_json
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                position_sql,
                index_to_sql(index, "mutation_index")?,
                i64::from(WORLD_MUTATION_FORMAT_VERSION),
                mutation_json,
            ],
        )?;
        apply_mutation(&sql, mutation, position_sql)?;
    }

    for (index, event) in world_transaction.events().iter().enumerate() {
        let payload = serde_json::to_string(event.payload())?;
        sql.execute(
            "INSERT INTO world_events (
                event_id, commit_position, event_index,
                schema_id, schema_version, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.id().into_bytes().as_slice(),
                position_sql,
                index_to_sql(index, "event_index")?,
                event.schema().id().as_str(),
                i64::from(event.schema().version().get()),
                payload,
            ],
        )?;
    }

    for (index, effect) in world_transaction.effects().iter().enumerate() {
        let payload = serde_json::to_string(effect.payload())?;
        sql.execute(
            "INSERT INTO effect_jobs (
                job_id, commit_position, job_index,
                schema_id, schema_version, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                effect.id().into_bytes().as_slice(),
                position_sql,
                index_to_sql(index, "job_index")?,
                effect.schema().id().as_str(),
                i64::from(effect.schema().version().get()),
                payload,
            ],
        )?;
    }

    let updated = sql.execute(
        "UPDATE world_metadata
         SET commit_position = ?1
         WHERE singleton = 1 AND commit_position = ?2",
        params![position_sql, position_to_sql(current_position)?],
    )?;
    if updated != 1 {
        return Err(StorageError::CorruptData(
            "world metadata commit position changed inside serialized transaction".into(),
        ));
    }

    sql.commit()?;
    Ok(CommitReceipt::new(
        CommitDisposition::Committed,
        command.id(),
        position,
        world_transaction
            .events()
            .iter()
            .map(|event| event.id())
            .collect(),
        world_transaction
            .effects()
            .iter()
            .map(|effect| effect.id())
            .collect(),
    ))
}

fn insert_commit(
    transaction: &Transaction<'_>,
    command: &WorldCommand,
    digest: &[u8; 32],
    position: i64,
) -> StorageResult<()> {
    let (actor_kind, actor_id) = actor_parts(command.actor());
    let (causation_kind, causation_id) = causation_parts(command.causation());
    let expected_position = command
        .expected_position()
        .map(position_to_sql)
        .transpose()?;
    let effective_at = command.effective_at_timestamp().map(UnixTimeMillis::get);
    let recorded_at = recorded_at_now()?.get();
    let payload = serde_json::to_string(command.payload())?;

    transaction.execute(
        "INSERT INTO world_commits (
            commit_position, command_id, command_schema_id, command_schema_version,
            principal_id, actor_kind, actor_id, causation_kind, causation_id,
            correlation_id, expected_position, effective_at_ms, recorded_at_ms,
            command_payload_json, command_digest
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
         )",
        params![
            position,
            command.id().into_bytes().as_slice(),
            command.schema().id().as_str(),
            i64::from(command.schema().version().get()),
            command.principal().into_bytes().as_slice(),
            actor_kind,
            actor_id,
            causation_kind,
            causation_id,
            command.correlation_id().into_bytes().as_slice(),
            expected_position,
            effective_at,
            recorded_at,
            payload,
            digest.as_slice(),
        ],
    )?;
    Ok(())
}

fn actor_parts(actor: ActorRef) -> (i64, Vec<u8>) {
    match actor {
        ActorRef::Principal(id) => (1, id.into_bytes().to_vec()),
        ActorRef::Entity(id) => (2, id.into_bytes().to_vec()),
    }
}

fn causation_parts(causation: Option<CausationRef>) -> (Option<i64>, Option<Vec<u8>>) {
    match causation {
        None => (None, None),
        Some(CausationRef::Command(id)) => (Some(1), Some(id.into_bytes().to_vec())),
        Some(CausationRef::Event(id)) => (Some(2), Some(id.into_bytes().to_vec())),
        Some(CausationRef::Effect(id)) => (Some(3), Some(id.into_bytes().to_vec())),
    }
}

fn recorded_at_now() -> StorageResult<UnixTimeMillis> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let millis = i64::try_from(duration.as_millis())
                .map_err(|_| StorageError::IndexOutOfRange("recorded_at_ms"))?;
            Ok(UnixTimeMillis::new(millis))
        }
        Err(error) => {
            let millis = i64::try_from(error.duration().as_millis())
                .map_err(|_| StorageError::IndexOutOfRange("recorded_at_ms"))?;
            Ok(UnixTimeMillis::new(-millis))
        }
    }
}

fn authorize_command(
    connection: &Connection,
    command: &WorldCommand,
    position: u64,
) -> StorageResult<()> {
    match command.actor() {
        ActorRef::Principal(actor_principal) if actor_principal == command.principal() => Ok(()),
        ActorRef::Principal(_) => Err(WorldError::ActorUnauthorized {
            principal: command.principal(),
            actor: command.actor(),
            command: command.schema().clone(),
        }
        .into()),
        ActorRef::Entity(actor_entity) => {
            if query::principal_can_control(
                connection,
                command.principal(),
                actor_entity,
                command.schema(),
                position,
            )? {
                return Ok(());
            }
            Err(WorldError::ActorUnauthorized {
                principal: command.principal(),
                actor: command.actor(),
                command: command.schema().clone(),
            }
            .into())
        }
    }
}

fn load_existing_receipt(
    connection: &Connection,
    command_id: CommandId,
    digest: &[u8; 32],
) -> StorageResult<Option<CommitReceipt>> {
    let existing = connection
        .query_row(
            "SELECT commit_position, command_digest
             FROM world_commits WHERE command_id = ?1",
            params![command_id.into_bytes().as_slice()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;

    let Some((position, stored_digest)) = existing else {
        return Ok(None);
    };
    if digest_from_blob(stored_digest)? != *digest {
        return Err(WorldError::CommandIdConflict(command_id).into());
    }

    let position = position_from_sql(position)?;
    let event_ids = event_ids_for_position(connection, position)?;
    let effect_job_ids = effect_ids_for_position(connection, position)?;
    Ok(Some(CommitReceipt::new(
        CommitDisposition::AlreadyCommitted,
        command_id,
        position,
        event_ids,
        effect_job_ids,
    )))
}

fn event_ids_for_position(
    connection: &Connection,
    position: u64,
) -> StorageResult<Vec<WorldEventId>> {
    let mut statement = connection.prepare(
        "SELECT event_id FROM world_events
         WHERE commit_position = ?1 ORDER BY event_index",
    )?;
    let rows = statement.query_map(params![position_to_sql(position)?], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;

    rows.map(|row| event_id_from_blob(row?)).collect()
}

fn effect_ids_for_position(
    connection: &Connection,
    position: u64,
) -> StorageResult<Vec<EffectJobId>> {
    let mut statement = connection.prepare(
        "SELECT job_id FROM effect_jobs
         WHERE commit_position = ?1 ORDER BY job_index",
    )?;
    let rows = statement.query_map(params![position_to_sql(position)?], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;

    rows.map(|row| effect_job_id_from_blob(row?)).collect()
}

fn reject_reused_event_or_effect_ids(
    transaction: &Transaction<'_>,
    world_transaction: &WorldTransaction,
) -> StorageResult<()> {
    for event in world_transaction.events() {
        if blob_exists(
            transaction,
            "SELECT EXISTS(SELECT 1 FROM world_events WHERE event_id = ?1)",
            &event.id().into_bytes(),
        )? {
            return Err(WorldError::WorldEventAlreadyExists(event.id()).into());
        }
    }
    for effect in world_transaction.effects() {
        if blob_exists(
            transaction,
            "SELECT EXISTS(SELECT 1 FROM effect_jobs WHERE job_id = ?1)",
            &effect.id().into_bytes(),
        )? {
            return Err(WorldError::EffectJobAlreadyExists(effect.id()).into());
        }
    }
    Ok(())
}

fn current_position(connection: &Connection) -> StorageResult<u64> {
    let position: i64 = connection.query_row(
        "SELECT commit_position FROM world_metadata WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    position_from_sql(position)
}

fn apply_mutation(
    transaction: &Transaction<'_>,
    mutation: &WorldMutation,
    position: i64,
) -> StorageResult<()> {
    match mutation {
        WorldMutation::CreateEntity { entity } => {
            if entity_exists(transaction, entity.id())? {
                return Err(WorldError::EntityAlreadyExists(entity.id()).into());
            }
            transaction.execute(
                "INSERT INTO entities (
                    entity_id, schema_id, schema_version, created_position
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    entity.id().into_bytes().as_slice(),
                    entity.schema().id().as_str(),
                    i64::from(entity.schema().version().get()),
                    position,
                ],
            )?;
        }
        WorldMutation::DeleteEntity { entity_id } => {
            let changed = transaction.execute(
                "DELETE FROM entities WHERE entity_id = ?1",
                params![entity_id.into_bytes().as_slice()],
            )?;
            if changed == 0 {
                return Err(WorldError::EntityNotFound(*entity_id).into());
            }
        }
        WorldMutation::CreateRelation { relation } => {
            if relation_exists(transaction, relation.id())? {
                return Err(WorldError::RelationAlreadyExists(relation.id()).into());
            }
            ensure_relation_endpoint(transaction, relation.id(), relation.from())?;
            ensure_relation_endpoint(transaction, relation.id(), relation.to())?;
            transaction.execute(
                "INSERT INTO relations (
                    relation_id, schema_id, schema_version,
                    from_entity_id, to_entity_id, created_position
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    relation.id().into_bytes().as_slice(),
                    relation.schema().id().as_str(),
                    i64::from(relation.schema().version().get()),
                    relation.from().into_bytes().as_slice(),
                    relation.to().into_bytes().as_slice(),
                    position,
                ],
            )?;
        }
        WorldMutation::DeleteRelation { relation_id } => {
            let changed = transaction.execute(
                "DELETE FROM relations WHERE relation_id = ?1",
                params![relation_id.into_bytes().as_slice()],
            )?;
            if changed == 0 {
                return Err(WorldError::RelationNotFound(*relation_id).into());
            }
        }
        WorldMutation::SetFacet { facet } => {
            set_facet(transaction, facet, position)?;
        }
        WorldMutation::RemoveFacet { target, schema } => {
            remove_facet(transaction, *target, schema)?;
        }
        WorldMutation::GrantControl { grant } => {
            grant_control(transaction, grant, position)?;
        }
        WorldMutation::RevokeControl { grant_id } => {
            let changed = transaction.execute(
                "DELETE FROM control_grants WHERE grant_id = ?1",
                params![grant_id.into_bytes().as_slice()],
            )?;
            if changed == 0 {
                return Err(WorldError::ControlGrantNotFound(*grant_id).into());
            }
        }
    }
    Ok(())
}

fn grant_control(
    transaction: &Transaction<'_>,
    grant: &rintawa_world::ControlGrant,
    position: i64,
) -> StorageResult<()> {
    if !entity_exists(transaction, grant.actor_entity())? {
        return Err(WorldError::EntityNotFound(grant.actor_entity()).into());
    }
    if blob_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM control_grants WHERE grant_id = ?1)",
        &grant.id().into_bytes(),
    )? {
        return Err(WorldError::ControlGrantAlreadyExists(grant.id()).into());
    }

    let (scope_kind, exact_schemas) = match grant.scope() {
        ControlScope::AnyCommand => (1_i64, None),
        ControlScope::ExactSchemas(schemas) => (2_i64, Some(schemas)),
    };
    let valid_through = grant
        .valid_through_position()
        .map(position_to_sql)
        .transpose()?;

    transaction.execute(
        "INSERT INTO control_grants (
            grant_id, principal_id, actor_entity_id,
            scope_kind, valid_through_position, created_position
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            grant.id().into_bytes().as_slice(),
            grant.principal().into_bytes().as_slice(),
            grant.actor_entity().into_bytes().as_slice(),
            scope_kind,
            valid_through,
            position,
        ],
    )?;

    if let Some(schemas) = exact_schemas {
        for schema in schemas {
            transaction.execute(
                "INSERT INTO control_grant_schemas (
                    grant_id, schema_id, schema_version
                 ) VALUES (?1, ?2, ?3)",
                params![
                    grant.id().into_bytes().as_slice(),
                    schema.id().as_str(),
                    i64::from(schema.version().get()),
                ],
            )?;
        }
    }
    Ok(())
}

fn set_facet(
    transaction: &Transaction<'_>,
    facet: &FacetRecord,
    position: i64,
) -> StorageResult<()> {
    let payload = serde_json::to_string(facet.payload())?;
    match facet.target() {
        FacetTarget::World => {
            transaction.execute(
                "INSERT INTO world_facets (
                    schema_id, schema_version, payload_json, updated_position
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(schema_id, schema_version) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    updated_position = excluded.updated_position",
                params![
                    facet.schema().id().as_str(),
                    i64::from(facet.schema().version().get()),
                    payload,
                    position,
                ],
            )?;
        }
        FacetTarget::Entity(entity_id) => {
            if !entity_exists(transaction, entity_id)? {
                return Err(WorldError::FacetTargetNotFound.into());
            }
            transaction.execute(
                "INSERT INTO entity_facets (
                    entity_id, schema_id, schema_version, payload_json, updated_position
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(entity_id, schema_id, schema_version) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    updated_position = excluded.updated_position",
                params![
                    entity_id.into_bytes().as_slice(),
                    facet.schema().id().as_str(),
                    i64::from(facet.schema().version().get()),
                    payload,
                    position,
                ],
            )?;
        }
        FacetTarget::Relation(relation_id) => {
            if !relation_exists(transaction, relation_id)? {
                return Err(WorldError::FacetTargetNotFound.into());
            }
            transaction.execute(
                "INSERT INTO relation_facets (
                    relation_id, schema_id, schema_version, payload_json, updated_position
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(relation_id, schema_id, schema_version) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    updated_position = excluded.updated_position",
                params![
                    relation_id.into_bytes().as_slice(),
                    facet.schema().id().as_str(),
                    i64::from(facet.schema().version().get()),
                    payload,
                    position,
                ],
            )?;
        }
    }
    Ok(())
}

fn remove_facet(
    transaction: &Transaction<'_>,
    target: FacetTarget,
    schema: &rintawa_sdk::world::SchemaKey,
) -> StorageResult<()> {
    match target {
        FacetTarget::World => {
            transaction.execute(
                "DELETE FROM world_facets
                 WHERE schema_id = ?1 AND schema_version = ?2",
                params![schema.id().as_str(), i64::from(schema.version().get())],
            )?;
        }
        FacetTarget::Entity(entity_id) => {
            if !entity_exists(transaction, entity_id)? {
                return Err(WorldError::FacetTargetNotFound.into());
            }
            transaction.execute(
                "DELETE FROM entity_facets
                 WHERE entity_id = ?1 AND schema_id = ?2 AND schema_version = ?3",
                params![
                    entity_id.into_bytes().as_slice(),
                    schema.id().as_str(),
                    i64::from(schema.version().get()),
                ],
            )?;
        }
        FacetTarget::Relation(relation_id) => {
            if !relation_exists(transaction, relation_id)? {
                return Err(WorldError::FacetTargetNotFound.into());
            }
            transaction.execute(
                "DELETE FROM relation_facets
                 WHERE relation_id = ?1 AND schema_id = ?2 AND schema_version = ?3",
                params![
                    relation_id.into_bytes().as_slice(),
                    schema.id().as_str(),
                    i64::from(schema.version().get()),
                ],
            )?;
        }
    }
    Ok(())
}

fn ensure_relation_endpoint(
    transaction: &Transaction<'_>,
    relation_id: RelationId,
    entity_id: EntityId,
) -> StorageResult<()> {
    if entity_exists(transaction, entity_id)? {
        return Ok(());
    }
    Err(WorldError::RelationEndpointNotFound {
        relation_id,
        entity_id,
    }
    .into())
}

fn entity_exists(transaction: &Transaction<'_>, id: EntityId) -> StorageResult<bool> {
    blob_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM entities WHERE entity_id = ?1)",
        &id.into_bytes(),
    )
}

fn relation_exists(transaction: &Transaction<'_>, id: RelationId) -> StorageResult<bool> {
    blob_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM relations WHERE relation_id = ?1)",
        &id.into_bytes(),
    )
}

fn blob_exists(connection: &Connection, query: &str, id: &[u8; 16]) -> StorageResult<bool> {
    let exists: i64 = connection.query_row(query, params![id.as_slice()], |row| row.get(0))?;
    Ok(exists != 0)
}

fn position_to_sql(position: u64) -> StorageResult<i64> {
    i64::try_from(position).map_err(|_| StorageError::IndexOutOfRange("commit_position"))
}

fn position_from_sql(position: i64) -> StorageResult<u64> {
    u64::try_from(position)
        .map_err(|_| StorageError::CorruptData("negative world commit position".into()))
}

fn index_to_sql(index: usize, label: &'static str) -> StorageResult<i64> {
    let index = u32::try_from(index).map_err(|_| StorageError::IndexOutOfRange(label))?;
    Ok(i64::from(index))
}
