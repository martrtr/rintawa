//! Host-owned, strictly ordered durable World event consumer checkpoints.
//!
//! Checkpoints reduce replay work but cannot replace a deterministic,
//! idempotent downstream WorldCommand/EffectJob. A crash after that command
//! commits and before the acknowledgement intentionally replays the event.
//! This storage interface must not be exposed as guest-extensible authority.

use rintawa_sdk::world::WorldId;
use rintawa_world::StoredWorldEvent;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{StorageError, StorageResult, WorldEventCursor};

const MAX_CONSUMER_ID_BYTES: usize = 256;

/// Restricts storage checkpoint keys to bounded, canonical Host-owned IDs.
fn validate_consumer_id(value: &str) -> StorageResult<()> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_CONSUMER_ID_BYTES
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'-' | b'_' | b':' | b'/' | b'@')
        })
    {
        return Err(StorageError::InvalidEventConsumerId);
    }
    Ok(())
}

/// Returns the persisted cursor for one consumer, or None for fresh consumers.
pub(super) fn load_cursor(
    connection: &Connection,
    world_id: WorldId,
    consumer_id: &str,
) -> StorageResult<Option<WorldEventCursor>> {
    validate_consumer_id(consumer_id)?;
    let stored: Option<(i64, i64)> = connection
        .query_row(
            "SELECT commit_position, event_index FROM world_event_consumers
             WHERE consumer_id = ?1",
            [consumer_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    stored
        .map(|(position, index)| {
            let commit_position = u64::try_from(position)
                .map_err(|_| StorageError::CorruptData("negative consumer commit".into()))?;
            if commit_position == 0 {
                return Err(StorageError::CorruptData(
                    "consumer checkpoint points to empty World".into(),
                ));
            }
            let event_index = u32::try_from(index)
                .map_err(|_| StorageError::CorruptData("invalid consumer event index".into()))?;
            Ok(WorldEventCursor::from_stored_parts(
                world_id,
                commit_position,
                event_index,
            ))
        })
        .transpose()
}

/// Persist one next-event acknowledgement under an IMMEDIATE transaction.
///
/// The selected event is checked against the authoritative next record in
/// World order, not merely compared with guest supplied event metadata.
pub(super) fn acknowledge_next(
    connection: &mut Connection,
    world_id: WorldId,
    consumer_id: &str,
    expected: Option<WorldEventCursor>,
    event: &StoredWorldEvent,
) -> StorageResult<WorldEventCursor> {
    validate_consumer_id(consumer_id)?;
    if event.world_id != world_id {
        return Err(StorageError::EventCursorWorldMismatch {
            expected: world_id,
            found: event.world_id,
        });
    }
    if let Some(cursor) = expected
        && cursor.world_id() != world_id
    {
        return Err(StorageError::EventCursorWorldMismatch {
            expected: world_id,
            found: cursor.world_id(),
        });
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_cursor(&transaction, world_id, consumer_id)?;
    if current != expected {
        return Err(StorageError::EventConsumerCursorConflict);
    }
    let (position, index) = current.map_or((0, u32::MAX), |cursor| {
        (cursor.commit_position(), cursor.event_index())
    });
    let position = i64::try_from(position)
        .map_err(|_| StorageError::IndexOutOfRange("consumer commit_position"))?;
    let next: Option<(i64, i64, Vec<u8>)> = transaction
        .query_row(
            "SELECT commit_position, event_index, event_id
             FROM world_events
             WHERE commit_position > ?1
                OR (commit_position = ?1 AND event_index > ?2)
             ORDER BY commit_position, event_index
             LIMIT 1",
            params![position, i64::from(index)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let (next_position, next_index, next_id) =
        next.ok_or(StorageError::EventConsumerEventNotNext)?;
    let expected_position = i64::try_from(event.commit_position)
        .map_err(|_| StorageError::IndexOutOfRange("event commit_position"))?;
    if next_position != expected_position
        || next_index != i64::from(event.event_index)
        || next_id.as_slice() != event.id.into_bytes().as_slice()
    {
        return Err(StorageError::EventConsumerEventNotNext);
    }

    if current.is_none() {
        transaction.execute(
            "INSERT INTO world_event_consumers (
                 consumer_id, commit_position, event_index
             ) VALUES (?1, ?2, ?3)",
            params![consumer_id, next_position, next_index],
        )?;
    } else {
        transaction.execute(
            "UPDATE world_event_consumers
             SET commit_position = ?2, event_index = ?3
             WHERE consumer_id = ?1",
            params![consumer_id, next_position, next_index],
        )?;
    }
    transaction.commit()?;
    Ok(WorldEventCursor::after(event))
}
