//! SQLite codecs for world identifiers and schema definitions.

use rintawa_sdk::{
    types::ExtensionId,
    world::{
        CommandId, CorrelationId, EffectJobId, EntityId, PrincipalId, RelationId, SchemaId,
        SchemaKey, SchemaVersion, WorldEventId, WorldId,
    },
};
use rintawa_world::{SchemaDefinition, SchemaKind, SchemaRegistry};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StorageError, StorageResult};

pub(super) fn load_schema_registry(connection: &Connection) -> StorageResult<SchemaRegistry> {
    let mut statement = connection.prepare(
        "SELECT schema_id, schema_version, kind, owner_extension_id, definition_json
         FROM world_schemas ORDER BY schema_id, schema_version",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut registry = SchemaRegistry::new();
    for row in rows {
        let (schema_id, version, kind, owner, definition_json) = row?;
        registry.register(decode_schema(
            schema_id,
            version,
            kind,
            owner,
            definition_json,
        )?)?;
    }
    Ok(registry)
}

pub(super) fn load_schema(
    connection: &Connection,
    key: &SchemaKey,
) -> StorageResult<Option<SchemaDefinition>> {
    let row = connection
        .query_row(
            "SELECT kind, owner_extension_id, definition_json
             FROM world_schemas WHERE schema_id = ?1 AND schema_version = ?2",
            params![key.id().as_str(), i64::from(key.version().get())],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;

    row.map(|(kind, owner, definition_json)| {
        decode_schema(
            key.id().as_str().to_string(),
            i64::from(key.version().get()),
            kind,
            owner,
            definition_json,
        )
    })
    .transpose()
}

pub(super) fn decode_schema_key(schema_id: String, version: i64) -> StorageResult<SchemaKey> {
    let schema_id =
        SchemaId::parse(schema_id).map_err(|error| StorageError::CorruptData(error.to_string()))?;
    let version = u32::try_from(version)
        .ok()
        .and_then(|value| SchemaVersion::new(value).ok())
        .ok_or_else(|| StorageError::CorruptData("invalid schema version".into()))?;
    Ok(SchemaKey::new(schema_id, version))
}

fn decode_schema(
    schema_id: String,
    version: i64,
    kind: i64,
    owner: String,
    definition_json: String,
) -> StorageResult<SchemaDefinition> {
    let key = decode_schema_key(schema_id, version)?;
    let definition = serde_json::from_str(&definition_json)?;
    Ok(SchemaDefinition::new(
        key,
        schema_kind_from_code(kind)?,
        ExtensionId::new(owner),
        definition,
    ))
}

pub(super) fn schema_kind_code(kind: SchemaKind) -> i64 {
    match kind {
        SchemaKind::Entity => 1,
        SchemaKind::Relation => 2,
        SchemaKind::Facet => 3,
        SchemaKind::Command => 4,
        SchemaKind::Event => 5,
        SchemaKind::Effect => 6,
    }
}

fn schema_kind_from_code(code: i64) -> StorageResult<SchemaKind> {
    match code {
        1 => Ok(SchemaKind::Entity),
        2 => Ok(SchemaKind::Relation),
        3 => Ok(SchemaKind::Facet),
        4 => Ok(SchemaKind::Command),
        5 => Ok(SchemaKind::Event),
        6 => Ok(SchemaKind::Effect),
        _ => Err(StorageError::CorruptData(format!(
            "unknown schema kind code {code}"
        ))),
    }
}

fn fixed_id_bytes(bytes: Vec<u8>, label: &'static str) -> StorageResult<[u8; 16]> {
    bytes
        .try_into()
        .map_err(|_| StorageError::CorruptData(format!("{label} must contain 16 bytes")))
}

pub(super) fn world_id_from_blob(bytes: Vec<u8>) -> StorageResult<WorldId> {
    Ok(WorldId::from_bytes(fixed_id_bytes(bytes, "world_id")?))
}

pub(super) fn entity_id_from_blob(bytes: Vec<u8>) -> StorageResult<EntityId> {
    Ok(EntityId::from_bytes(fixed_id_bytes(bytes, "entity_id")?))
}

pub(super) fn relation_id_from_blob(bytes: Vec<u8>) -> StorageResult<RelationId> {
    Ok(RelationId::from_bytes(fixed_id_bytes(
        bytes,
        "relation_id",
    )?))
}

pub(super) fn command_id_from_blob(bytes: Vec<u8>) -> StorageResult<CommandId> {
    Ok(CommandId::from_bytes(fixed_id_bytes(bytes, "command_id")?))
}

pub(super) fn event_id_from_blob(bytes: Vec<u8>) -> StorageResult<WorldEventId> {
    Ok(WorldEventId::from_bytes(fixed_id_bytes(bytes, "event_id")?))
}

pub(super) fn principal_id_from_blob(bytes: Vec<u8>) -> StorageResult<PrincipalId> {
    Ok(PrincipalId::from_bytes(fixed_id_bytes(
        bytes,
        "principal_id",
    )?))
}

pub(super) fn correlation_id_from_blob(bytes: Vec<u8>) -> StorageResult<CorrelationId> {
    Ok(CorrelationId::from_bytes(fixed_id_bytes(
        bytes,
        "correlation_id",
    )?))
}

pub(super) fn effect_job_id_from_blob(bytes: Vec<u8>) -> StorageResult<EffectJobId> {
    Ok(EffectJobId::from_bytes(fixed_id_bytes(
        bytes,
        "effect_job_id",
    )?))
}

pub(super) fn digest_from_blob(bytes: Vec<u8>) -> StorageResult<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| StorageError::CorruptData("command digest must contain 32 bytes".into()))
}
