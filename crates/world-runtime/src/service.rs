//! Service-backed World System protocol for ordinary extension providers.

use std::{collections::HashSet, sync::Arc};

use rintawa_sdk::{
    contracts::ContractKey,
    services::ServiceCallResult,
    world::{SchemaKey, WorldId},
    world_system::{
        WorldSystemActor, WorldSystemCausation, WorldSystemCommand, WorldSystemEffectProposal,
        WorldSystemEntityRecord, WorldSystemEventProposal, WorldSystemFacetRecord,
        WorldSystemFacetTarget, WorldSystemMutation, WorldSystemRelationRecord,
    },
};
use rintawa_world::{
    ActorRef, CausationRef, EffectJobDraft, EntityRecord, FacetRecord, FacetTarget, RelationRecord,
    WorldCommand, WorldEventDraft, WorldMutation, WorldTransaction,
};

use crate::{SystemError, SystemResult, WorldSnapshot, WorldSystem};

pub use rintawa_sdk::world_system::{
    WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION, WorldSystemReadRequest, WorldSystemReadResult,
    WorldSystemServiceRequest, WorldSystemServiceResponse, WorldSystemTransaction,
    world_system_service_contract_key,
};

/// Maximum read/continuation rounds permitted for one System evaluation.
pub const MAX_WORLD_SYSTEM_SERVICE_ROUNDS: usize = 8;
/// Maximum distinct snapshot reads permitted for one System evaluation.
pub const MAX_WORLD_SYSTEM_SERVICE_READS: usize = 256;
const MAX_SYSTEM_DIAGNOSTIC_BYTES: usize = 8 * 1024;

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
            let envelope = WorldSystemServiceRequest {
                world_id: snapshot.world_id(),
                snapshot_position: snapshot.position(),
                command: encode_command(command),
                reads: reads.clone(),
            };
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
                WorldSystemServiceResponse::Transaction { transaction } => {
                    return decode_transaction(transaction);
                }
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

fn encode_command(command: &WorldCommand) -> WorldSystemCommand {
    WorldSystemCommand {
        id: command.id(),
        schema: command.schema().clone(),
        principal: command.principal(),
        actor: match command.actor() {
            ActorRef::Principal(principal) => WorldSystemActor::Principal(principal),
            ActorRef::Entity(entity) => WorldSystemActor::Entity(entity),
        },
        expected_position: command.expected_position(),
        causation: command.causation().map(|causation| match causation {
            CausationRef::Command(id) => WorldSystemCausation::Command(id),
            CausationRef::Event(id) => WorldSystemCausation::Event(id),
            CausationRef::Effect(id) => WorldSystemCausation::Effect(id),
        }),
        correlation_id: command.correlation_id(),
        effective_at: command.effective_at_timestamp(),
        payload: command.payload().clone(),
    }
}

fn resolve_read(
    snapshot: &WorldSnapshot,
    request: WorldSystemReadRequest,
) -> SystemResult<WorldSystemReadResult> {
    match request {
        WorldSystemReadRequest::Entity { entity_id } => Ok(WorldSystemReadResult::Entity {
            entity_id,
            value: snapshot.load_entity(entity_id)?.map(encode_entity),
        }),
        WorldSystemReadRequest::Relation { relation_id } => Ok(WorldSystemReadResult::Relation {
            relation_id,
            value: snapshot.load_relation(relation_id)?.map(encode_relation),
        }),
        WorldSystemReadRequest::Facet { target, schema } => {
            let target = decode_facet_target(target);
            let value = snapshot.load_facet(target, &schema)?.map(encode_facet);
            Ok(WorldSystemReadResult::Facet {
                target: encode_facet_target(target),
                schema,
                value,
            })
        }
    }
}

fn encode_entity(entity: EntityRecord) -> WorldSystemEntityRecord {
    WorldSystemEntityRecord {
        id: entity.id(),
        schema: entity.schema().clone(),
    }
}

fn encode_relation(relation: RelationRecord) -> WorldSystemRelationRecord {
    WorldSystemRelationRecord {
        id: relation.id(),
        schema: relation.schema().clone(),
        from: relation.from(),
        to: relation.to(),
    }
}

fn encode_facet(facet: FacetRecord) -> WorldSystemFacetRecord {
    WorldSystemFacetRecord {
        target: encode_facet_target(facet.target()),
        schema: facet.schema().clone(),
        payload: facet.payload().clone(),
    }
}

fn encode_facet_target(target: FacetTarget) -> WorldSystemFacetTarget {
    match target {
        FacetTarget::World => WorldSystemFacetTarget::World,
        FacetTarget::Entity(entity) => WorldSystemFacetTarget::Entity(entity),
        FacetTarget::Relation(relation) => WorldSystemFacetTarget::Relation(relation),
    }
}

fn decode_facet_target(target: WorldSystemFacetTarget) -> FacetTarget {
    match target {
        WorldSystemFacetTarget::World => FacetTarget::World,
        WorldSystemFacetTarget::Entity(entity) => FacetTarget::Entity(entity),
        WorldSystemFacetTarget::Relation(relation) => FacetTarget::Relation(relation),
    }
}

fn decode_transaction(proposal: WorldSystemTransaction) -> SystemResult<WorldTransaction> {
    let mut transaction = WorldTransaction::new();
    for mutation in proposal.mutations {
        transaction.push_mutation(decode_mutation(mutation));
    }
    for WorldSystemEventProposal { schema, payload } in proposal.events {
        transaction.push_event(WorldEventDraft::new(schema, payload));
    }
    for WorldSystemEffectProposal { schema, payload } in proposal.effects {
        transaction.push_effect(EffectJobDraft::new(schema, payload));
    }
    Ok(transaction)
}

fn decode_mutation(mutation: WorldSystemMutation) -> WorldMutation {
    match mutation {
        WorldSystemMutation::CreateEntity { entity_id, schema } => WorldMutation::CreateEntity {
            entity: EntityRecord::new(entity_id, schema),
        },
        WorldSystemMutation::DeleteEntity { entity_id } => {
            WorldMutation::DeleteEntity { entity_id }
        }
        WorldSystemMutation::CreateRelation {
            relation_id,
            schema,
            from,
            to,
        } => WorldMutation::CreateRelation {
            relation: RelationRecord::new(relation_id, schema, from, to),
        },
        WorldSystemMutation::DeleteRelation { relation_id } => {
            WorldMutation::DeleteRelation { relation_id }
        }
        WorldSystemMutation::SetFacet {
            target,
            schema,
            payload,
        } => WorldMutation::SetFacet {
            facet: FacetRecord::new(decode_facet_target(target), schema, payload),
        },
        WorldSystemMutation::RemoveFacet { target, schema } => WorldMutation::RemoveFacet {
            target: decode_facet_target(target),
            schema,
        },
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
