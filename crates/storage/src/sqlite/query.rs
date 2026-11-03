//! Item, relation and property reads over the four-table model.

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_world::{ItemKind, ItemRecord, PropertyDefinition, PropertyRecord, PropertyRegistry};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StorageError, StorageResult};

use super::codec::{decode_property_key, item_id_from_blob};

pub(super) fn load_world_id(connection: &Connection) -> StorageResult<WorldId> {
    let bytes: Vec<u8> = connection
        .query_row("SELECT id FROM world", [], |row| row.get(0))
        .optional()?
        .ok_or(StorageError::Uninitialized)?;
    super::codec::world_id_from_blob(bytes)
}

pub(super) fn load_manifest(connection: &Connection) -> StorageResult<serde_json::Value> {
    let manifest: String = connection
        .query_row("SELECT manifest FROM world", [], |row| row.get(0))
        .optional()?
        .ok_or(StorageError::Uninitialized)?;
    Ok(serde_json::from_str(&manifest)?)
}

pub(super) fn load_item(connection: &Connection, id: ItemId) -> StorageResult<Option<ItemRecord>> {
    let row = connection
        .query_row(
            "SELECT type, subject, object FROM items WHERE id = ?1",
            params![id.into_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .optional()?;

    let Some((kind, subject, object)) = row else {
        return Ok(None);
    };
    let kind = ItemKind::from_code(kind)?;
    let subject = subject
        .map(|bytes| item_id_from_blob(bytes, "subject"))
        .transpose()?;
    let object = object
        .map(|bytes| item_id_from_blob(bytes, "object"))
        .transpose()?;

    Ok(Some(match (kind, subject, object) {
        (ItemKind::Object, _, _) => ItemRecord::object(id),
        (ItemKind::Relation, Some(subject), Some(object)) => {
            ItemRecord::relation(id, subject, object)
        }
        (ItemKind::Relation, _, _) => {
            return Err(StorageError::World(
                rintawa_world::WorldError::RelationEndpointAbsent(id),
            ));
        }
    }))
}

pub(super) fn load_property(
    connection: &Connection,
    item: ItemId,
    key: &SchemaKey,
) -> StorageResult<Option<PropertyRecord>> {
    let row = connection
        .query_row(
            "SELECT version, data FROM properties WHERE item = ?1 AND name = ?2",
            params![item.into_bytes().as_slice(), key.id().as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;

    row.map(|(version, data)| {
        if version != i64::from(key.version().get()) {
            return Err(StorageError::StalePropertyVersion {
                name: key.id().to_string(),
                expected: key.version().get(),
                found: u32::try_from(version).unwrap_or_default(),
            });
        }
        Ok(PropertyRecord::new(
            item,
            key.clone(),
            serde_json::from_str(&data)?,
        ))
    })
    .transpose()
}

pub(super) fn list_properties(
    connection: &Connection,
    item: ItemId,
) -> StorageResult<Vec<PropertyRecord>> {
    let mut statement = connection
        .prepare("SELECT name, version, data FROM properties WHERE item = ?1 ORDER BY name")?;
    let rows = statement.query_map(params![item.into_bytes().as_slice()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut properties = Vec::new();
    for row in rows {
        let (name, version, data) = row?;
        let key = decode_property_key(name, version)?;
        properties.push(PropertyRecord::new(item, key, serde_json::from_str(&data)?));
    }
    Ok(properties)
}

pub(super) fn relations_pointing_at(
    connection: &Connection,
    target: ItemId,
) -> StorageResult<Vec<ItemRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, subject, object FROM items
         WHERE type = 2 AND (subject = ?1 OR object = ?1) ORDER BY id",
    )?;
    let rows = statement.query_map(params![target.into_bytes().as_slice()], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;

    let mut relations = Vec::new();
    for row in rows {
        let (id, subject, object) = row?;
        relations.push(ItemRecord::relation(
            item_id_from_blob(id, "relation_id")?,
            item_id_from_blob(subject, "subject")?,
            item_id_from_blob(object, "object")?,
        ));
    }
    Ok(relations)
}

pub(super) fn load_registry(connection: &Connection) -> StorageResult<PropertyRegistry> {
    let mut statement = connection
        .prepare("SELECT name, version, schema FROM property_registry ORDER BY name, version")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut registry = PropertyRegistry::new();
    for row in rows {
        let (name, version, schema) = row?;
        let key = decode_property_key(name, version)?;
        let schema = serde_json::from_str(&schema)?;
        // Provenance lives in the Host composition; the vocabulary owns the
        // compiled definition.
        registry.register(PropertyDefinition::new(
            key,
            rintawa_sdk::types::ExtensionId::new("world"),
            schema,
        ))?;
    }
    Ok(registry)
}
