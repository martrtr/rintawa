//! Integration tests for extension-provided World System services.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::{
    services::ServiceCallError,
    world::{EntityId, PrincipalId},
    world_system::{WorldSystemEventProposal, WorldSystemTransaction},
};
use rintawa_world::{EntityRecord, WorldMutation, WorldTransaction};
use rintawa_world_runtime::{
    MAX_WORLD_SYSTEM_SERVICE_ROUNDS, ServiceWorldSystem, SystemError, WorldRuntimeBuilder,
    WorldRuntimeError, WorldSystemReadRequest, WorldSystemReadResult, WorldSystemServiceRequest,
    WorldSystemServiceResponse, world_system_service_contract_key,
};

use common::{command, create_storage};

#[test]
fn test_service_system_reads_pinned_snapshot_and_commits_transaction() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let entity_id = EntityId::new();

    let setup = command(
        &schemas.command,
        principal,
        serde_json::json!({ "kind": "setup" }),
    );
    let mut setup_transaction = WorldTransaction::new();
    setup_transaction.push_mutation(WorldMutation::CreateEntity {
        entity: EntityRecord::new(entity_id, schemas.entity.clone()),
    });
    storage.commit(&setup, &setup_transaction, 0)?;

    let expected_contract = world_system_service_contract_key(&schemas.command);
    let event_schema = schemas.event.clone();
    let entity_schema = schemas.entity.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |contract: &_, bytes: &[u8]| {
        assert_eq!(contract, &expected_contract);
        let request: WorldSystemServiceRequest =
            serde_json::from_slice(bytes).expect("service request must decode");
        assert_eq!(request.world_id(), world_id);
        assert_eq!(request.snapshot_position(), 1);

        let response = match observed_calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert!(request.reads().is_empty());
                WorldSystemServiceResponse::Read {
                    requests: vec![WorldSystemReadRequest::Entity { entity_id }],
                }
            }
            1 => {
                assert_eq!(request.reads().len(), 1);
                match &request.reads()[0] {
                    WorldSystemReadResult::Entity {
                        entity_id: observed_id,
                        value: Some(entity),
                    } => {
                        assert_eq!(*observed_id, entity_id);
                        assert_eq!(entity.id(), entity_id);
                        assert_eq!(entity.schema(), &entity_schema);
                    }
                    other => panic!("unexpected read result: {other:?}"),
                }

                let mut transaction = WorldSystemTransaction::new();
                transaction.push_event(WorldSystemEventProposal {
                    schema: event_schema.clone(),
                    payload: serde_json::json!({ "kind": "service-system" }),
                });
                WorldSystemServiceResponse::Transaction { transaction }
            }
            other => panic!("unexpected service call {other}"),
        };
        Ok(serde_json::to_vec(&response).expect("service response must encode"))
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        world_id,
        schemas.command.clone(),
        client,
    ))?;
    let runtime = builder.start()?;
    let receipt = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "execute" }),
        ))?
        .wait()?;

    assert_eq!(receipt.position(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_service_system_rejects_cross_world_binding_before_service_call() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &_, _: &[u8]| {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::to_vec(&WorldSystemServiceResponse::Failed {
            reason: String::from("must not be reached"),
        })
        .expect("service response must encode"))
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        rintawa_sdk::world::WorldId::new(),
        schemas.command.clone(),
        client,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "wrong-world" }),
        ))?
        .wait()
        .expect_err("cross-world service binding must fail closed");

    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Failed(reason),
            ..
        } if reason.contains("another world")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.shutdown()?;

    let storage = rintawa_storage::SqliteWorldStorage::open(path)?;
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_service_system_rejects_repeated_snapshot_read() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let entity_id = EntityId::new();

    let client = move |_: &_, _: &[u8]| {
        let response = WorldSystemServiceResponse::Read {
            requests: vec![WorldSystemReadRequest::Entity { entity_id }],
        };
        Ok(serde_json::to_vec(&response).expect("service response must encode"))
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        world_id,
        schemas.command.clone(),
        client,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "repeat-read" }),
        ))?
        .wait()
        .expect_err("repeated read must fail closed");
    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Failed(reason),
            ..
        } if reason.contains("repeated")
    ));
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_service_system_bounds_read_rounds() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &_, _: &[u8]| {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Read {
            requests: vec![WorldSystemReadRequest::Entity {
                entity_id: EntityId::new(),
            }],
        };
        Ok(serde_json::to_vec(&response).expect("service response must encode"))
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        world_id,
        schemas.command.clone(),
        client,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "round-limit" }),
        ))?
        .wait()
        .expect_err("unbounded continuation must fail closed");

    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Failed(reason),
            ..
        } if reason.contains("round limit")
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        MAX_WORLD_SYSTEM_SERVICE_ROUNDS
    );
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_service_system_transport_rejects_authority_mutation_variant() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();

    let client = move |_: &_, _: &[u8]| {
        Ok(serde_json::to_vec(&serde_json::json!({
            "kind": "transaction",
            "transaction": {
                "mutations": [{ "operation": "grant-control" }],
                "events": [],
                "effects": []
            }
        }))
        .expect("malformed test response must encode"))
    };

    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        world_id,
        schemas.authority_command.clone(),
        client,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.authority_command,
            principal,
            serde_json::json!({ "kind": "grant" }),
        ))?
        .wait()
        .expect_err("ordinary System transport must not represent authority mutations");

    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Failed(reason),
            ..
        } if reason.contains("invalid System service response")
    ));
    runtime.shutdown()?;

    let storage = rintawa_storage::SqliteWorldStorage::open(path)?;
    assert_eq!(storage.load_session()?.commit_position(), 0);
    Ok(())
}

#[test]
fn test_service_system_preserves_domain_and_transport_failures() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();

    let rejecting = |_: &_, _: &[u8]| {
        Ok(serde_json::to_vec(&WorldSystemServiceResponse::Rejected {
            reason: String::from("domain rejection"),
        })
        .expect("service response must encode"))
    };
    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        world_id,
        schemas.command.clone(),
        rejecting,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "reject" }),
        ))?
        .wait()
        .expect_err("domain rejection must propagate");
    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Rejected(reason),
            ..
        } if reason == "domain rejection"
    ));
    runtime.shutdown()?;

    let (_root, storage, schemas) = create_storage()?;
    let unavailable_world_id = storage.world_id();
    let unavailable = |_: &_, _: &[u8]| -> Result<Vec<u8>, ServiceCallError> {
        Err(ServiceCallError::Unavailable)
    };
    let mut builder = WorldRuntimeBuilder::new(storage);
    builder.register_system(ServiceWorldSystem::new(
        unavailable_world_id,
        schemas.command.clone(),
        unavailable,
    ))?;
    let runtime = builder.start()?;
    let error = runtime
        .submit(command(
            &schemas.command,
            principal,
            serde_json::json!({ "kind": "unavailable" }),
        ))?
        .wait()
        .expect_err("transport failure must propagate");
    assert!(matches!(
        error,
        WorldRuntimeError::SystemFailed {
            source: SystemError::Failed(reason),
            ..
        } if reason.contains("service unavailable")
    ));
    runtime.shutdown()?;
    Ok(())
}
