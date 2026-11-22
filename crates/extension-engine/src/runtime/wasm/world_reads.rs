//! Bounded guest graph reads over existing owner-scoped World snapshots.

use crate::runtime::wasm::{
    WitDirection, WorldDataError, WorldSystemSnapshotError as Error, map_world_system_read_error,
};
use rintawa_sdk::world::{ItemId, SchemaKey};
use rintawa_world::RelationDirection;
use rintawa_world_runtime::WorldSnapshot;

pub(super) fn input_size(
    parts: impl IntoIterator<Item = impl AsRef<str>>,
    maximum: usize,
) -> Result<(), Error> {
    if parts
        .into_iter()
        .try_fold(0usize, |total, value| {
            total.checked_add(value.as_ref().len())
        })
        .is_none_or(|bytes| bytes > maximum)
    {
        return Err(Error::MessageTooLarge);
    }
    Ok(())
}
fn ids(values: Vec<String>) -> Result<Vec<ItemId>, Error> {
    if values.len() > 64 {
        return Err(Error::InvalidRequest);
    }
    let mut result = values
        .into_iter()
        .map(|value| value.parse().map_err(|_| Error::InvalidRequest))
        .collect::<Result<Vec<_>, _>>()?;
    result.sort();
    result.dedup();
    Ok(result)
}
fn keys(values: Vec<String>) -> Result<Vec<SchemaKey>, Error> {
    if values.len() > 64 {
        return Err(Error::InvalidRequest);
    }
    let mut result = values
        .into_iter()
        .map(|value| value.parse().map_err(|_| Error::InvalidRequest))
        .collect::<Result<Vec<_>, _>>()?;
    result.sort();
    result.dedup();
    Ok(result)
}
fn after(value: Option<String>) -> Result<Option<ItemId>, Error> {
    value
        .map(|value| value.parse().map_err(|_| Error::InvalidRequest))
        .transpose()
}
fn page_limit(limit: u32) -> Result<usize, Error> {
    if !(1..=256).contains(&limit) {
        Err(Error::InvalidRequest)
    } else {
        Ok(limit as usize)
    }
}
pub(super) fn encode(value: impl serde::Serialize, maximum: usize) -> Result<String, Error> {
    let value = serde_json::to_string(&value).map_err(|_| Error::Unavailable)?;
    if value.len() > maximum {
        return Err(Error::MessageTooLarge);
    }
    Ok(value)
}
pub(super) fn pages<T: serde::Serialize>(
    values: Vec<T>,
    maximum: usize,
) -> Result<Vec<String>, Error> {
    let values = values
        .into_iter()
        .map(|value| encode(value, maximum))
        .collect::<Result<Vec<_>, _>>()?;
    encode(&values, maximum)?; // Include list delimiters, quotes and escaping.
    Ok(values)
}
pub(super) fn read(
    snapshot: &WorldSnapshot,
    items: Vec<String>,
    properties: Vec<String>,
    maximum: usize,
) -> Result<String, Error> {
    if items.len() > 64 || properties.len() > 64 {
        return Err(Error::InvalidRequest);
    }
    input_size(items.iter().chain(&properties), maximum)?;
    let batch = snapshot
        .read(&ids(items)?, &keys(properties)?)
        .map_err(map_world_system_read_error)?;
    encode(
        serde_json::json!({"items":batch.items(),"properties":batch.properties()}),
        maximum,
    )
}
pub(super) fn find_relations(
    snapshot: &WorldSnapshot,
    targets: Vec<String>,
    direction: WitDirection,
    property: Option<String>,
    cursor: Option<String>,
    limit: u32,
    maximum: usize,
) -> Result<Vec<String>, Error> {
    relation_page(
        snapshot,
        targets,
        direction,
        property,
        RelationCursor::After(cursor),
        limit,
        maximum,
    )
}

pub(super) fn find_relations_before(
    snapshot: &WorldSnapshot,
    targets: Vec<String>,
    direction: WitDirection,
    property: Option<String>,
    cursor: Option<String>,
    limit: u32,
    maximum: usize,
) -> Result<Vec<String>, Error> {
    relation_page(
        snapshot,
        targets,
        direction,
        property,
        RelationCursor::Before(cursor),
        limit,
        maximum,
    )
}

enum RelationCursor {
    After(Option<String>),
    Before(Option<String>),
}

fn relation_page(
    snapshot: &WorldSnapshot,
    targets: Vec<String>,
    direction: WitDirection,
    property: Option<String>,
    cursor: RelationCursor,
    limit: u32,
    maximum: usize,
) -> Result<Vec<String>, Error> {
    let (cursor, descending) = match cursor {
        RelationCursor::After(value) => (value, false),
        RelationCursor::Before(value) => (value, true),
    };
    if targets.len() > 64 {
        return Err(Error::InvalidRequest);
    }
    input_size(
        targets.iter().chain(property.iter()).chain(cursor.iter()),
        maximum,
    )?;
    let limit = page_limit(limit)?;
    let property = property
        .map(|value| {
            value
                .parse::<SchemaKey>()
                .map_err(|_| Error::InvalidRequest)
        })
        .transpose()?;
    let direction = match direction {
        WitDirection::Outgoing => RelationDirection::Outgoing,
        WitDirection::Incoming => RelationDirection::Incoming,
        WitDirection::Either => RelationDirection::Either,
    };
    let targets = ids(targets)?;
    let cursor = after(cursor)?;
    let values = if descending {
        snapshot.find_relations_before(&targets, direction, property.as_ref(), cursor, limit)
    } else {
        snapshot.find_relations(&targets, direction, property.as_ref(), cursor, limit)
    }
    .map_err(map_world_system_read_error)?;
    pages(values, maximum)
}
pub(super) fn find_properties(
    snapshot: &WorldSnapshot,
    key: String,
    cursor: Option<String>,
    limit: u32,
    maximum: usize,
) -> Result<Vec<String>, Error> {
    input_size(std::iter::once(&key).chain(cursor.iter()), maximum)?;
    let limit = page_limit(limit)?;
    let key = key.parse().map_err(|_| Error::InvalidRequest)?;
    pages(
        snapshot
            .find_properties(&key, after(cursor)?, limit)
            .map_err(map_world_system_read_error)?,
        maximum,
    )
}
pub(super) fn ordinary_error(error: Error) -> WorldDataError {
    match error {
        Error::AccessNotActive => WorldDataError::AccessNotActive,
        Error::InvalidRequest => WorldDataError::InvalidRequest,
        Error::AccessDenied => WorldDataError::AccessDenied,
        Error::Unavailable => WorldDataError::Unavailable,
        Error::MessageTooLarge => WorldDataError::MessageTooLarge,
    }
}
