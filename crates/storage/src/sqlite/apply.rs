//! Applying one validated change set as a single SQLite transaction.

use rintawa_world::{StateChange, WorldTransaction};
use rusqlite::{TransactionBehavior, params};

use crate::{StorageError, StorageResult};

/// Applies every change of one transaction atomically.
///
/// SQLite `IMMEDIATE` takes the write lock before the first statement, so either
/// every item and property change becomes visible or none does. The caller must
/// only reply to the user after this function returns `Ok`.
pub(super) fn apply(
    connection: &mut rusqlite::Connection,
    transaction: &WorldTransaction,
) -> StorageResult<()> {
    let sqlite = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    for change in transaction.changes() {
        match change {
            StateChange::CreateObject { id } => {
                sqlite
                    .execute(
                        "INSERT INTO items (id, type, subject, object) VALUES (?1, 1, NULL, NULL)",
                        params![id.into_bytes().as_slice()],
                    )
                    .map_err(|error| item_error(error, *id, "create"))?;
            }
            StateChange::CreateRelation {
                id,
                subject,
                object,
            } => {
                sqlite
                    .execute(
                        "INSERT INTO items (id, type, subject, object) VALUES (?1, 2, ?2, ?3)",
                        params![
                            id.into_bytes().as_slice(),
                            subject.into_bytes().as_slice(),
                            object.into_bytes().as_slice()
                        ],
                    )
                    .map_err(|error| item_error(error, *id, "create"))?;
            }
            StateChange::DeleteItem { id } => {
                sqlite.execute(
                    "DELETE FROM items WHERE id = ?1",
                    params![id.into_bytes().as_slice()],
                )?;
            }
            StateChange::SetProperty { item, key, data } => {
                sqlite.execute(
                    "INSERT INTO properties (item, name, version, data) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (item, name) DO UPDATE SET
                        version = excluded.version,
                        data = excluded.data",
                    params![
                        item.into_bytes().as_slice(),
                        key.id().as_str(),
                        i64::from(key.version().get()),
                        serde_json::to_string(data)?
                    ],
                )?;
            }
            StateChange::RemoveProperty { item, key } => {
                sqlite.execute(
                    "DELETE FROM properties WHERE item = ?1 AND name = ?2 AND version = ?3",
                    params![
                        item.into_bytes().as_slice(),
                        key.id().as_str(),
                        i64::from(key.version().get())
                    ],
                )?;
            }
        }
    }

    sqlite.commit()?;
    Ok(())
}

fn item_error(
    error: rusqlite::Error,
    id: rintawa_sdk::world::ItemId,
    action: &str,
) -> StorageError {
    let message = error.to_string();
    if message.contains("UNIQUE") {
        return StorageError::World(rintawa_world::WorldError::ItemAlreadyExists(id));
    }
    if message.contains("relation endpoints must be object items") {
        return StorageError::World(rintawa_world::WorldError::RelationEndpointAbsent(id));
    }
    StorageError::CorruptData(format!("cannot {action} item {id}: {message}"))
}
