//! Public transport contract for ordinary extension-provided World Systems.
//!
//! These DTOs describe proposals rather than Core storage/runtime internals.
//! Ordinary packages can evaluate commands and propose item/property changes
//! without linking to `rintawa-world`. There is no durable event or effect
//! proposal: a System may only attach best-effort, post-COMMIT runtime signals.

use serde::{Deserialize, Serialize};

use crate::{
    contracts::{ContractKey, ContractVersion},
    world::{CommandId, CorrelationId, ItemId, PrincipalId, SchemaKey, UnixTimeMillis, WorldId},
};

/// Major version of the platform-owned ordinary World System service protocol.
pub const WORLD_SYSTEM_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(2);

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

/// Durable reference to the operation that directly caused a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum WorldSystemCausation {
    /// Another submitted command.
    Command(CommandId),
}

/// Immutable authoritative command envelope visible to an extension System.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemCommand {
    /// Stable command identity allocated by the Host.
    pub id: CommandId,
    /// Exact versioned command schema routed to this System.
    pub schema: SchemaKey,
    /// Authenticated outside principal selected by the Host.
    pub principal: PrincipalId,
    /// Direct causal predecessor when the command is part of another operation.
    pub causation: Option<WorldSystemCausation>,
    /// Stable identity shared by one causal operation chain.
    pub correlation_id: CorrelationId,
    /// Optional semantic effective timestamp.
    pub effective_at: Option<UnixTimeMillis>,
    /// Feature-owned command payload already validated by Core before commit.
    pub payload: serde_json::Value,
}

/// Persistent kind of one item, mirrored from the authoritative `items.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorldSystemItemKind {
    /// An object-item.
    Object,
    /// A relation-item with subject and object endpoints.
    Relation,
}

/// Public item record returned from a pinned World System snapshot read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSystemItemRecord {
    /// Stable item identity.
    pub id: ItemId,
    /// Persistent item kind.
    pub kind: WorldSystemItemKind,
    /// Subject endpoint, present only for relations.
    pub subject: Option<ItemId>,
    /// Object endpoint, present only for relations.
    pub object: Option<ItemId>,
}

impl WorldSystemItemRecord {
    /// Returns the stable item identity.
    pub const fn id(&self) -> ItemId {
        self.id
    }

    /// Returns whether this item is a relation.
    pub const fn is_relation(&self) -> bool {
        matches!(self.kind, WorldSystemItemKind::Relation)
    }
}

/// Public property record returned from a pinned World System snapshot read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemPropertyRecord {
    /// Item that owns the property.
    pub item: ItemId,
    /// Exact versioned property name.
    pub key: SchemaKey,
    /// Validated property value.
    pub data: serde_json::Value,
}

impl WorldSystemPropertyRecord {
    /// Returns the owning item.
    pub const fn item(&self) -> ItemId {
        self.item
    }

    /// Returns the exact versioned property name.
    pub const fn key(&self) -> &SchemaKey {
        &self.key
    }

    /// Returns the property value.
    pub const fn data(&self) -> &serde_json::Value {
        &self.data
    }
}

/// One bounded read requested from the same pinned snapshot used for evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemReadRequest {
    /// Loads one item by stable identity.
    Item {
        /// Item to read.
        item_id: ItemId,
    },
    /// Loads one exact property of one item.
    Property {
        /// Item that owns the property.
        item_id: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
    },
}

/// One read result resolved by the Host from the pinned authoritative snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldSystemReadResult {
    /// Result of an item read.
    Item {
        /// Requested item identity.
        item_id: ItemId,
        /// Current item record, or `None` when absent.
        value: Option<WorldSystemItemRecord>,
    },
    /// Result of a property read.
    Property {
        /// Requested owning item.
        item_id: ItemId,
        /// Requested exact property name.
        key: SchemaKey,
        /// Current property value, or `None` when absent.
        value: Option<WorldSystemPropertyRecord>,
    },
}

/// Stateless evaluation envelope sent to an ordinary extension System provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemServiceRequest {
    /// Authoritative world being evaluated.
    pub world_id: WorldId,
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

    /// Returns the immutable authenticated command envelope.
    pub const fn command(&self) -> &WorldSystemCommand {
        &self.command
    }

    /// Returns results from prior bounded read rounds.
    pub fn reads(&self) -> &[WorldSystemReadResult] {
        &self.reads
    }
}

/// Ordinary current-state change proposed by an extension System.
///
/// Authority changes are absent: control grants do not exist in this model, and
/// extension permissions stay a Host-owned composition concern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "operation")]
pub enum WorldSystemMutation {
    /// Creates one object-item.
    CreateObject {
        /// Stable identity of the object to create.
        item_id: ItemId,
    },
    /// Creates one relation-item between two object-items.
    CreateRelation {
        /// Stable identity of the relation to create.
        item_id: ItemId,
        /// Existing object-item used as the subject.
        subject: ItemId,
        /// Existing object-item used as the object.
        object: ItemId,
    },
    /// Deletes one item, the relations referencing it, and their properties.
    DeleteItem {
        /// Item identity to delete.
        item_id: ItemId,
    },
    /// Inserts or replaces the current value of one property.
    SetProperty {
        /// Item that owns the property.
        item_id: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
        /// Property value validated by Core against the registered definition.
        data: serde_json::Value,
    },
    /// Removes one property when present.
    RemoveProperty {
        /// Item that owns the property.
        item_id: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
    },
}

/// Ephemeral, post-COMMIT runtime signal proposed by an extension System.
///
/// Signals are not persisted, not replayed, and may be lost if the process dies
/// between COMMIT and dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemSignalProposal {
    /// Exact versioned signal schema.
    pub schema: SchemaKey,
    /// Feature-owned signal payload.
    pub payload: serde_json::Value,
}

/// Ordinary extension proposal for one atomic authoritative world transaction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldSystemTransaction {
    /// Current-state changes in application order.
    pub mutations: Vec<WorldSystemMutation>,
    /// Best-effort runtime signals drained after COMMIT.
    pub signals: Vec<WorldSystemSignalProposal>,
}

impl WorldSystemTransaction {
    /// Creates an empty proposal. Core rejects it if no change is added.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one current-state change.
    pub fn push_mutation(&mut self, mutation: WorldSystemMutation) {
        self.mutations.push(mutation);
    }

    /// Adds one best-effort runtime signal.
    pub fn push_signal(&mut self, signal: WorldSystemSignalProposal) {
        self.signals.push(signal);
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
    fn test_should_round_trip_item_and_property_proposal() -> anyhow::Result<()> {
        let alice = ItemId::new();
        let forest = ItemId::new();
        let mut transaction = WorldSystemTransaction::new();
        transaction.push_mutation(WorldSystemMutation::CreateObject { item_id: alice });
        transaction.push_mutation(WorldSystemMutation::CreateRelation {
            item_id: ItemId::new(),
            subject: alice,
            object: forest,
        });
        transaction.push_mutation(WorldSystemMutation::SetProperty {
            item_id: alice,
            key: "player.identity@1".parse()?,
            data: serde_json::json!({ "seat": 1 }),
        });
        transaction.push_signal(WorldSystemSignalProposal {
            schema: "narrator.turn@1".parse()?,
            payload: serde_json::json!({}),
        });

        let encoded = serde_json::to_vec(&transaction)?;
        let decoded: WorldSystemTransaction = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, transaction);
        Ok(())
    }
}
