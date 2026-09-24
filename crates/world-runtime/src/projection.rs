//! Policy-filtered world projections produced by extension-owned services.

use std::{collections::HashSet, sync::Arc};

use rintawa_sdk::{
    contracts::{ContractKey, ContractVersion},
    services::{ServiceCallError, ServiceCallResult},
    types::ExtensionId,
    world::{EntityId, PrincipalId, RelationId, SchemaKey, WorldId},
};
use rintawa_world::{
    EntityRecord, FacetRecord, FacetTarget, RelationRecord, SchemaKind, SchemaRegistry, WorldError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{WorldReadError, WorldSnapshot};

/// Major version of the platform World Projection service protocol.
pub const WORLD_PROJECTION_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(1);
/// Maximum continuation rounds permitted for one projection evaluation.
pub const MAX_WORLD_PROJECTION_SERVICE_ROUNDS: usize = 8;
/// Maximum distinct authoritative reads permitted for one projection evaluation.
pub const MAX_WORLD_PROJECTION_SERVICE_READS: usize = 256;
/// Maximum serialized feature input accepted before invoking a projection provider.
pub const MAX_WORLD_PROJECTION_INPUT_BYTES: usize = 64 * 1024;
/// Maximum provider diagnostic size accepted by the projection protocol.
pub const MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES: usize = 8 * 1024;

/// Returns the platform service contract for one exact projection schema.
pub fn world_projection_service_contract_key(projection_schema: &SchemaKey) -> ContractKey {
    ContractKey::new(
        format!(
            "rintawa.world.projection.{}.v{}",
            projection_schema.id(),
            projection_schema.version().get()
        ),
        WORLD_PROJECTION_SERVICE_PROTOCOL_VERSION,
    )
}

/// One bounded authoritative read requested while building a filtered projection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldProjectionReadRequest {
    /// Loads one entity only when its exact schema is owned by the projection extension.
    Entity {
        /// Entity to read.
        entity_id: EntityId,
    },
    /// Loads one relation only when its exact schema is owned by the projection extension.
    Relation {
        /// Relation to read.
        relation_id: RelationId,
    },
    /// Loads one facet only when its exact schema is owned by the projection extension.
    Facet {
        /// Target carrying the facet.
        target: FacetTarget,
        /// Exact versioned facet schema.
        schema: SchemaKey,
    },
    /// Checks whether the fixed projection Principal controls one entity for one command.
    ///
    /// The command schema must be owned by the projection extension. The Principal is
    /// never supplied by the provider and therefore cannot be substituted in a read request.
    CanControl {
        /// Entity actor whose current grant should be checked.
        actor_entity: EntityId,
        /// Exact command schema whose grant scope is required.
        command_schema: SchemaKey,
    },
}

/// One host-resolved read result returned to the projection provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldProjectionReadResult {
    /// Result of an owner-filtered entity read.
    Entity {
        /// Requested entity identity.
        entity_id: EntityId,
        /// Entity value, or `None` when absent or owned by another extension.
        value: Option<EntityRecord>,
    },
    /// Result of an owner-filtered relation read.
    Relation {
        /// Requested relation identity.
        relation_id: RelationId,
        /// Relation value, or `None` when absent or owned by another extension.
        value: Option<RelationRecord>,
    },
    /// Result of an owner-filtered facet read.
    Facet {
        /// Requested state target.
        target: FacetTarget,
        /// Requested exact facet schema.
        schema: SchemaKey,
        /// Facet value, or `None` when absent or owned by another extension.
        value: Option<FacetRecord>,
    },
    /// Result of an authoritative ControlGrant check for the fixed Principal.
    CanControl {
        /// Entity actor checked by the host.
        actor_entity: EntityId,
        /// Exact command schema checked by the host.
        command_schema: SchemaKey,
        /// Whether a non-expired matching ControlGrant exists at the pinned position.
        allowed: bool,
    },
}

/// Stateless continuation envelope sent to one extension-owned projection provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldProjectionServiceRequest {
    world_id: WorldId,
    snapshot_position: u64,
    projection_schema: SchemaKey,
    principal: PrincipalId,
    input: serde_json::Value,
    reads: Vec<WorldProjectionReadResult>,
}

impl WorldProjectionServiceRequest {
    fn new(
        world_id: WorldId,
        snapshot_position: u64,
        projection_schema: SchemaKey,
        principal: PrincipalId,
        input: serde_json::Value,
        reads: Vec<WorldProjectionReadResult>,
    ) -> Self {
        Self {
            world_id,
            snapshot_position,
            projection_schema,
            principal,
            input,
            reads,
        }
    }

