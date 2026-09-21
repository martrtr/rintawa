//! Service-backed World System protocol for ordinary extension providers.

use std::{collections::HashSet, sync::Arc};

use rintawa_sdk::{
    contracts::{ContractKey, ContractVersion},
    services::ServiceCallResult,
    world::{EntityId, RelationId, SchemaKey, WorldId},
};
use rintawa_world::{
    EntityRecord, FacetRecord, FacetTarget, RelationRecord, WorldCommand, WorldTransaction,
};
use serde::{Deserialize, Serialize};

use crate::{SystemError, SystemResult, WorldSnapshot, WorldSystem};

/// Major version of the platform World System service protocol.
pub const WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(1);
/// Maximum read/continuation rounds permitted for one System evaluation.
pub const MAX_WORLD_SYSTEM_SERVICE_ROUNDS: usize = 8;
/// Maximum distinct snapshot reads permitted for one System evaluation.
pub const MAX_WORLD_SYSTEM_SERVICE_READS: usize = 256;
const MAX_SYSTEM_DIAGNOSTIC_BYTES: usize = 8 * 1024;

/// Returns the platform service contract for one exact command schema.
pub fn world_system_service_contract_key(command_schema: &SchemaKey) -> ContractKey {
    ContractKey::new(
        format!(
            "rintawa.world.system.{}.v{}",
            command_schema.id(),
            command_schema.version().get()
        ),
        WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION,
    )
}

/// One bounded read that an extension System may request from its pinned snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemReadRequest {
    /// Loads one entity by stable identity.
    Entity {
        /// Entity to read.
        entity_id: EntityId,
    },
    /// Loads one relation by stable identity.
    Relation {
        /// Relation to read.
        relation_id: RelationId,
    },
    /// Loads one exact facet from one state target.
    Facet {
        /// Target carrying the facet.
        target: FacetTarget,
        /// Exact versioned facet schema.
        schema: SchemaKey,
    },
}

/// One result resolved from the same pinned snapshot used for evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemReadResult {
    /// Result of an entity read.
    Entity {
        /// Requested entity identity.
        entity_id: EntityId,
        /// Entity value, or None when absent.
        value: Option<EntityRecord>,
    },
    /// Result of a relation read.
    Relation {
        /// Requested relation identity.
        relation_id: RelationId,
        /// Relation value, or None when absent.
        value: Option<RelationRecord>,
    },
    /// Result of a facet read.
    Facet {
        /// Requested target.
        target: FacetTarget,
        /// Requested facet schema.
        schema: SchemaKey,
        /// Facet value, or None when absent.
        value: Option<FacetRecord>,
    },
}

/// Stateless evaluation envelope sent to an extension System provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemServiceRequest {
    world_id: WorldId,
    snapshot_position: u64,
    command: WorldCommand,
    reads: Vec<WorldSystemReadResult>,
}

impl WorldSystemServiceRequest {
    fn new(
        world_id: WorldId,
        snapshot_position: u64,
        command: WorldCommand,
        reads: Vec<WorldSystemReadResult>,
    ) -> Self {
        Self {
            world_id,
            snapshot_position,
            command,
            reads,
        }
    }

    /// Returns the authoritative world being evaluated.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact pinned state position used for all reads.
    pub const fn snapshot_position(&self) -> u64 {
        self.snapshot_position
    }

    /// Returns the immutable command envelope.
    pub const fn command(&self) -> &WorldCommand {
        &self.command
    }

    /// Returns every read result resolved in earlier protocol rounds.
    pub fn reads(&self) -> &[WorldSystemReadResult] {
        &self.reads
    }
}

/// Response produced by one extension System service invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemServiceResponse {
    /// Evaluation finished with a proposed authoritative transaction.
    Transaction {
        /// Transaction still subject to normal runtime validation and commit.
        transaction: WorldTransaction,
    },
    /// Evaluation needs additional data from the same pinned snapshot.
    Read {
        /// New reads required before the provider can finish evaluation.
        requests: Vec<WorldSystemReadRequest>,
    },
    /// Domain rules deliberately rejected the command.
    Rejected {
        /// Sanitized, non-secret diagnostic suitable for user-visible failure.
        reason: String,
    },
    /// The provider could not evaluate the command.
    Failed {
        /// Sanitized, non-secret diagnostic suitable for host logs.
        reason: String,
    },
}

/// Transport used by a service-backed System implementation.
pub trait WorldSystemServiceClient: Send + Sync + 'static {
    /// Calls one exact platform service contract.
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>>;
}

impl<F> WorldSystemServiceClient for F
where
    F: Fn(&ContractKey, &[u8]) -> ServiceCallResult<Vec<u8>> + Send + Sync + 'static,
{
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self(contract, request)
    }
}

