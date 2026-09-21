//! Crash-safe SQLite state transitions for durable effect jobs.

use rintawa_sdk::world::{EffectJobId, UnixTimeMillis};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{ClaimedEffectJob, StorageError, StorageResult};

use super::{codec::effect_job_id_from_blob, query};

const STATUS_PENDING: i64 = 0;
const STATUS_RUNNING: i64 = 1;
const STATUS_COMPLETED: i64 = 2;
const STATUS_CANCELLED: i64 = 3;
const MAX_LAST_ERROR_BYTES: usize = 8 * 1024;

pub(super) fn claim_next(
    connection: &mut Connection,
    world_id: rintawa_sdk::world::WorldId,
    now: UnixTimeMillis,
    lease_expires_at: UnixTimeMillis,
) -> StorageResult<Option<ClaimedEffectJob>> {
    if lease_expires_at <= now {
        return Err(StorageError::InvalidEffectLease);
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let candidate = transaction
        .query_row(
            "SELECT job_id, attempt_count
             FROM effect_jobs
             WHERE (status = ?1 AND available_at_ms <= ?3)
                OR (status = ?2 AND lease_expires_at_ms <= ?3)
             ORDER BY commit_position, job_index
             LIMIT 1",
            params![STATUS_PENDING, STATUS_RUNNING, now.get()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;

    let Some((job_id, attempt_count)) = candidate else {
        transaction.commit()?;
        return Ok(None);
    };
    let job_id = effect_job_id_from_blob(job_id)?;
    let attempt =
        u32::try_from(attempt_count).map_err(|_| StorageError::IndexOutOfRange("attempt_count"))?;
    let next_attempt = attempt
        .checked_add(1)
        .ok_or(StorageError::EffectAttemptOverflow(job_id))?;

    let changed = transaction.execute(
        "UPDATE effect_jobs
         SET status = ?1,
             attempt_count = ?2,
             lease_expires_at_ms = ?3
         WHERE job_id = ?4
           AND attempt_count = ?5
           AND (
                (status = ?6 AND available_at_ms <= ?7)
                OR (status = ?8 AND lease_expires_at_ms <= ?7)
           )",
        params![
            STATUS_RUNNING,
            i64::from(next_attempt),
            lease_expires_at.get(),
            job_id.into_bytes().as_slice(),
            i64::from(attempt),
            STATUS_PENDING,
            now.get(),
            STATUS_RUNNING,
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::EffectJobClaimLost(job_id));
    }

    let job = query::effect_job_by_id(&transaction, world_id, job_id)?
        .ok_or(StorageError::EffectJobNotFound(job_id))?;
    transaction.commit()?;
    Ok(Some(ClaimedEffectJob::new(job, lease_expires_at)))
}

pub(super) fn complete(
    connection: &Connection,
    job_id: EffectJobId,
    attempt: u32,
) -> StorageResult<()> {
    transition_claim(
        connection,
        job_id,
        "UPDATE effect_jobs
         SET status = ?1, lease_expires_at_ms = NULL, last_error = NULL
         WHERE job_id = ?2 AND status = ?3 AND attempt_count = ?4",
        params![
            STATUS_COMPLETED,
            job_id.into_bytes().as_slice(),
            STATUS_RUNNING,
            i64::from(attempt)
        ],
    )
}

pub(super) fn retry(
    connection: &Connection,
    job_id: EffectJobId,
    attempt: u32,
    available_at: UnixTimeMillis,
    diagnostic: &str,
) -> StorageResult<()> {
    if diagnostic.len() > MAX_LAST_ERROR_BYTES {
        return Err(StorageError::EffectErrorTooLarge {
            actual_bytes: diagnostic.len(),
            maximum_bytes: MAX_LAST_ERROR_BYTES,
        });
    }
    transition_claim(
        connection,
        job_id,
        "UPDATE effect_jobs
         SET status = ?1,
             available_at_ms = ?2,
             lease_expires_at_ms = NULL,
             last_error = ?3
         WHERE job_id = ?4 AND status = ?5 AND attempt_count = ?6",
        params![
            STATUS_PENDING,
            available_at.get(),
            diagnostic,
            job_id.into_bytes().as_slice(),
            STATUS_RUNNING,
            i64::from(attempt)
        ],
    )
}

pub(super) fn cancel(connection: &Connection, job_id: EffectJobId) -> StorageResult<bool> {
    let changed = connection.execute(
        "UPDATE effect_jobs
         SET status = ?1, lease_expires_at_ms = NULL
         WHERE job_id = ?2 AND status IN (?3, ?4)",
        params![
            STATUS_CANCELLED,
            job_id.into_bytes().as_slice(),
            STATUS_PENDING,
            STATUS_RUNNING
        ],
    )?;
    if changed == 1 {
        return Ok(true);
    }

    let status = connection
        .query_row(
            "SELECT status FROM effect_jobs WHERE job_id = ?1",
            [job_id.into_bytes().as_slice()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    match status {
        None => Err(StorageError::EffectJobNotFound(job_id)),
        Some(STATUS_CANCELLED) => Ok(false),
        Some(STATUS_COMPLETED) => Err(StorageError::EffectJobTerminal(job_id)),
        Some(_) => Err(StorageError::CorruptData(String::from(
            "effect job has unsupported status",
        ))),
    }
}

fn transition_claim<P>(
    connection: &Connection,
    job_id: EffectJobId,
    sql: &str,
    params: P,
) -> StorageResult<()>
where
    P: rusqlite::Params,
{
    let changed = connection.execute(sql, params)?;
    if changed == 1 {
        return Ok(());
    }

    let exists = connection
        .query_row(
            "SELECT 1 FROM effect_jobs WHERE job_id = ?1",
            [job_id.into_bytes().as_slice()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        Err(StorageError::EffectJobClaimLost(job_id))
    } else {
        Err(StorageError::EffectJobNotFound(job_id))
    }
}