    /// Returns the authoritative world being projected.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact pinned position used for every read in this evaluation.
    pub const fn snapshot_position(&self) -> u64 {
        self.snapshot_position
    }

    /// Returns the exact projection schema requested by the caller.
    pub const fn projection_schema(&self) -> &SchemaKey {
        &self.projection_schema
    }

    /// Returns the authenticated audience Principal fixed by the host caller.
    pub const fn principal(&self) -> PrincipalId {
        self.principal
    }

    /// Returns extension-defined bounded projection input.
    pub const fn input(&self) -> &serde_json::Value {
        &self.input
    }

    /// Returns every host-resolved read from earlier continuation rounds.
    pub fn reads(&self) -> &[WorldProjectionReadResult] {
        &self.reads
    }
}

/// Response produced by one projection service invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldProjectionServiceResponse {
    /// Projection evaluation finished with a policy-filtered view model.
    Complete {
        /// View payload that must validate against the registered Projection schema.
        value: serde_json::Value,
    },
    /// Evaluation needs additional owner-filtered reads from the same pinned snapshot.
    Read {
        /// New reads required before the provider can complete the view.
        requests: Vec<WorldProjectionReadRequest>,
    },
    /// Projection policy deliberately denies this request.
    Rejected {
        /// Sanitized, bounded diagnostic suitable for caller-visible failure.
        reason: String,
    },
    /// The provider could not build a projection.
    Failed {
        /// Sanitized, bounded diagnostic suitable for host logs.
        reason: String,
    },
}

/// Transport used by one service-backed projection implementation.
pub trait WorldProjectionServiceClient: Send + Sync + 'static {
    /// Calls one exact platform projection service contract.
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

/// One immutable policy-filtered view produced at an authoritative world position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldProjectionView {
    world_id: WorldId,
    snapshot_position: u64,
    schema: SchemaKey,
    principal: PrincipalId,
    value: serde_json::Value,
}

impl WorldProjectionView {
    fn new(
        world_id: WorldId,
        snapshot_position: u64,
        schema: SchemaKey,
        principal: PrincipalId,
        value: serde_json::Value,
    ) -> Self {
        Self {
            world_id,
            snapshot_position,
            schema,
            principal,
            value,
        }
    }

    /// Returns the authoritative world represented by this view.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact world position used to build this view.
    pub const fn snapshot_position(&self) -> u64 {
        self.snapshot_position
    }

    /// Returns the exact versioned projection schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the authenticated audience Principal used by projection policy.
    pub const fn principal(&self) -> PrincipalId {
        self.principal
    }

    /// Returns the schema-validated feature-owned view payload.
    pub const fn value(&self) -> &serde_json::Value {
        &self.value
    }
}

