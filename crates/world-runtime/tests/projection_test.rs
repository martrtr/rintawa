//! Integration tests for policy-filtered world projections.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::{
    types::ExtensionId,
    world::{EntityId, PrincipalId, SchemaKey},
};
use rintawa_world::{
    ControlGrant, ControlScope, EntityRecord, FacetRecord, FacetTarget, SchemaDefinition,
    SchemaKind, WorldMutation, WorldTransaction,
};
use rintawa_world_runtime::{
    ServiceWorldProjection, WorldProjectionError, WorldProjectionReadRequest,
    WorldProjectionReadResult, WorldProjectionServiceRequest, WorldProjectionServiceResponse,
    WorldRuntimeBuilder, world_projection_service_contract_key,
};

use common::{command, create_storage};

fn register_projection_schema(
    storage: &rintawa_storage::SqliteWorldStorage,
    owner: &ExtensionId,
) -> Result<SchemaKey> {
    let key: SchemaKey = "rintawa.runtime-test.character-view@1".parse()?;
    storage.register_schema(&SchemaDefinition::new(
        key.clone(),
        SchemaKind::Projection,
        owner.clone(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "own_secret": { "type": "string" },
                "foreign_visible": { "type": "boolean" },
                "can_control": { "type": "boolean" }
            },
            "required": ["own_secret", "foreign_visible", "can_control"],
            "additionalProperties": false
        }),
    ))?;
    Ok(key)
}

