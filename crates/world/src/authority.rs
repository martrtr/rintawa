//! Principal, actor, and durable control-grant primitives.

use std::collections::BTreeSet;

use rintawa_sdk::world::{ControlGrantId, EntityId, PrincipalId, SchemaKey};
use serde::{Deserialize, Serialize};

use crate::{WorldError, WorldResult};

/// Role on whose behalf one world command executes.
///
/// Transport/provider identity is deliberately absent. A principal may act as
/// itself, or may control a world entity through a durable ControlGrant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "id")]
pub enum ActorRef {
    /// The authenticated outside principal acts directly.
    Principal(PrincipalId),
    /// The command acts on behalf of one world entity.
    Entity(EntityId),
}

/// Command-schema scope of one durable control grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "schemas")]
pub enum ControlScope {
    /// Allows every registered command schema.
    AnyCommand,
    /// Allows only the listed exact versioned command schemas.
    ExactSchemas(BTreeSet<SchemaKey>),
}

impl ControlScope {
    /// Creates an exact-schema scope.
    ///
    /// # Errors
    ///
    /// Returns WorldError::EmptyControlScope when no command schemas are supplied.
    pub fn exact(schemas: impl IntoIterator<Item = SchemaKey>) -> WorldResult<Self> {
        let schemas = schemas.into_iter().collect::<BTreeSet<_>>();
        if schemas.is_empty() {
            return Err(WorldError::EmptyControlScope);
        }
        Ok(Self::ExactSchemas(schemas))
    }

    /// Returns exact schemas when this is a restricted scope.
    pub const fn exact_schemas(&self) -> Option<&BTreeSet<SchemaKey>> {
        match self {
            Self::AnyCommand => None,
            Self::ExactSchemas(schemas) => Some(schemas),
        }
    }

    /// Returns whether this scope allows one exact command schema.
    pub fn allows(&self, schema: &SchemaKey) -> bool {
        match self {
            Self::AnyCommand => true,
            Self::ExactSchemas(schemas) => schemas.contains(schema),
        }
    }
}

/// Durable permission for one principal to control one entity actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlGrant {
    id: ControlGrantId,
    principal: PrincipalId,
    actor_entity: EntityId,
    scope: ControlScope,
    valid_through_position: Option<u64>,
}

impl ControlGrant {
    /// Creates an unrestricted control grant with a new grant identity.
    pub fn any(principal: PrincipalId, actor_entity: EntityId) -> Self {
        Self {
            id: ControlGrantId::new(),
            principal,
            actor_entity,
            scope: ControlScope::AnyCommand,
            valid_through_position: None,
        }
    }

    /// Creates a grant with an explicit identity and scope.
    pub const fn with_id(
        id: ControlGrantId,
        principal: PrincipalId,
        actor_entity: EntityId,
        scope: ControlScope,
        valid_through_position: Option<u64>,
    ) -> Self {
        Self {
            id,
            principal,
            actor_entity,
            scope,
            valid_through_position,
        }
    }

    /// Restricts the grant to commands evaluated at or before the given position.
    pub const fn valid_through(mut self, position: u64) -> Self {
        self.valid_through_position = Some(position);
        self
    }

    /// Returns the stable grant identity.
    pub const fn id(&self) -> ControlGrantId {
        self.id
    }

    /// Returns the authenticated principal receiving control.
    pub const fn principal(&self) -> PrincipalId {
        self.principal
    }

    /// Returns the entity actor controlled by the principal.
    pub const fn actor_entity(&self) -> EntityId {
        self.actor_entity
    }

    /// Returns the command scope.
    pub const fn scope(&self) -> &ControlScope {
        &self.scope
    }

    /// Returns the final world position at which the grant is valid.
    pub const fn valid_through_position(&self) -> Option<u64> {
        self.valid_through_position
    }

    /// Returns whether this grant authorizes a command at the given snapshot position.
    pub fn allows(&self, schema: &SchemaKey, position: u64) -> bool {
        self.scope.allows(schema)
            && self
                .valid_through_position
                .is_none_or(|valid_through| position <= valid_through)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_reject_empty_exact_control_scope() {
        let error = ControlScope::exact(Vec::<SchemaKey>::new()).unwrap_err();
        assert_eq!(error, WorldError::EmptyControlScope);
    }

    #[test]
    fn test_should_expire_control_grant_after_position() -> WorldResult<()> {
        let schema: SchemaKey = "rintawa.test.command@1".parse().unwrap();
        let scope = ControlScope::exact([schema.clone()])?;
        let grant = ControlGrant::with_id(
            ControlGrantId::new(),
            PrincipalId::new(),
            EntityId::new(),
            scope,
            Some(5),
        );

        assert!(grant.allows(&schema, 5));
        assert!(!grant.allows(&schema, 6));
        Ok(())
    }
}
