//! Implementation-neutral durable effect-job claim contracts.

use rintawa_sdk::world::UnixTimeMillis;
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