/// Failure while resolving one policy-filtered world projection.
#[derive(Debug, Error)]
pub enum WorldProjectionError {
    /// The projection binding was used with a snapshot from another World.
    #[error("projection service binding belongs to another world")]
    CrossWorldBinding,
    /// The projection schema was not registered in the pinned world snapshot.
    #[error("projection schema {0} is not registered")]
    SchemaUnavailable(SchemaKey),
    /// The bound projection schema does not have Projection semantics.
    #[error("schema {0} is not a projection schema")]
    SchemaKindMismatch(SchemaKey),
    /// Durable schema ownership no longer matches the owner-pinned service binding.
    #[error("projection schema {0} is owned by another extension")]
    SchemaOwnerMismatch(SchemaKey),
    /// Caller input exceeded the host-side projection bound.
    #[error("projection input is {actual_bytes} bytes; maximum is {maximum_bytes}")]
    InputTooLarge {
        /// Serialized input size observed by the host.
        actual_bytes: usize,
        /// Maximum accepted serialized input size.
        maximum_bytes: usize,
    },
    /// Persistent snapshot state could not be read.
    #[error(transparent)]
    Read(#[from] WorldReadError),
    /// Generic service routing or provider execution failed.
    #[error("projection service transport failed: {0}")]
    Transport(#[from] ServiceCallError),
    /// Host request serialization unexpectedly failed.
    #[error("failed to encode projection service request")]
    RequestEncode(#[source] serde_json::Error),
    /// Provider returned bytes that are not a valid projection response.
    #[error("invalid projection service response")]
    ResponseDecode(#[source] serde_json::Error),
    /// Provider requested no reads in a continuation round.
    #[error("projection service requested an empty read round")]
    EmptyReadRound,
    /// Provider requested the same pinned read more than once.
    #[error("projection service repeated a resolved snapshot read")]
    RepeatedRead,
    /// Provider exceeded the total read budget for one projection.
    #[error("projection service exceeded the snapshot read limit")]
    ReadLimitExceeded,
    /// Provider exhausted the continuation-round budget without completing.
    #[error("projection service exceeded the evaluation round limit")]
    RoundLimitExceeded,
    /// Provider returned a diagnostic above the protocol bound.
    #[error("projection service returned an oversized diagnostic")]
    DiagnosticTooLarge,
    /// Projection policy deliberately rejected the caller.
    #[error("projection rejected: {0}")]
    Rejected(String),
    /// Projection provider failed to produce a view.
    #[error("projection failed: {0}")]
    Failed(String),
    /// Provider output failed its registered Projection schema.
    #[error("projection output failed schema validation")]
    InvalidOutput(#[source] WorldError),
}

/// Result type used by policy-filtered projection evaluation.
pub type WorldProjectionResult<T> = Result<T, WorldProjectionError>;

/// Extension-owned projection evaluator bound to one exact world/schema/owner tuple.
pub struct ServiceWorldProjection {
    world_id: WorldId,
    projection_schema: SchemaKey,
    projection_owner: ExtensionId,
    contract: ContractKey,
    client: Arc<dyn WorldProjectionServiceClient>,
}

impl ServiceWorldProjection {
    /// Creates one owner-pinned projection evaluator.
    pub fn new<C>(
        world_id: WorldId,
        projection_schema: SchemaKey,
        projection_owner: ExtensionId,
        client: C,
    ) -> Self
    where
        C: WorldProjectionServiceClient,
    {
        let contract = world_projection_service_contract_key(&projection_schema);
        Self {
            world_id,
            projection_schema,
            projection_owner,
            contract,
            client: Arc::new(client),
        }
    }

    /// Returns the exact world owned by this projection binding.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact registered Projection schema.
    pub const fn projection_schema(&self) -> &SchemaKey {
        &self.projection_schema
    }

    /// Returns the immutable extension owner used for raw-read filtering.
    pub const fn projection_owner(&self) -> &ExtensionId {
        &self.projection_owner
    }

    /// Returns the platform service contract that supplies this projection.
    pub const fn service_contract(&self) -> &ContractKey {
        &self.contract
    }

    /// Builds one Principal-specific view from a fixed authoritative snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed binding, storage, transport, protocol, policy, or output
    /// validation failure. Raw reads are never exposed directly to the caller.
    pub fn project(
        &self,
        snapshot: &WorldSnapshot,
        principal: PrincipalId,
        input: serde_json::Value,
    ) -> WorldProjectionResult<WorldProjectionView> {
        if snapshot.world_id() != self.world_id {
            return Err(WorldProjectionError::CrossWorldBinding);
        }

        let definition = snapshot
            .load_schema_definition(&self.projection_schema)?
            .ok_or_else(|| {
                WorldProjectionError::SchemaUnavailable(self.projection_schema.clone())
            })?;
        if definition.kind() != SchemaKind::Projection {
            return Err(WorldProjectionError::SchemaKindMismatch(
                self.projection_schema.clone(),
            ));
        }
        if definition.owner() != &self.projection_owner {
            return Err(WorldProjectionError::SchemaOwnerMismatch(
                self.projection_schema.clone(),
            ));
        }

        let input_bytes = serde_json::to_vec(&input)
            .map_err(WorldProjectionError::RequestEncode)?
            .len();
        if input_bytes > MAX_WORLD_PROJECTION_INPUT_BYTES {
            return Err(WorldProjectionError::InputTooLarge {
                actual_bytes: input_bytes,
                maximum_bytes: MAX_WORLD_PROJECTION_INPUT_BYTES,
            });
        }

        let mut output_registry = SchemaRegistry::new();
        output_registry
            .register(definition)
            .map_err(WorldProjectionError::InvalidOutput)?;
        let mut reads = Vec::new();
        let mut seen_reads = HashSet::new();

        for _ in 0..MAX_WORLD_PROJECTION_SERVICE_ROUNDS {
            let envelope = WorldProjectionServiceRequest::new(
                snapshot.world_id(),
                snapshot.position(),
                self.projection_schema.clone(),
                principal,
                input.clone(),
                reads.clone(),
            );
            let request =
                serde_json::to_vec(&envelope).map_err(WorldProjectionError::RequestEncode)?;
            let response = self.client.call(&self.contract, &request)?;
            let response: WorldProjectionServiceResponse =
                serde_json::from_slice(&response).map_err(WorldProjectionError::ResponseDecode)?;

            match response {
                WorldProjectionServiceResponse::Complete { value } => {
                    output_registry
                        .validate_payload(&self.projection_schema, SchemaKind::Projection, &value)
                        .map_err(WorldProjectionError::InvalidOutput)?;
                    return Ok(WorldProjectionView::new(
                        snapshot.world_id(),
                        snapshot.position(),
                        self.projection_schema.clone(),
                        principal,
                        value,
                    ));
                }
                WorldProjectionServiceResponse::Rejected { reason } => {
                    return Err(WorldProjectionError::Rejected(validate_diagnostic(reason)?));
                }
                WorldProjectionServiceResponse::Failed { reason } => {
                    return Err(WorldProjectionError::Failed(validate_diagnostic(reason)?));
                }
                WorldProjectionServiceResponse::Read { requests } => {
                    if requests.is_empty() {
                        return Err(WorldProjectionError::EmptyReadRound);
                    }
                    if reads.len().saturating_add(requests.len())
                        > MAX_WORLD_PROJECTION_SERVICE_READS
                    {
                        return Err(WorldProjectionError::ReadLimitExceeded);
                    }
                    for request in requests {
                        if !seen_reads.insert(request.clone()) {
                            return Err(WorldProjectionError::RepeatedRead);
                        }
                        reads.push(resolve_read(
                            snapshot,
                            principal,
                            &self.projection_owner,
                            request,
                        )?);
                    }
                }
            }
        }

        Err(WorldProjectionError::RoundLimitExceeded)
    }
}

fn resolve_read(
    snapshot: &WorldSnapshot,
    principal: PrincipalId,
    projection_owner: &ExtensionId,
    request: WorldProjectionReadRequest,
) -> WorldProjectionResult<WorldProjectionReadResult> {
    match request {
        WorldProjectionReadRequest::Entity { entity_id } => {
            let value = match snapshot.load_entity(entity_id)? {
                Some(entity)
                    if schema_owned_by(
                        snapshot,
                        entity.schema(),
                        projection_owner,
                        SchemaKind::Entity,
                    )? =>
                {
                    Some(entity)
                }
                _ => None,
            };
            Ok(WorldProjectionReadResult::Entity { entity_id, value })
        }
        WorldProjectionReadRequest::Relation { relation_id } => {
            let value = match snapshot.load_relation(relation_id)? {
                Some(relation)
                    if schema_owned_by(
                        snapshot,
                        relation.schema(),
                        projection_owner,
                        SchemaKind::Relation,
                    )? =>
                {
                    Some(relation)
                }
                _ => None,
            };
            Ok(WorldProjectionReadResult::Relation { relation_id, value })
        }
        WorldProjectionReadRequest::Facet { target, schema } => {
            let value = if schema_owned_by(snapshot, &schema, projection_owner, SchemaKind::Facet)?
            {
                snapshot.load_facet(target, &schema)?
            } else {
                None
            };
            Ok(WorldProjectionReadResult::Facet {
                target,
                schema,
                value,
            })
        }
        WorldProjectionReadRequest::CanControl {
            actor_entity,
            command_schema,
        } => {
            let allowed = if schema_owned_by(
                snapshot,
                &command_schema,
                projection_owner,
                SchemaKind::Command,
            )? {
                snapshot.principal_can_control(principal, actor_entity, &command_schema)?
            } else {
                false
            };
            Ok(WorldProjectionReadResult::CanControl {
                actor_entity,
                command_schema,
                allowed,
            })
        }
    }
}

fn schema_owned_by(
    snapshot: &WorldSnapshot,
    schema: &SchemaKey,
    owner: &ExtensionId,
    kind: SchemaKind,
) -> Result<bool, WorldReadError> {
    Ok(snapshot
        .load_schema_definition(schema)?
        .is_some_and(|definition| definition.kind() == kind && definition.owner() == owner))
}

fn validate_diagnostic(reason: String) -> WorldProjectionResult<String> {
    if reason.len() > MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES {
        return Err(WorldProjectionError::DiagnosticTooLarge);
    }
    Ok(reason)
}
