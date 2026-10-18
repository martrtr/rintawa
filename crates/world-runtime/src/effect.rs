//! Service-backed durable effect execution protocol for ordinary extension providers.
//!
//! An effect handler performs post-commit, potentially nondeterministic work. The
//! durable EffectJob identity is the external idempotency key. A handler may return
//! an optional follow-up WorldCommand; the host accepts it only when its identity,
//! causation, and correlation are deterministically tied to the claimed job.

use std::sync::Arc;

use rintawa_sdk::{
    contracts::{ContractKey, ContractVersion},
    services::{ServiceCallError, ServiceCallResult},
    world::{CommandId, EffectJobId, SchemaKey, UnixTimeMillis, WorldId},
};
use rintawa_storage::ClaimedEffectJob;
use rintawa_world::{CausationRef, StoredEffectJob, WorldCommand};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Major version of the platform World Effect service protocol.
pub const WORLD_EFFECT_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(1);
/// Maximum UTF-8 bytes accepted in a handler-provided retry/cancel diagnostic.
pub const MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES: usize = 8 * 1024;

/// Returns the platform service contract for one exact effect schema.
pub fn world_effect_service_contract_key(effect_schema: &SchemaKey) -> ContractKey {
    ContractKey::new(
        format!(
            "rintawa.world.effect.{}.v{}",
            effect_schema.id(),
            effect_schema.version().get()
        ),
        WORLD_EFFECT_SERVICE_PROTOCOL_VERSION,
    )
}

/// Returns the only valid follow-up command identity for one durable effect job.
///
/// Reusing the job's exact UUID bytes makes a replay after "command committed but
/// effect completion not persisted" idempotently resolve to `AlreadyCommitted`.
pub fn effect_completion_command_id(job_id: EffectJobId) -> CommandId {
    CommandId::from_bytes(job_id.into_bytes())
}

/// Durable effect execution envelope sent to an extension handler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldEffectServiceRequest {
    world_id: WorldId,
    job: StoredEffectJob,
    lease_expires_at: UnixTimeMillis,
    completion_command_id: CommandId,
}

impl WorldEffectServiceRequest {
    fn new(claim: &ClaimedEffectJob) -> Self {
        let job = claim.job().clone();
        Self {
            world_id: job.world_id,
            completion_command_id: effect_completion_command_id(job.id),
            job,
            lease_expires_at: claim.lease_expires_at(),
        }
    }

    /// Returns the authoritative world that owns the effect.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the complete durable job envelope and current fencing attempt.
    pub const fn job(&self) -> &StoredEffectJob {
        &self.job
    }

    /// Returns when another worker may reclaim this attempt.
    pub const fn lease_expires_at(&self) -> UnixTimeMillis {
        self.lease_expires_at
    }

    /// Returns the required identity of an optional follow-up WorldCommand.
    pub const fn completion_command_id(&self) -> CommandId {
        self.completion_command_id
    }
}

/// Result produced by one extension effect-handler invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldEffectServiceResponse {
    /// External work succeeded. The optional command re-enters authoritative state
    /// through the ordinary World Runtime before the durable job is completed.
    Complete {
        /// Optional authoritative follow-up command.
        command: Option<WorldCommand>,
    },
    /// External work should be tried again after host-owned backoff.
    Retry {
        /// Sanitized, non-secret diagnostic persisted with the job.
        reason: String,
    },
    /// External work is deliberately abandoned and the claimed job becomes cancelled.
    Cancel {
        /// Sanitized, non-secret terminal diagnostic persisted with cancellation.
        reason: String,
    },
}

