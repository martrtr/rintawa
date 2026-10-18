//! Public transport contract for ordinary extension-provided World Systems.
//!
//! These DTOs deliberately describe proposals rather than Core storage/runtime
//! internals. Ordinary packages can evaluate commands and propose feature state,
//! events, and durable effects without linking to `rintawa-world`.

use serde::{Deserialize, Serialize};

use crate::{
    contracts::{ContractKey, ContractVersion},
    world::{
        CommandId, CorrelationId, EffectJobId, EntityId, PrincipalId, RelationId, SchemaKey,
        UnixTimeMillis, WorldEventId, WorldId,
    },
};

/// Major version of the platform-owned ordinary World System service protocol.
pub const WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(1);

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

/// Role on whose behalf an authoritative command executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum WorldSystemActor {
    /// The authenticated outside principal acts directly.
    Principal(PrincipalId),
    /// The authenticated principal acts through one world entity.
    Entity(EntityId),
}

/// Durable reference to the operation that directly caused a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum WorldSystemCausation {
    /// Another submitted command.
    Command(CommandId),
    /// A previously committed world event.
    Event(WorldEventId),
    /// A durable external effect result.
    Effect(EffectJobId),
}

/// Immutable authoritative command envelope visible to an extension System.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemCommand {
    /// Stable command idempotency identity allocated by the Host.
    pub id: CommandId,
    /// Exact versioned command schema routed to this System.
    pub schema: SchemaKey,
    /// Authenticated outside principal selected by the Host.
    pub principal: PrincipalId,
    /// Role on whose behalf the command executes.
    pub actor: WorldSystemActor,
    /// Optional optimistic-concurrency position supplied by the caller.
    pub expected_position: Option<u64>,
    /// Direct causal predecessor when the command is part of another operation.
    pub causation: Option<WorldSystemCausation>,
    /// Stable identity shared by one causal operation chain.
    pub correlation_id: CorrelationId,
    /// Optional semantic effective timestamp.
    pub effective_at: Option<UnixTimeMillis>,
    /// Feature-owned command payload already validated by Core before commit.
    pub payload: serde_json::Value,
}

/// Public entity record returned from a pinned World System snapshot read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSystemEntityRecord {
    /// Stable entity identity.
    pub id: EntityId,
    /// Exact versioned entity-type schema.
    pub schema: SchemaKey,
}

impl WorldSystemEntityRecord {
    /// Returns the stable entity identity.
    pub const fn id(&self) -> EntityId {
        self.id
    }

    /// Returns the exact versioned entity-type schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }
}

/// Public relation record returned from a pinned World System snapshot read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSystemRelationRecord {
    /// Stable relation identity.
    pub id: RelationId,
    /// Exact versioned relation-type schema.
    pub schema: SchemaKey,
    /// Source entity.
    pub from: EntityId,
    /// Destination entity.
    pub to: EntityId,
}

impl WorldSystemRelationRecord {
    /// Returns the stable relation identity.
    pub const fn id(&self) -> RelationId {
        self.id
    }

    /// Returns the exact versioned relation schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the source entity.
    pub const fn from(&self) -> EntityId {
        self.from
    }

    /// Returns the destination entity.
    pub const fn to(&self) -> EntityId {
        self.to
    }
}

/// Target that owns one extension-defined facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum WorldSystemFacetTarget {
    /// The world session itself.
    World,
    /// One entity.
    Entity(EntityId),
    /// One relation.
    Relation(RelationId),
}

/// Public facet record returned from a pinned World System snapshot read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemFacetRecord {
    /// State target carrying the facet.
    pub target: WorldSystemFacetTarget,
    /// Exact versioned facet schema.
    pub schema: SchemaKey,
    /// Validated feature-owned facet payload.
    pub payload: serde_json::Value,
}

impl WorldSystemFacetRecord {
    /// Returns the state target carrying this facet.
    pub const fn target(&self) -> WorldSystemFacetTarget {
        self.target
    }

    /// Returns the exact versioned facet schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the validated feature-owned facet payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
}

/// One bounded read requested from the same pinned snapshot used for evaluation.
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
        target: WorldSystemFacetTarget,
        /// Exact versioned facet schema.
        schema: SchemaKey,
    },
}

/// One read result resolved by the Host from the pinned authoritative snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemReadResult {
    /// Result of an entity read.
    Entity {
        /// Requested entity identity.
        entity_id: EntityId,
        /// Current entity record, or `None` when absent.
        value: Option<WorldSystemEntityRecord>,
    },
    /// Result of a relation read.
    Relation {
        /// Requested relation identity.
        relation_id: RelationId,
        /// Current relation record, or `None` when absent.
        value: Option<WorldSystemRelationRecord>,
    },
    /// Result of a facet read.
    Facet {
        /// Requested state target.
        target: WorldSystemFacetTarget,
        /// Requested exact facet schema.
        schema: SchemaKey,
        /// Current facet record, or `None` when absent.
        value: Option<WorldSystemFacetRecord>,
    },
}

/// Stateless evaluation envelope sent to an ordinary extension System provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemServiceRequest {
    /// Authoritative world being evaluated.
    pub world_id: WorldId,
    /// Exact pinned state position used for every read in this evaluation.
    pub snapshot_position: u64,
    /// Immutable authenticated command envelope.
    pub command: WorldSystemCommand,
    /// Results of prior bounded read rounds.
    pub reads: Vec<WorldSystemReadResult>,
}