/// Ordinary World System whose evaluator lives behind a platform service contract.
pub struct ServiceWorldSystem {
    world_id: WorldId,
    command_schema: SchemaKey,
    contract: ContractKey,
    client: Arc<dyn WorldSystemServiceClient>,
}

impl ServiceWorldSystem {
    /// Creates a service-backed System for one exact command schema.
    pub fn new<C>(world_id: WorldId, command_schema: SchemaKey, client: C) -> Self
    where
        C: WorldSystemServiceClient,
    {
        Self::from_shared(world_id, command_schema, Arc::new(client))
    }

    /// Creates a service-backed System from one shared transport client.
    pub fn from_shared(
        world_id: WorldId,
        command_schema: SchemaKey,
        client: Arc<dyn WorldSystemServiceClient>,
    ) -> Self {
        let contract = world_system_service_contract_key(&command_schema);
        Self {
            world_id,
            command_schema,
            contract,
            client,
        }
    }

    /// Returns the authoritative world to which this service binding belongs.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the platform contract that must be provided in the world scope.
    pub const fn service_contract(&self) -> &ContractKey {
        &self.contract
    }
}

impl WorldSystem for ServiceWorldSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        if snapshot.world_id() != self.world_id {
            return Err(SystemError::Failed(String::from(
                "System service binding belongs to another world",
            )));
        }
        if command.schema() != &self.command_schema {
            return Err(SystemError::Failed(String::from(
                "System service received a mismatched command schema",
            )));
        }

        let mut reads = Vec::new();
        let mut seen_reads = HashSet::new();

        for _ in 0..MAX_WORLD_SYSTEM_SERVICE_ROUNDS {
            let envelope = WorldSystemServiceRequest::new(
                snapshot.world_id(),
                snapshot.position(),
                command.clone(),
                reads.clone(),
            );
            let request = serde_json::to_vec(&envelope).map_err(|_| {
                SystemError::Failed(String::from("failed to encode System request"))
            })?;
            let response = self
                .client
                .call(&self.contract, &request)
                .map_err(|error| {
                    SystemError::Failed(format!("System service transport failed: {error}"))
                })?;
            let response: WorldSystemServiceResponse =
                serde_json::from_slice(&response).map_err(|_| {
                    SystemError::Failed(String::from("invalid System service response"))
                })?;

            match response {
                WorldSystemServiceResponse::Transaction { transaction } => return Ok(transaction),
                WorldSystemServiceResponse::Rejected { reason } => {
                    return Err(SystemError::Rejected(validate_diagnostic(reason)?));
                }
                WorldSystemServiceResponse::Failed { reason } => {
                    return Err(SystemError::Failed(validate_diagnostic(reason)?));
                }
                WorldSystemServiceResponse::Read { requests } => {
                    if requests.is_empty() {
                        return Err(SystemError::Failed(String::from(
                            "System service requested an empty read round",
                        )));
                    }
                    if reads.len().saturating_add(requests.len()) > MAX_WORLD_SYSTEM_SERVICE_READS {
                        return Err(SystemError::Failed(String::from(
                            "System service exceeded the snapshot read limit",
                        )));
                    }
                    for request in requests {
                        if !seen_reads.insert(request.clone()) {
                            return Err(SystemError::Failed(String::from(
                                "System service repeated a resolved snapshot read",
                            )));
                        }
                        reads.push(resolve_read(snapshot, request)?);
                    }
                }
            }
        }

        Err(SystemError::Failed(String::from(
            "System service exceeded the evaluation round limit",
        )))
    }
}

fn resolve_read(
    snapshot: &WorldSnapshot,
    request: WorldSystemReadRequest,
) -> SystemResult<WorldSystemReadResult> {
    match request {
        WorldSystemReadRequest::Entity { entity_id } => Ok(WorldSystemReadResult::Entity {
            entity_id,
            value: snapshot.load_entity(entity_id)?,
        }),
        WorldSystemReadRequest::Relation { relation_id } => Ok(WorldSystemReadResult::Relation {
            relation_id,
            value: snapshot.load_relation(relation_id)?,
        }),
        WorldSystemReadRequest::Facet { target, schema } => {
            let value = snapshot.load_facet(target, &schema)?;
            Ok(WorldSystemReadResult::Facet {
                target,
                schema,
                value,
            })
        }
    }
}

fn validate_diagnostic(reason: String) -> SystemResult<String> {
    if reason.len() > MAX_SYSTEM_DIAGNOSTIC_BYTES {
        return Err(SystemError::Failed(String::from(
            "System service returned an oversized diagnostic",
        )));
    }
    Ok(reason)
}