/// Protocol or transport failure while invoking an effect handler.
#[derive(Debug, Error)]
pub enum WorldEffectServiceError {
    /// The claimed job belongs to another authoritative world.
    #[error("effect handler binding belongs to another world")]
    WorldMismatch,
    /// The claimed job uses another effect schema.
    #[error("effect handler received a mismatched effect schema")]
    SchemaMismatch,
    /// Generic service routing failed before a valid handler response was obtained.
    #[error("effect service transport failed: {0}")]
    Transport(#[from] ServiceCallError),
    /// The provider returned bytes that are not a valid protocol response.
    #[error("invalid effect service response")]
    InvalidResponse,
    /// The provider returned a diagnostic larger than the protocol permits.
    #[error("effect service returned an oversized diagnostic")]
    DiagnosticTooLarge,
    /// A follow-up command did not use the job-derived idempotency identity.
    #[error("effect follow-up command id must be derived from the durable job id")]
    CommandIdMismatch,
    /// A follow-up command attempted to replace the authenticated originating principal.
    #[error("effect follow-up command must preserve the durable job principal")]
    CommandPrincipalMismatch,
    /// A follow-up command did not preserve the originating correlation chain.
    #[error("effect follow-up command must preserve the durable job correlation id")]
    CommandCorrelationMismatch,
    /// A follow-up command did not cite the durable effect as direct causation.
    #[error("effect follow-up command must be caused by the durable effect job")]
    CommandCausationMismatch,
}

/// Result type used by the service-backed effect-handler protocol.
pub type WorldEffectServiceResult<T> = Result<T, WorldEffectServiceError>;

/// Transport used by a service-backed effect handler.
pub trait WorldEffectServiceClient: Send + Sync + 'static {
    /// Calls one exact platform service contract.
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>>;
}

impl<F> WorldEffectServiceClient for F
where
    F: Fn(&ContractKey, &[u8]) -> ServiceCallResult<Vec<u8>> + Send + Sync + 'static,
{
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self(contract, request)
    }
}

/// Ordinary extension effect handler bound to one exact schema and world scope.
pub struct ServiceWorldEffectHandler {
    world_id: WorldId,
    effect_schema: SchemaKey,
    contract: ContractKey,
    client: Arc<dyn WorldEffectServiceClient>,
}

impl ServiceWorldEffectHandler {
    /// Creates one service-backed effect handler.
    pub fn new<C>(world_id: WorldId, effect_schema: SchemaKey, client: C) -> Self
    where
        C: WorldEffectServiceClient,
    {
        Self::from_shared(world_id, effect_schema, Arc::new(client))
    }

    /// Creates one service-backed effect handler from a shared transport client.
    pub fn from_shared(
        world_id: WorldId,
        effect_schema: SchemaKey,
        client: Arc<dyn WorldEffectServiceClient>,
    ) -> Self {
        let contract = world_effect_service_contract_key(&effect_schema);
        Self {
            world_id,
            effect_schema,
            contract,
            client,
        }
    }

    /// Returns the platform service contract required by this effect schema.
    pub const fn service_contract(&self) -> &ContractKey {
        &self.contract
    }

    /// Executes one already-fenced durable claim through the extension provider.
    ///
    /// Success only means the provider response is structurally and causally valid;
    /// the Host still owns retry/cancel state transitions and authoritative command
    /// submission.
    pub fn execute(
        &self,
        claim: &ClaimedEffectJob,
    ) -> WorldEffectServiceResult<WorldEffectServiceResponse> {
        let job = claim.job();
        if job.world_id != self.world_id {
            return Err(WorldEffectServiceError::WorldMismatch);
        }
        if job.schema != self.effect_schema {
            return Err(WorldEffectServiceError::SchemaMismatch);
        }

        let request = serde_json::to_vec(&WorldEffectServiceRequest::new(claim))
            .map_err(|_| WorldEffectServiceError::InvalidResponse)?;
        let response = self.client.call(&self.contract, &request)?;
        let response: WorldEffectServiceResponse = serde_json::from_slice(&response)
            .map_err(|_| WorldEffectServiceError::InvalidResponse)?;
        validate_response(job, &response)?;
        Ok(response)
    }
}

