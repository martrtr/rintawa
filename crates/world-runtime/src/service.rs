//! Service-backed World System protocol for ordinary extension providers.

use std::{collections::HashSet, sync::Arc};

use rintawa_sdk::{
    contracts::ContractKey,
    services::ServiceCallResult,
    world::{SchemaKey, WorldId},
    world_system::{
        WorldSystemCausation, WorldSystemCommand, WorldSystemItemKind, WorldSystemItemRecord,
        WorldSystemMutation, WorldSystemPropertyRecord, WorldSystemSignalProposal,
    },
};
use rintawa_world::{
    CausationRef, ItemRecord, PropertyRecord, StateChange, WorldCommand, WorldTransaction,
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
        causation: command.causation().map(|causation| match causation {
            CausationRef::Command(id) => WorldSystemCausation::Command(id),
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
        WorldSystemReadRequest::Item { item_id } => Ok(WorldSystemReadResult::Item {
            item_id,
            value: snapshot.load_item(item_id)?.map(encode_item),
        }),
        WorldSystemReadRequest::Property { item_id, key } => {
            let value = snapshot.load_property(item_id, &key)?.map(encode_property);
            Ok(WorldSystemReadResult::Property {
                item_id,
                key,
                value,
            })
        }
    }
}

pub(crate) fn encode_item(item: ItemRecord) -> WorldSystemItemRecord {
    WorldSystemItemRecord {
        id: item.id(),
        kind: match item.kind() {
            rintawa_world::ItemKind::Object => WorldSystemItemKind::Object,
            rintawa_world::ItemKind::Relation => WorldSystemItemKind::Relation,
        },
        subject: item.subject(),
        object: item.object_target(),
    }
}

pub(crate) fn encode_property(property: PropertyRecord) -> WorldSystemPropertyRecord {
    WorldSystemPropertyRecord {
        item: property.item(),
        key: property.key().clone(),
        data: property.data().clone(),
    }
}

fn decode_transaction(proposal: WorldSystemTransaction) -> SystemResult<WorldTransaction> {
    let mut transaction = WorldTransaction::new();
    for mutation in proposal.mutations {
        transaction.push_change(decode_mutation(mutation));
    }
    for WorldSystemSignalProposal { schema, payload } in proposal.signals {
        transaction.push_signal(rintawa_world::RuntimeSignal::new(schema, payload));
    }
    Ok(transaction)
}

fn decode_mutation(mutation: WorldSystemMutation) -> StateChange {
    match mutation {
        WorldSystemMutation::CreateObject { item_id } => StateChange::CreateObject { id: item_id },
        WorldSystemMutation::CreateRelation {
            item_id,
            subject,
            object,
        } => StateChange::CreateRelation {
            id: item_id,
            subject,
            object,
        },
        WorldSystemMutation::DeleteItem { item_id } => StateChange::DeleteItem { id: item_id },
        WorldSystemMutation::SetProperty { item_id, key, data } => StateChange::SetProperty {
            item: item_id,
            key,
            data,
        },
        WorldSystemMutation::RemoveProperty { item_id, key } => {
            StateChange::RemoveProperty { item: item_id, key }
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
