//! SQLite codecs for world identifiers, item kinds and property keys.

use crate::{StorageError, StorageResult};
use rintawa_sdk::world::{ItemId, SchemaId, SchemaKey, SchemaVersion, WorldId};

pub(super) fn fixed_id_bytes(bytes: Vec<u8>, label: &'static str) -> StorageResult<[u8; 16]> {
    bytes
        .try_into()
        .map_err(|_| StorageError::CorruptData(format!("{label} must contain 16 bytes")))
}

pub(super) fn world_id_from_blob(bytes: Vec<u8>) -> StorageResult<WorldId> {
    Ok(WorldId::from_bytes(fixed_id_bytes(bytes, "world_id")?))
}

pub(super) fn item_id_from_blob(bytes: Vec<u8>, label: &'static str) -> StorageResult<ItemId> {
    Ok(ItemId::from_bytes(fixed_id_bytes(bytes, label)?))
}

pub(super) fn decode_property_key(name: String, version: i64) -> StorageResult<SchemaKey> {
    let name =
        SchemaId::parse(name).map_err(|error| StorageError::CorruptData(error.to_string()))?;
    let version = u32::try_from(version)
        .ok()
        .and_then(|value| SchemaVersion::new(value).ok())
        .ok_or_else(|| StorageError::CorruptData("invalid property version".into()))?;
    Ok(SchemaKey::new(name, version))
}