fn validate_response(
    job: &StoredEffectJob,
    response: &WorldEffectServiceResponse,
) -> WorldEffectServiceResult<()> {
    match response {
        WorldEffectServiceResponse::Retry { reason }
        | WorldEffectServiceResponse::Cancel { reason } => {
            if reason.len() > MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES {
                return Err(WorldEffectServiceError::DiagnosticTooLarge);
            }
        }
        WorldEffectServiceResponse::Complete { command: None } => {}
        WorldEffectServiceResponse::Complete {
            command: Some(command),
        } => {
            if command.id() != effect_completion_command_id(job.id) {
                return Err(WorldEffectServiceError::CommandIdMismatch);
            }
            if command.principal() != job.provenance.principal {
                return Err(WorldEffectServiceError::CommandPrincipalMismatch);
            }
            if command.correlation_id() != job.provenance.correlation_id {
                return Err(WorldEffectServiceError::CommandCorrelationMismatch);
            }
            if command.causation() != Some(CausationRef::Effect(job.id)) {
                return Err(WorldEffectServiceError::CommandCausationMismatch);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::world::{CorrelationId, PrincipalId};
    use rintawa_world::{ActorRef, CommandProvenance};

    fn test_job() -> StoredEffectJob {
        let principal = PrincipalId::new();
        StoredEffectJob {
            world_id: WorldId::new(),
            id: EffectJobId::new(),
            commit_position: 1,
            job_index: 0,
            provenance: CommandProvenance {
                command_id: CommandId::new(),
                command_schema: "rintawa.test.seed@1".parse().unwrap(),
                principal,
                actor: ActorRef::Principal(principal),
                causation: None,
                correlation_id: CorrelationId::new(),
                effective_at: None,
                recorded_at: UnixTimeMillis::new(1),
            },
            schema: "rintawa.test.effect@1".parse().unwrap(),
            payload: serde_json::json!({ "input": "test" }),
            attempt_count: 1,
        }
    }

    fn valid_follow_up(job: &StoredEffectJob) -> WorldCommand {
        WorldCommand::with_ids(
            effect_completion_command_id(job.id),
            job.provenance.correlation_id,
            "rintawa.test.result@1".parse().unwrap(),
            job.provenance.principal,
            job.provenance.actor,
            serde_json::json!({ "result": "ok" }),
        )
        .caused_by(CausationRef::Effect(job.id))
    }

    #[test]
    fn test_should_accept_causally_bound_effect_follow_up_command() {
        let job = test_job();
        let response = WorldEffectServiceResponse::Complete {
            command: Some(valid_follow_up(&job)),
        };

        assert!(validate_response(&job, &response).is_ok());
    }

    #[test]
    fn test_should_reject_effect_follow_up_with_wrong_command_id() {
        let job = test_job();
        let command = WorldCommand::with_ids(
            CommandId::new(),
            job.provenance.correlation_id,
            "rintawa.test.result@1".parse().unwrap(),
            job.provenance.principal,
            job.provenance.actor,
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Effect(job.id));
        let response = WorldEffectServiceResponse::Complete {
            command: Some(command),
        };

        assert!(matches!(
            validate_response(&job, &response),
            Err(WorldEffectServiceError::CommandIdMismatch)
        ));
    }

    #[test]
    fn test_should_reject_effect_follow_up_with_replaced_principal() {
        let job = test_job();
        let other_principal = PrincipalId::new();
        let command = WorldCommand::with_ids(
            effect_completion_command_id(job.id),
            job.provenance.correlation_id,
            "rintawa.test.result@1".parse().unwrap(),
            other_principal,
            ActorRef::Principal(other_principal),
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Effect(job.id));

        assert!(matches!(
            validate_response(
                &job,
                &WorldEffectServiceResponse::Complete {
                    command: Some(command),
                },
            ),
            Err(WorldEffectServiceError::CommandPrincipalMismatch)
        ));
    }

    #[test]
    fn test_should_reject_effect_follow_up_with_wrong_causal_chain() {
        let job = test_job();
        let wrong_correlation = WorldCommand::with_ids(
            effect_completion_command_id(job.id),
            CorrelationId::new(),
            "rintawa.test.result@1".parse().unwrap(),
            job.provenance.principal,
            job.provenance.actor,
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Effect(job.id));
        assert!(matches!(
            validate_response(
                &job,
                &WorldEffectServiceResponse::Complete {
                    command: Some(wrong_correlation),
                },
            ),
            Err(WorldEffectServiceError::CommandCorrelationMismatch)
        ));

        let wrong_causation = WorldCommand::with_ids(
            effect_completion_command_id(job.id),
            job.provenance.correlation_id,
            "rintawa.test.result@1".parse().unwrap(),
            job.provenance.principal,
            job.provenance.actor,
            serde_json::json!({}),
        )
        .caused_by(CausationRef::Command(job.provenance.command_id));
        assert!(matches!(
            validate_response(
                &job,
                &WorldEffectServiceResponse::Complete {
                    command: Some(wrong_causation),
                },
            ),
            Err(WorldEffectServiceError::CommandCausationMismatch)
        ));
    }

    #[test]
    fn test_should_reject_oversized_effect_diagnostic() {
        let job = test_job();
        let response = WorldEffectServiceResponse::Retry {
            reason: "x".repeat(MAX_WORLD_EFFECT_DIAGNOSTIC_BYTES + 1),
        };

        assert!(matches!(
            validate_response(&job, &response),
            Err(WorldEffectServiceError::DiagnosticTooLarge)
        ));
    }
}
