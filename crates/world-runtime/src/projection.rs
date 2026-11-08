//! Service-backed policy-filtered World Projections.

use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

use rintawa_sdk::{
    contracts::ContractKey,
    services::ServiceCallResult,
    world::{PrincipalId, SchemaId, SchemaKey, WorldId},
    world_projection::{
        WorldProjectionReadRequest, WorldProjectionReadResult, WorldProjectionServiceRequest,
        WorldProjectionServiceResponse, WorldProjectionView, world_projection_service_contract_key,
    },
};
use thiserror::Error;

use crate::WorldSnapshot;
use crate::service::{encode_item, encode_property};

pub use rintawa_sdk::world_projection::{
    MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES, MAX_WORLD_PROJECTION_INPUT_BYTES,
    WORLD_PROJECTION_SERVICE_PROTOCOL_VERSION,
};

/// Maximum read/continuation rounds permitted for one projection evaluation.
pub const MAX_WORLD_PROJECTION_SERVICE_ROUNDS: usize = 8;
/// Maximum distinct snapshot reads permitted for one projection evaluation.
pub const MAX_WORLD_PROJECTION_SERVICE_READS: usize = 256;

/// Transport used by a service-backed projection provider.
pub trait WorldProjectionServiceClient: Send + Sync + 'static {
    /// Calls one exact platform service contract.
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>>;
}

impl<F> WorldProjectionServiceClient for F
where
    F: Fn(&ContractKey, &[u8]) -> ServiceCallResult<Vec<u8>> + Send + Sync + 'static,
{
    fn call(&self, contract: &ContractKey, request: &[u8]) -> ServiceCallResult<Vec<u8>> {
        self(contract, request)
    }
}

/// Errors returned while building one policy-filtered projection.
#[derive(Debug, Error)]
pub enum WorldProjectionError {
    /// Snapshot state required by the provider could not be read.
    #[error(transparent)]
    Read(#[from] crate::WorldReadError),

    /// Projection policy deliberately denies the request.
    #[error("projection rejected: {0}")]
    Rejected(String),

    /// The provider could not build a projection.
    #[error("projection failed: {0}")]
    Failed(String),
}

/// Result type used by projection evaluation.
pub type WorldProjectionResult<T> = Result<T, WorldProjectionError>;

/// Projection provider bound to one world and one exact projection schema.
pub struct ServiceWorldProjection {
    world_id: WorldId,
    projection_schema: SchemaKey,
    contract: ContractKey,
    owned_properties: Arc<BTreeSet<SchemaId>>,
    client: Arc<dyn WorldProjectionServiceClient>,
}

impl ServiceWorldProjection {
    /// Creates a projection provider that may read only the listed property names.
    pub fn new<C>(
        world_id: WorldId,
        projection_schema: SchemaKey,
        owned_properties: impl IntoIterator<Item = SchemaId>,
        client: C,
    ) -> Self
    where
        C: WorldProjectionServiceClient,
    {
        Self::from_shared(
            world_id,
            projection_schema,
            owned_properties,
            Arc::new(client),
        )
    }

    /// Creates a projection provider from one shared transport client.
    pub fn from_shared(
        world_id: WorldId,
        projection_schema: SchemaKey,
        owned_properties: impl IntoIterator<Item = SchemaId>,
        client: Arc<dyn WorldProjectionServiceClient>,
    ) -> Self {
        let contract = world_projection_service_contract_key(&projection_schema);
        Self {
            world_id,
            projection_schema,
            contract,
            owned_properties: Arc::new(owned_properties.into_iter().collect()),
            client,
        }
    }

    /// Returns the platform contract that must be provided in the world scope.
    pub const fn service_contract(&self) -> &ContractKey {
        &self.contract
    }

    /// Builds one projection for a fixed Principal at the snapshot of the caller.
    ///
    /// # Errors
    ///
    /// Returns a read, rejection or provider failure.
    pub fn project(
        &self,
        snapshot: &WorldSnapshot,
        principal: PrincipalId,
        input: serde_json::Value,
    ) -> WorldProjectionResult<WorldProjectionView> {
        let input_size = serde_json::to_vec(&input)
            .map_err(|_| WorldProjectionError::Failed(String::from("invalid input")))?
            .len();
        if input_size > MAX_WORLD_PROJECTION_INPUT_BYTES {
            return Err(WorldProjectionError::Rejected(String::from(
                "projection input is too large",
            )));
        }

        let mut reads = Vec::new();
        let mut seen = HashSet::new();

        for _ in 0..MAX_WORLD_PROJECTION_SERVICE_ROUNDS {
            let request = WorldProjectionServiceRequest {
                world_id: self.world_id,
                projection_schema: self.projection_schema.clone(),
                principal,
                input: input.clone(),
                reads: reads.clone(),
            };
            let encoded = serde_json::to_vec(&request)
                .map_err(|_| WorldProjectionError::Failed(String::from("encode failed")))?;
            let response = self
                .client
                .call(&self.contract, &encoded)
                .map_err(|error| {
                    WorldProjectionError::Failed(format!("projection transport failed: {error}"))
                })?;
            let response: WorldProjectionServiceResponse = serde_json::from_slice(&response)
                .map_err(|_| {
                    WorldProjectionError::Failed(String::from("invalid projection response"))
                })?;

            match response {
                WorldProjectionServiceResponse::Complete { value } => {
                    return Ok(WorldProjectionView::new(
                        self.world_id,
                        self.projection_schema.clone(),
                        principal,
                        value,
                    ));
                }
                WorldProjectionServiceResponse::Rejected { reason } => {
                    return Err(WorldProjectionError::Rejected(bounded(reason)?));
                }
                WorldProjectionServiceResponse::Failed { reason } => {
                    return Err(WorldProjectionError::Failed(bounded(reason)?));
                }
                WorldProjectionServiceResponse::Read { requests } => {
                    if requests.is_empty() {
                        return Err(WorldProjectionError::Failed(String::from(
                            "projection requested an empty read round",
                        )));
                    }
                    if reads.len().saturating_add(requests.len())
                        > MAX_WORLD_PROJECTION_SERVICE_READS
                    {
                        return Err(WorldProjectionError::Failed(String::from(
                            "projection exceeded the snapshot read limit",
                        )));
                    }
                    for request in requests {
                        if !seen.insert(request.clone()) {
                            return Err(WorldProjectionError::Failed(String::from(
                                "projection repeated a resolved snapshot read",
                            )));
                        }
                        reads.push(self.resolve_read(snapshot, request)?);
                    }
                }
            }
        }

        Err(WorldProjectionError::Failed(String::from(
            "projection exceeded the evaluation round limit",
        )))
    }

    fn resolve_read(
        &self,
        snapshot: &WorldSnapshot,
        request: WorldProjectionReadRequest,
    ) -> WorldProjectionResult<WorldProjectionReadResult> {
        match request {
            WorldProjectionReadRequest::Item { item_id } => Ok(WorldProjectionReadResult::Item {
                item_id,
                value: snapshot.load_item(item_id)?.map(encode_item),
            }),
            WorldProjectionReadRequest::Property { item_id, key } => {
                let value = if self.owned_properties.contains(key.id()) {
                    snapshot.load_property(item_id, &key)?.map(encode_property)
                } else {
                    None
                };
                Ok(WorldProjectionReadResult::Property {
                    item_id,
                    key,
                    value,
                })
            }
        }
    }
}

fn bounded(reason: String) -> WorldProjectionResult<String> {
    if reason.len() > MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES {
        return Err(WorldProjectionError::Failed(String::from(
            "projection returned an oversized diagnostic",
        )));
    }
    Ok(reason)
}
