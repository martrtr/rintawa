mod common;

use anyhow::Result;
use rintawa_sdk::world::{EntityId, PrincipalId, SchemaKey};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    ActorRef, ControlGrant, ControlScope, EntityRecord, WorldCommand, WorldMutation,
    WorldTransaction,
};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntimeBuilder, WorldRuntimeError, WorldSnapshot, WorldSystem,
    WorldSystemPrivileges,
};

use common::{EchoSystem, command, create_storage};

struct AuthoritySystem {
    command_schema: SchemaKey,
    entity_schema: SchemaKey,
    controlled_command_schema: SchemaKey,
    principal: PrincipalId,
    entity_id: EntityId,
}

impl WorldSystem for AuthoritySystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        _command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let scope = ControlScope::exact([self.controlled_command_schema.clone()])
            .map_err(|error| SystemError::Failed(error.to_string()))?;

        let mut transaction = WorldTransaction::new();
        transaction.push_mutation(WorldMutation::CreateEntity {
            entity: EntityRecord::new(self.entity_id, self.entity_schema.clone()),
        });
        transaction.push_mutation(WorldMutation::GrantControl {
            grant: ControlGrant::with_id(
                rintawa_sdk::world::ControlGrantId::new(),
                self.principal,
                self.entity_id,
                scope,
                None,
            ),
        });
        Ok(transaction)
    }
}

#[test]
fn test_should_require_host_privilege_for_authority_mutation() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let entity_id = EntityId::new();

    let authority_system = AuthoritySystem {
        command_schema: schemas.authority_command.clone(),
        entity_schema: schemas.entity.clone(),
        controlled_command_schema: schemas.command.clone(),
        principal,
        entity_id,
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(authority_system)?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.authority_command,
        principal,
        serde_json::json!({ "kind": "grant" }),
    ))?;
    let error = ticket.wait().unwrap_err();
    assert!(matches!(
        error,
        WorldRuntimeError::AuthorityMutationDenied(schema)
            if schema == schemas.authority_command
    ));
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(&path)?;
    assert_eq!(storage.load_session()?.commit_position(), 0);
    assert!(storage.load_entity(entity_id)?.is_none());

    let authority_system = AuthoritySystem {
        command_schema: schemas.authority_command.clone(),
        entity_schema: schemas.entity.clone(),
        controlled_command_schema: schemas.command.clone(),
        principal,
        entity_id,
    };
    let action_system = EchoSystem::new(schemas.command.clone(), schemas.event.clone());
    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system_with_privileges(
        authority_system,
        WorldSystemPrivileges::authority_manager(),
    )?;
    builder.register_system(action_system)?;
    let runtime = builder.start()?;

    assert_eq!(
        runtime
            .submit(command(
                &schemas.authority_command,
                principal,
                serde_json::json!({ "kind": "grant" }),
            ))?
            .wait()?
            .position(),
        1
    );

    let controlled = WorldCommand::new(
        schemas.command.clone(),
        principal,
        ActorRef::Entity(entity_id),
        serde_json::json!({ "kind": "controlled" }),
    );
    assert_eq!(runtime.submit(controlled)?.wait()?.position(), 2);
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert!(storage.load_entity(entity_id)?.is_some());
    assert_eq!(storage.events_after(0, 10)?.len(), 1);
    Ok(())
}
