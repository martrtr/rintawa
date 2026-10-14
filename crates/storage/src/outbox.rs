//! Implementation-neutral durable effect-job claim contracts.

use std::time::Instant;

use rintawa_sdk::world::{EffectJobId, UnixTimeMillis};
use rintawa_world::StoredEffectJob;

/// One durable effect job leased to an outbox worker.
///
/// The job's `attempt_count` is the fencing token for completion or retry:
/// transitions from an older lease are rejected after the job is claimed again.
/// Lease expiry makes the job reclaimable; external side effects must additionally
/// use the stable job identity as their idempotency key.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedEffectJob {
    job: StoredEffectJob,
    lease_expires_at: UnixTimeMillis,
}

impl ClaimedEffectJob {
    pub(crate) const fn new(job: StoredEffectJob, lease_expires_at: UnixTimeMillis) -> Self {
        Self {
            job,
            lease_expires_at,
        }
    }

    /// Returns the durable job envelope, including its current attempt number.
    pub const fn job(&self) -> &StoredEffectJob {
        &self.job
    }

    /// Returns the claim attempt used as a fencing token.
    pub const fn attempt(&self) -> u32 {
        self.job.attempt_count
    }

    /// Returns when this job becomes eligible for claim by another worker.
    pub const fn lease_expires_at(&self) -> UnixTimeMillis {
        self.lease_expires_at
    }

    /// Consumes the claim and returns the durable job envelope.
    pub fn into_job(self) -> StoredEffectJob {
        self.job
    }
}

/// Host-only fencing data for an authoritative follow-up World commit.
///
/// The SQLite command transaction compares the live durable attempt under the
/// same writer lock as the World commit. Elapsed time is monotonic, so a
/// long-running System evaluation cannot revive a lease that expires in flight.
#[derive(Debug, Clone)]
pub struct EffectCommitGuard {
    job_id: EffectJobId,
    attempt: u32,
    observed_at: UnixTimeMillis,
    observed_instant: Instant,
}

impl EffectCommitGuard {
    /// Binds a follow-up commit to this exact leased effect attempt.
    pub fn new(claim: &ClaimedEffectJob, observed_at: UnixTimeMillis) -> Self {
        Self {
            job_id: claim.job().id,
            attempt: claim.attempt(),
            observed_at,
            observed_instant: Instant::now(),
        }
    }

    /// Returns the only effect job that may cause the guarded command.
    pub const fn job_id(&self) -> EffectJobId {
        self.job_id
    }

    /// Returns the exact SQLite fencing attempt.
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the observed time advanced by elapsed monotonic wall time.
    pub fn commit_time(&self) -> UnixTimeMillis {
        let elapsed =
            i64::try_from(self.observed_instant.elapsed().as_millis()).unwrap_or(i64::MAX);
        UnixTimeMillis::new(self.observed_at.get().saturating_add(elapsed))
    }
}
