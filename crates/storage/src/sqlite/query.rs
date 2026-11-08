//! Item, relation and property reads over the four-table model.

use rintawa_sdk::world::{ItemId, SchemaKey, WorldId};
use rintawa_world::{ItemKind, ItemRecord, PropertyDefinition, PropertyRecord, PropertyRegistry};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::{StorageError, StorageResult};

use crate::sqlite::codec::{decode_property_key, item_id_from_blob, world_id_from_blob};

/// Maximum number of records one indexed read may return.
pub(crate) const MAX_QUERY_PAGE_SIZE: usize = 256;
/// Maximum number of supplied item IDs accepted by one relation read.
pub(crate) const MAX_QUERY_BATCH_SIZE: usize = 64;
/// Maximum JSON payload bytes returned by one property search page.
pub(crate) const MAX_PROPERTY_QUERY_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn validate_page_size(limit: usize) -> StorageResult<i64> {
    if limit == 0 || limit > MAX_QUERY_PAGE_SIZE {
        return Err(StorageError::InvalidQueryPageSize {
            requested: limit,
            maximum: MAX_QUERY_PAGE_SIZE,
        });
    }
    Ok(limit as i64)
}

pub(super) fn load_world_id(connection: &Connection) -> StorageResult<WorldId> {
    let bytes: Vec<u8> = connection
        .query_row("SELECT id FROM world", [], |row| row.get(0))
        .optional()?
        .ok_or(StorageError::Uninitialized)?;
    world_id_from_blob(bytes)
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

/// Reads a bounded keyset page using the (name, version, item) index.
///
/// A cursor is an exclusive item ID; the implicit initial cursor is the empty
/// BLOB, strictly before every valid 16-byte UUID. This leaves the indexed item
/// range seek available to SQLite without a dynamic SQL branch.
pub(super) fn find_properties(
    connection: &Connection,
    key: &SchemaKey,
    after: Option<ItemId>,
    limit: usize,
) -> StorageResult<Vec<PropertyRecord>> {
    let limit = validate_page_size(limit)?;
    let after_bytes = after.map(ItemId::into_bytes);
    let mut statement = connection.prepare(
        "SELECT item, data FROM properties
         WHERE name = ?1 AND version = ?2 AND item > COALESCE(?3, X'')
         ORDER BY item LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            key.id().as_str(),
            i64::from(key.version().get()),
            after_bytes.as_ref().map(|id| id.as_slice()),
            limit,
        ],
        |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut properties = Vec::new();
    let mut page_bytes = 0_usize;
    for row in rows {
        let (item, data) = row?;
        page_bytes = page_bytes.saturating_add(data.len());
        if page_bytes > MAX_PROPERTY_QUERY_BYTES {
            return Err(StorageError::QueryPageTooLarge {
                bytes: page_bytes,
                maximum: MAX_PROPERTY_QUERY_BYTES,
            });
        }
        properties.push(PropertyRecord::new(
            item_id_from_blob(item, "property search item")?,
            key.clone(),
            serde_json::from_str(&data)?,
        ));
    }
    Ok(properties)
}

/// Reads a bounded, deduplicated page of relation-items adjacent to any target.
///
/// Targets are native item IDs, not user-controlled SQL. Only placeholder
/// counts are interpolated in the statement; every ID remains bound as BLOB.
pub(super) fn relations_touching_any(
    connection: &Connection,
    targets: &[ItemId],
    after: Option<ItemId>,
    limit: usize,
) -> StorageResult<Vec<ItemRecord>> {
    let limit = validate_page_size(limit)?;
    if targets.len() > MAX_QUERY_BATCH_SIZE {
        return Err(StorageError::InvalidQueryBatchSize {
            requested: targets.len(),
            maximum: MAX_QUERY_BATCH_SIZE,
        });
    }
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; targets.len()].join(", ");
    let sql = format!(
        "SELECT id, subject, object FROM items
         WHERE type = 2 AND (subject IN ({placeholders}) OR object IN ({placeholders}))
         AND id > COALESCE(?, X'')
         ORDER BY id LIMIT ?",
    );
    let mut values = Vec::with_capacity(targets.len() * 2 + 2);
    for _ in 0..2 {
        for target in targets {
            values.push(Value::Blob(target.into_bytes().to_vec()));
        }
    }
    values.push(match after {
        Some(cursor) => Value::Blob(cursor.into_bytes().to_vec()),
        None => Value::Null,
    });
    values.push(Value::Integer(limit));

    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| {
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

/// Returns distinct property keys removed by SQLite cascading an item deletion.
///
/// A single JOIN covers both the item and all relations incident to that item,
/// avoiding one property query for each edge in a large graph.
pub(super) fn property_keys_deleted_with_item(
    connection: &Connection,
    item: ItemId,
) -> StorageResult<Vec<SchemaKey>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT p.name, p.version FROM properties p
         JOIN items i ON i.id = p.item
         WHERE i.id = ?1 OR
               (i.type = 2 AND (i.subject = ?1 OR i.object = ?1))
         ORDER BY p.name, p.version",
    )?;
    let rows = statement.query_map(params![item.into_bytes().as_slice()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut keys = Vec::new();
    for row in rows {
        let (name, version) = row?;
        keys.push(decode_property_key(name, version)?);
    }
    Ok(keys)
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
