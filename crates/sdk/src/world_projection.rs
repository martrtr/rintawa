//! Public transport contract for extension-owned policy-filtered World Projections.
//!
//! Projection providers receive a Host-fixed Principal plus bounded, owner-filtered
//! snapshot reads. These DTOs deliberately avoid Core storage/runtime types so an
//! ordinary extension can provide a projection without linking to `rintawa-world`.

use serde::{Deserialize, Serialize};

use crate::{
    contracts::{ContractKey, ContractVersion},
    world::{ItemId, PrincipalId, SchemaKey, WorldId},
    world_system::{WorldSystemItemRecord, WorldSystemPropertyRecord},
};

/// Major version of the platform-owned World Projection service protocol.
pub const WORLD_PROJECTION_SERVICE_PROTOCOL_VERSION: ContractVersion = ContractVersion::new(2);
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
    /// Loads one item.
    Item {
        /// Item to read.
        item_id: ItemId,
    },
    /// Loads one property only when its exact versioned name is owned by the
    /// projection extension.
    Property {
        /// Item that owns the property.
        item_id: ItemId,
        /// Exact versioned property name.
        key: SchemaKey,
    },
}

/// One Host-resolved read result returned to the projection provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum WorldProjectionReadResult {
    /// Result of an owner-filtered item read.
    Item {
        /// Requested item identity.
        item_id: ItemId,
        /// Item value, or `None` when absent.
        value: Option<WorldSystemItemRecord>,
    },
    /// Result of an owner-filtered property read.
    Property {
        /// Requested owning item.
        item_id: ItemId,
        /// Requested exact property name.
        key: SchemaKey,
        /// Property value, or `None` when absent or owned by another extension.
        value: Option<WorldSystemPropertyRecord>,
    },
}

/// Stateless continuation envelope sent to one extension-owned projection provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldProjectionServiceRequest {
    /// Authoritative World being projected.
    pub world_id: WorldId,
    /// Exact projection schema requested by the caller.
    pub projection_schema: SchemaKey,
    /// Authenticated audience Principal fixed by the Host.
    pub principal: PrincipalId,
    /// Extension-defined bounded projection input.
    pub input: serde_json::Value,
    /// Host-resolved reads from earlier continuation rounds.
    pub reads: Vec<WorldProjectionReadResult>,
}

impl WorldProjectionServiceRequest {
    /// Returns the authoritative World being projected.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact projection schema requested by the caller.
    pub const fn projection_schema(&self) -> &SchemaKey {
        &self.projection_schema
    }

    /// Returns the authenticated audience Principal fixed by the Host caller.
    pub const fn principal(&self) -> PrincipalId {
        self.principal
    }

    /// Returns extension-defined bounded projection input.
    pub const fn input(&self) -> &serde_json::Value {
        &self.input
    }

    /// Returns every Host-resolved read from earlier continuation rounds.
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
        /// Sanitized, bounded diagnostic suitable for Host logs.
        reason: String,
    },
}

/// One immutable policy-filtered view produced by an extension projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldProjectionView {
    world_id: WorldId,
    schema: SchemaKey,
    principal: PrincipalId,
    value: serde_json::Value,
}

impl WorldProjectionView {
    /// Creates one already-authorized projection view.
    pub fn new(
        world_id: WorldId,
        schema: SchemaKey,
        principal: PrincipalId,
        value: serde_json::Value,
    ) -> Self {
        Self {
            world_id,
            schema,
            principal,
            value,
        }
    }

    /// Returns the authoritative World represented by this view.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{SchemaId, SchemaVersion};

    fn schema(id: &str) -> SchemaKey {
        SchemaKey::new(
            SchemaId::parse(id).expect("test schema id must be valid"),
            SchemaVersion::new(1).expect("test schema version must be valid"),
        )
    }

    #[test]
    fn test_should_round_trip_projection_service_transport() -> anyhow::Result<()> {
        let request = WorldProjectionServiceResponse::Read {
            requests: vec![WorldProjectionReadRequest::Property {
                item_id: ItemId::new(),
                key: schema("example.timeline"),
            }],
        };
        let encoded = serde_json::to_vec(&request)?;
        assert_eq!(
            serde_json::from_slice::<WorldProjectionServiceResponse>(&encoded)?,
            request
        );
        Ok(())
    }
}