#[test]
fn test_should_build_principal_filtered_projection_from_owner_scoped_reads() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let owner = ExtensionId::new("rintawa.runtime-test");
    let foreign_owner = ExtensionId::new("rintawa.foreign");
    let projection_schema = register_projection_schema(&storage, &owner)?;
    let own_facet: SchemaKey = "rintawa.runtime-test.private@1".parse()?;
    let foreign_facet: SchemaKey = "rintawa.foreign.private@1".parse()?;
    storage.register_schema(&SchemaDefinition::new(
        own_facet.clone(),
        SchemaKind::Facet,
        owner.clone(),
        serde_json::json!({ "type": "object" }),
    ))?;
    storage.register_schema(&SchemaDefinition::new(
        foreign_facet.clone(),
        SchemaKind::Facet,
        foreign_owner,
        serde_json::json!({ "type": "object" }),
    ))?;

    let principal = PrincipalId::new();
    let entity_id = EntityId::new();
    let mut setup = WorldTransaction::new();
    setup.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    setup.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(entity_id),
            own_facet.clone(),
            serde_json::json!({ "secret": "owner-visible" }),
        ),
    });
    setup.push_mutation(WorldMutation::SetFacet {
        facet: FacetRecord::new(
            FacetTarget::Entity(entity_id),
            foreign_facet.clone(),
            serde_json::json!({ "secret": "must-not-leak" }),
        ),
    });
    setup.push_mutation(WorldMutation::GrantControl {
        grant: ControlGrant::with_id(
            rintawa_sdk::world::ControlGrantId::new(),
            principal,
            entity_id,
            ControlScope::exact([schemas.command.clone()])?,
            None,
        ),
    });
    storage.commit(
        &command(
            &schemas.authority_command,
            principal,
            serde_json::json!({ "kind": "setup" }),
        ),
        &setup,
        0,
    )?;

    let runtime = WorldRuntimeBuilder::new(storage).start()?;
    let snapshot = runtime.snapshot()?;
    let expected_contract = world_projection_service_contract_key(&projection_schema);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let expected_command = schemas.command.clone();
    let expected_own_facet = own_facet.clone();
    let expected_foreign_facet = foreign_facet.clone();

    let projection = ServiceWorldProjection::new(
        world_id,
        projection_schema.clone(),
        owner,
        move |contract: &_, bytes: &[u8]| {
            assert_eq!(contract, &expected_contract);
            let request: WorldProjectionServiceRequest =
                serde_json::from_slice(bytes).expect("projection request must decode");
            assert_eq!(request.world_id(), world_id);
            assert_eq!(request.snapshot_position(), 1);
            assert_eq!(request.principal(), principal);

            let response = match observed_calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert!(request.reads().is_empty());
                    WorldProjectionServiceResponse::Read {
                        requests: vec![
                            WorldProjectionReadRequest::Facet {
                                target: FacetTarget::Entity(entity_id),
                                schema: expected_own_facet.clone(),
                            },
                            WorldProjectionReadRequest::Facet {
                                target: FacetTarget::Entity(entity_id),
                                schema: expected_foreign_facet.clone(),
                            },
                            WorldProjectionReadRequest::CanControl {
                                actor_entity: entity_id,
                                command_schema: expected_command.clone(),
                            },
                        ],
                    }
                }
                1 => {
                    assert_eq!(request.reads().len(), 3);
                    match &request.reads()[0] {
                        WorldProjectionReadResult::Facet {
                            value: Some(facet), ..
                        } => assert_eq!(facet.payload()["secret"], "owner-visible"),
                        other => panic!("unexpected owner facet result: {other:?}"),
                    }
                    assert!(matches!(
                        &request.reads()[1],
                        WorldProjectionReadResult::Facet { value: None, .. }
                    ));
                    assert!(matches!(
                        &request.reads()[2],
                        WorldProjectionReadResult::CanControl { allowed: true, .. }
                    ));
                    WorldProjectionServiceResponse::Complete {
                        value: serde_json::json!({
                            "own_secret": "owner-visible",
                            "foreign_visible": false,
                            "can_control": true
                        }),
                    }
                }
                other => panic!("unexpected projection call {other}"),
            };
            Ok(serde_json::to_vec(&response).expect("projection response must encode"))
        },
    );

    let view = projection.project(
        &snapshot,
        principal,
        serde_json::json!({ "entity_id": entity_id.to_string() }),
    )?;
    assert_eq!(view.world_id(), world_id);
    assert_eq!(view.snapshot_position(), 1);
    assert_eq!(view.schema(), &projection_schema);
    assert_eq!(view.principal(), principal);
    assert_eq!(view.value()["foreign_visible"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_fail_closed_for_foreign_control_probe_and_invalid_output() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let owner = ExtensionId::new("rintawa.runtime-test");
    let projection_schema = register_projection_schema(&storage, &owner)?;
    let foreign_command: SchemaKey = "rintawa.foreign.command@1".parse()?;
    storage.register_schema(&SchemaDefinition::new(
        foreign_command.clone(),
        SchemaKind::Command,
        ExtensionId::new("rintawa.foreign"),
        serde_json::json!({ "type": "object" }),
    ))?;
    let principal = PrincipalId::new();
    let entity_id = EntityId::new();
    let mut setup = WorldTransaction::new();
    setup.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    setup.push_mutation(WorldMutation::GrantControl {
        grant: ControlGrant::any(principal, entity_id),
    });
    storage.commit(
        &command(
            &schemas.authority_command,
            principal,
            serde_json::json!({ "kind": "setup" }),
        ),
        &setup,
        0,
    )?;

    let runtime = WorldRuntimeBuilder::new(storage).start()?;
    let snapshot = runtime.snapshot()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let projection = ServiceWorldProjection::new(
        world_id,
        projection_schema,
        owner,
        move |_: &_, bytes: &[u8]| {
            let request: WorldProjectionServiceRequest =
                serde_json::from_slice(bytes).expect("projection request must decode");
            let response = match observed_calls.fetch_add(1, Ordering::SeqCst) {
                0 => WorldProjectionServiceResponse::Read {
                    requests: vec![WorldProjectionReadRequest::CanControl {
                        actor_entity: entity_id,
                        command_schema: foreign_command.clone(),
                    }],
                },
                1 => {
                    assert!(matches!(
                        request.reads(),
                        [WorldProjectionReadResult::CanControl { allowed: false, .. }]
                    ));
                    WorldProjectionServiceResponse::Complete {
                        value: serde_json::json!({ "invalid": true }),
                    }
                }
                other => panic!("unexpected projection call {other}"),
            };
            Ok(serde_json::to_vec(&response).expect("projection response must encode"))
        },
    );

    assert!(matches!(
        projection.project(&snapshot, principal, serde_json::json!({})),
        Err(WorldProjectionError::InvalidOutput(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_repeated_projection_snapshot_read() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let world_id = storage.world_id();
    let owner = ExtensionId::new("rintawa.runtime-test");
    let projection_schema = register_projection_schema(&storage, &owner)?;
    let principal = PrincipalId::new();
    let runtime = WorldRuntimeBuilder::new(storage).start()?;
    let snapshot = runtime.snapshot()?;
    let entity_id = EntityId::new();
    let projection = ServiceWorldProjection::new(
        world_id,
        projection_schema,
        owner,
        move |_: &_, _: &[u8]| {
            Ok(serde_json::to_vec(&WorldProjectionServiceResponse::Read {
                requests: vec![WorldProjectionReadRequest::Entity { entity_id }],
            })
            .expect("projection response must encode"))
        },
    );

    assert!(matches!(
        projection.project(&snapshot, principal, serde_json::json!({})),
        Err(WorldProjectionError::RepeatedRead)
    ));
    runtime.shutdown()?;
    Ok(())
}