impl WorldSystemServiceRequest {
    /// Returns the authoritative world being evaluated.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact pinned state position.
    pub const fn snapshot_position(&self) -> u64 {
        self.snapshot_position
    }

    /// Returns the immutable authenticated command envelope.
    pub const fn command(&self) -> &WorldSystemCommand {
        &self.command
    }

    /// Returns results from prior bounded read rounds.
    pub fn reads(&self) -> &[WorldSystemReadResult] {
        &self.reads
    }
}

/// Ordinary current-state mutation proposed by an extension System.
///
/// Authority mutations are intentionally absent. Durable ControlGrant ownership
/// remains a Host-privileged Core operation rather than a package transport feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "operation")]
pub enum WorldSystemMutation {
    /// Creates one typed entity.
    CreateEntity {
        /// Stable identity of the entity to create.
        entity_id: EntityId,
        /// Exact versioned entity-type schema.
        schema: SchemaKey,
    },
    /// Deletes one entity and Core-owned dependent current state.
    DeleteEntity {
        /// Entity identity to delete.
        entity_id: EntityId,
    },
    /// Creates one typed directed relation.
    CreateRelation {
        /// Stable identity of the relation to create.
        relation_id: RelationId,
        /// Exact versioned relation schema.
        schema: SchemaKey,
        /// Source entity.
        from: EntityId,
        /// Destination entity.
        to: EntityId,
    },
    /// Deletes one relation and its current facets.
    DeleteRelation {
        /// Relation identity to delete.
        relation_id: RelationId,
    },
    /// Inserts or replaces one extension-owned facet.
    SetFacet {
        /// Target carrying the facet.
        target: WorldSystemFacetTarget,
        /// Exact versioned facet schema.
        schema: SchemaKey,
        /// Feature-owned JSON payload validated by Core before commit.
        payload: serde_json::Value,
    },
    /// Removes one extension-owned facet when present.
    RemoveFacet {
        /// Target carrying the facet.
        target: WorldSystemFacetTarget,
        /// Exact versioned facet schema.
        schema: SchemaKey,
    },
}

/// Semantic durable event proposed by an extension System.
///
/// The Host allocates the durable `WorldEventId` while converting the proposal to
/// an authoritative Core transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemEventProposal {
    /// Exact versioned event schema.
    pub schema: SchemaKey,
    /// Feature-owned event payload.
    pub payload: serde_json::Value,
}

/// Durable external effect proposed by an extension System.
///
/// The Host allocates the durable `EffectJobId`, which remains the external
/// idempotency identity for the committed job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemEffectProposal {
    /// Exact versioned effect schema.
    pub schema: SchemaKey,
    /// Feature-owned effect payload.
    pub payload: serde_json::Value,
}

/// Ordinary extension proposal for one atomic authoritative world transaction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemTransaction {
    /// Current-state mutations in application order.
    pub mutations: Vec<WorldSystemMutation>,
    /// Durable semantic events in event order.
    pub events: Vec<WorldSystemEventProposal>,
    /// Durable external effects in outbox order.
    pub effects: Vec<WorldSystemEffectProposal>,
}

impl WorldSystemTransaction {
    /// Creates an empty proposal. Core rejects it if no output is added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one ordinary state mutation.
    pub fn push_mutation(&mut self, mutation: WorldSystemMutation) {
        self.mutations.push(mutation);
    }

    /// Adds one semantic durable event proposal.
    pub fn push_event(&mut self, event: WorldSystemEventProposal) {
        self.events.push(event);
    }

    /// Adds one durable external effect proposal.
    pub fn push_effect(&mut self, effect: WorldSystemEffectProposal) {
        self.effects.push(effect);
    }
}

/// Response produced by one ordinary extension System service invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemServiceResponse {
    /// Evaluation finished with a proposal still subject to authoritative validation.
    Transaction {
        /// Ordinary extension transaction proposal.
        transaction: WorldSystemTransaction,
    },
    /// Evaluation needs additional data from the same pinned snapshot.
    Read {
        /// New distinct reads required before the provider can finish evaluation.
        requests: Vec<WorldSystemReadRequest>,
    },
    /// Domain rules deliberately rejected the command.
    Rejected {
        /// Bounded non-secret diagnostic suitable for user-visible failure.
        reason: String,
    },
    /// The provider could not evaluate the command.
    Failed {
        /// Bounded non-secret diagnostic suitable for Host logs.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_round_trip_ordinary_transaction_without_authority_mutations()
    -> anyhow::Result<()> {
        let entity = EntityId::new();
        let mut transaction = WorldSystemTransaction::new();
        transaction.push_mutation(WorldSystemMutation::CreateEntity {
            entity_id: entity,
            schema: "example.entity@1".parse()?,
        });
        transaction.push_mutation(WorldSystemMutation::SetFacet {
            target: WorldSystemFacetTarget::Entity(entity),
            schema: "example.identity@1".parse()?,
            payload: serde_json::json!({ "name": "Alice" }),
        });
        let encoded = serde_json::to_vec(&transaction)?;
        let decoded: WorldSystemTransaction = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, transaction);
        Ok(())
    }
}
