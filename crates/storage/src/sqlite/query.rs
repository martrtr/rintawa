//! Read-side SQLite projections for current state, events, and outbox jobs.

use rintawa_sdk::world::{EntityId, RelationId, SchemaKey, UnixTimeMillis, WorldId};
use rintawa_world::{
    ActorRef, CausationRef, CommandProvenance, EntityRecord, FacetRecord, FacetTarget,
    RelationRecord, StoredEffectJob, StoredWorldEvent, StoredWorldMutation,
    WORLD_MUTATION_FORMAT_VERSION, WorldMutation,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{StorageError, StorageResult};

use super::codec::{
    command_id_from_blob, correlation_id_from_blob, decode_schema_key, effect_job_id_from_blob,
    entity_id_from_blob, event_id_from_blob, principal_id_from_blob, relation_id_from_blob,
};

const MAX_READ_BATCH: usize = 1024;

pub(super) fn load_entity(
    connection: &Connection,
    entity_id: EntityId,
) -> StorageResult<Option<EntityRecord>> {
    let row = connection
        .query_row(
            "SELECT entity_id, schema_id, schema_version
             FROM entities WHERE entity_id = ?1",
            params![entity_id.into_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;

    row.map(|(id, schema_id, schema_version)| {
        Ok(EntityRecord::new(
            entity_id_from_blob(id)?,
            decode_schema_key(schema_id, schema_version)?,
        ))
    })
    .transpose()
}

pub(super) fn load_relation(
    connection: &Connection,
    relation_id: RelationId,
) -> StorageResult<Option<RelationRecord>> {
    let row = connection
        .query_row(
            "SELECT relation_id, schema_id, schema_version, from_entity_id, to_entity_id
             FROM relations WHERE relation_id = ?1",
            params![relation_id.into_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()?;

    row.map(|(id, schema_id, schema_version, from, to)| {
        Ok(RelationRecord::new(
            relation_id_from_blob(id)?,
            decode_schema_key(schema_id, schema_version)?,
            entity_id_from_blob(from)?,
            entity_id_from_blob(to)?,
        ))
    })
    .transpose()
}

pub(super) fn load_facet(
    connection: &Connection,
    target: FacetTarget,
    schema: &SchemaKey,
) -> StorageResult<Option<FacetRecord>> {
    let payload = match target {
        FacetTarget::World => connection
            .query_row(
                "SELECT payload_json FROM world_facets
                 WHERE schema_id = ?1 AND schema_version = ?2",
                params![schema.id().as_str(), i64::from(schema.version().get())],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        FacetTarget::Entity(entity_id) => connection
            .query_row(
                "SELECT payload_json FROM entity_facets
                 WHERE entity_id = ?1 AND schema_id = ?2 AND schema_version = ?3",
                params![
                    entity_id.into_bytes().as_slice(),
                    schema.id().as_str(),
                    i64::from(schema.version().get()),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        FacetTarget::Relation(relation_id) => connection
            .query_row(
                "SELECT payload_json FROM relation_facets
                 WHERE relation_id = ?1 AND schema_id = ?2 AND schema_version = ?3",
                params![
                    relation_id.into_bytes().as_slice(),
                    schema.id().as_str(),
                    i64::from(schema.version().get()),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
    };

    payload
        .map(|payload| {
            Ok(FacetRecord::new(
                target,
                schema.clone(),
                serde_json::from_str(&payload)?,
            ))
        })
        .transpose()
}

pub(super) fn mutations_after(
    connection: &Connection,
    position: u64,
    limit: usize,
) -> StorageResult<Vec<StoredWorldMutation>> {
    let limit = bounded_limit(limit);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let position = position_to_sql(position)?;
    let mut statement = connection.prepare(
        "SELECT commit_position, mutation_index, format_version, mutation_json
         FROM world_mutations
         WHERE commit_position > ?1
         ORDER BY commit_position, mutation_index
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![position, limit_to_sql(limit)?], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    rows.map(|row| {
        let (commit_position, mutation_index, format_version, mutation_json) = row?;
        let format_version = u32::try_from(format_version)
            .map_err(|_| StorageError::CorruptData("invalid mutation format version".into()))?;
        if format_version != WORLD_MUTATION_FORMAT_VERSION {
            return Err(StorageError::UnsupportedMutationFormat {
                found: format_version,
                supported: WORLD_MUTATION_FORMAT_VERSION,
            });
        }
        let mutation: WorldMutation = serde_json::from_str(&mutation_json)?;
        Ok(StoredWorldMutation {
            commit_position: position_from_sql(commit_position)?,
            mutation_index: index_from_sql(mutation_index, "mutation_index")?,
            format_version,
            mutation,
        })
    })
    .collect()
}

pub(super) fn events_after(
    connection: &Connection,
    world_id: WorldId,
    position: u64,
    limit: usize,
) -> StorageResult<Vec<StoredWorldEvent>> {
    let limit = bounded_limit(limit);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let position = position_to_sql(position)?;
    let mut statement = connection.prepare(
        "SELECT
            e.event_id, e.commit_position, e.event_index,
            e.schema_id, e.schema_version, e.payload_json,
            c.command_id, c.command_schema_id, c.command_schema_version,
            c.principal_id, c.actor_kind, c.actor_id,
            c.causation_kind, c.causation_id, c.correlation_id,
            c.effective_at_ms, c.recorded_at_ms
         FROM world_events e
         JOIN world_commits c ON c.commit_position = e.commit_position
         WHERE e.commit_position > ?1
         ORDER BY e.commit_position, e.event_index
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![position, limit_to_sql(limit)?], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            provenance_sql_from_row(row, 6)?,
        ))
    })?;

    rows.map(|row| {
        let (id, commit_position, event_index, schema_id, schema_version, payload, provenance) =
            row?;
        Ok(StoredWorldEvent {
            world_id,
            id: event_id_from_blob(id)?,
            commit_position: position_from_sql(commit_position)?,
            event_index: index_from_sql(event_index, "event_index")?,
            provenance: decode_provenance(provenance)?,
            schema: decode_schema_key(schema_id, schema_version)?,
            payload: serde_json::from_str(&payload)?,
        })
    })
    .collect()
}

pub(super) fn pending_effects(
    connection: &Connection,
    world_id: WorldId,
    limit: usize,
) -> StorageResult<Vec<StoredEffectJob>> {
    let limit = bounded_limit(limit);
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut statement = connection.prepare(
        "SELECT
            j.job_id, j.commit_position, j.job_index,
            j.schema_id, j.schema_version, j.payload_json, j.attempt_count,
            c.command_id, c.command_schema_id, c.command_schema_version,
            c.principal_id, c.actor_kind, c.actor_id,
            c.causation_kind, c.causation_id, c.correlation_id,
            c.effective_at_ms, c.recorded_at_ms
         FROM effect_jobs j
         JOIN world_commits c ON c.commit_position = j.commit_position
         WHERE j.status = 0
         ORDER BY j.commit_position, j.job_index
         LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit_to_sql(limit)?], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            provenance_sql_from_row(row, 7)?,
        ))
    })?;

    rows.map(|row| {
        let (
            id,
            commit_position,
            job_index,
            schema_id,
            schema_version,
            payload,
            attempt_count,
            provenance,
        ) = row?;
        Ok(StoredEffectJob {
            world_id,
            id: effect_job_id_from_blob(id)?,
            commit_position: position_from_sql(commit_position)?,
            job_index: index_from_sql(job_index, "job_index")?,
            provenance: decode_provenance(provenance)?,
            schema: decode_schema_key(schema_id, schema_version)?,
            payload: serde_json::from_str(&payload)?,
            attempt_count: index_from_sql(attempt_count, "attempt_count")?,
        })
    })
    .collect()
}

#[derive(Debug)]
struct ProvenanceSql {
    command_id: Vec<u8>,
    command_schema_id: String,
    command_schema_version: i64,
    principal_id: Vec<u8>,
    actor_kind: i64,
    actor_id: Vec<u8>,
    causation_kind: Option<i64>,
    causation_id: Option<Vec<u8>>,
    correlation_id: Vec<u8>,
    effective_at_ms: Option<i64>,
    recorded_at_ms: i64,
}

fn provenance_sql_from_row(row: &Row<'_>, base: usize) -> rusqlite::Result<ProvenanceSql> {
    Ok(ProvenanceSql {
        command_id: row.get(base)?,
        command_schema_id: row.get(base + 1)?,
        command_schema_version: row.get(base + 2)?,
        principal_id: row.get(base + 3)?,
        actor_kind: row.get(base + 4)?,
        actor_id: row.get(base + 5)?,
        causation_kind: row.get(base + 6)?,
        causation_id: row.get(base + 7)?,
        correlation_id: row.get(base + 8)?,
        effective_at_ms: row.get(base + 9)?,
        recorded_at_ms: row.get(base + 10)?,
    })
}

fn decode_provenance(value: ProvenanceSql) -> StorageResult<CommandProvenance> {
    let principal = principal_id_from_blob(value.principal_id)?;
    let actor = match value.actor_kind {
        1 => ActorRef::Principal(principal_id_from_blob(value.actor_id)?),
        2 => ActorRef::Entity(entity_id_from_blob(value.actor_id)?),
        code => {
            return Err(StorageError::CorruptData(format!(
                "unknown actor kind code {code}"
            )));
        }
    };

    let causation = match (value.causation_kind, value.causation_id) {
        (None, None) => None,
        (Some(1), Some(id)) => Some(CausationRef::Command(command_id_from_blob(id)?)),
        (Some(2), Some(id)) => Some(CausationRef::Event(event_id_from_blob(id)?)),
        (Some(3), Some(id)) => Some(CausationRef::Effect(effect_job_id_from_blob(id)?)),
        (Some(code), Some(_)) => {
            return Err(StorageError::CorruptData(format!(
                "unknown causation kind code {code}"
            )));
        }
        _ => {
            return Err(StorageError::CorruptData(
                "causation kind/id presence mismatch".into(),
            ));
        }
    };

    Ok(CommandProvenance {
        command_id: command_id_from_blob(value.command_id)?,
        command_schema: decode_schema_key(value.command_schema_id, value.command_schema_version)?,
        principal,
        actor,
        causation,
        correlation_id: correlation_id_from_blob(value.correlation_id)?,
        effective_at: value.effective_at_ms.map(UnixTimeMillis::new),
        recorded_at: UnixTimeMillis::new(value.recorded_at_ms),
    })
}

fn bounded_limit(limit: usize) -> usize {
    limit.min(MAX_READ_BATCH)
}

fn limit_to_sql(limit: usize) -> StorageResult<i64> {
    i64::try_from(limit).map_err(|_| StorageError::IndexOutOfRange("query limit"))
}

fn position_to_sql(position: u64) -> StorageResult<i64> {
    i64::try_from(position).map_err(|_| StorageError::IndexOutOfRange("commit_position"))
}

fn position_from_sql(position: i64) -> StorageResult<u64> {
    u64::try_from(position)
        .map_err(|_| StorageError::CorruptData("negative world commit position".into()))
}

fn index_from_sql(index: i64, label: &'static str) -> StorageResult<u32> {
    u32::try_from(index).map_err(|_| StorageError::CorruptData(format!("invalid {label}")))
}
