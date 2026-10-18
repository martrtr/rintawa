//! Generic current-state records and mutations for authoritative worlds.

use rintawa_sdk::world::{ControlGrantId, EntityId, RelationId, SchemaKey};

use crate::ControlGrant;
use serde::{Deserialize, Serialize};

/// One typed entity in authoritative current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRecord {
    id: EntityId,
    schema: SchemaKey,
}

impl EntityRecord {
    /// Creates an entity record.
    pub const fn new(id: EntityId, schema: SchemaKey) -> Self {
        Self { id, schema }
    }

    /// Returns the stable entity identity.
    pub const fn id(&self) -> EntityId {
        self.id
    }

    /// Returns the versioned entity-type schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }
}

/// One typed directed relation between two entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationRecord {
    id: RelationId,
    schema: SchemaKey,
    from: EntityId,
    to: EntityId,
}

impl RelationRecord {
    /// Creates a directed relation record.
    pub const fn new(id: RelationId, schema: SchemaKey, from: EntityId, to: EntityId) -> Self {
        Self {
            id,
            schema,
            from,
            to,
        }
    }

    /// Returns the stable relation identity.
    pub const fn id(&self) -> RelationId {
        self.id
    }

    /// Returns the versioned relation-type schema.
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
pub enum FacetTarget {
    /// The world session itself.
    World,
    /// One entity.
    Entity(EntityId),
    /// One relation.
    Relation(RelationId),
}

/// One current facet value attached to a world-state target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacetRecord {
    target: FacetTarget,
    schema: SchemaKey,
    payload: serde_json::Value,
}

impl FacetRecord {
    /// Creates a facet value.
    pub fn new(target: FacetTarget, schema: SchemaKey, payload: serde_json::Value) -> Self {
        Self {
            target,
            schema,
            payload,
        }
    }

    /// Returns the target that owns this facet.
    pub const fn target(&self) -> FacetTarget {
        self.target
    }

    /// Returns the versioned facet schema.
    pub const fn schema(&self) -> &SchemaKey {
        &self.schema
    }

    /// Returns the validated facet payload.
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }
}

/// One generic current-state mutation within an atomic world transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "operation")]
pub enum WorldMutation {
    /// Creates one typed entity.
    CreateEntity {
        /// Entity record to create.
        entity: EntityRecord,
    },
    /// Deletes an entity and current relations/facets owned by it.
    DeleteEntity {
        /// Entity identity to delete.
        entity_id: EntityId,
    },
    /// Creates one typed directed relation.
    CreateRelation {
        /// Relation record to create.
        relation: RelationRecord,
    },
    /// Deletes one relation and its current facets.
    DeleteRelation {
        /// Relation identity to delete.
        relation_id: RelationId,
    },
    /// Inserts or replaces one facet value.
    SetFacet {
        /// Facet value to persist.
        facet: FacetRecord,
    },
    /// Removes one facet value when present.
    RemoveFacet {
        /// Target that owns the facet.
        target: FacetTarget,
        /// Versioned facet schema to remove.
        schema: SchemaKey,
    },
    /// Creates one durable principal-to-entity control grant.
    GrantControl {
        /// Grant to persist.
        grant: ControlGrant,
    },
    /// Revokes one durable control grant.
    RevokeControl {
        /// Grant identity to revoke.
        grant_id: ControlGrantId,
    },
}
