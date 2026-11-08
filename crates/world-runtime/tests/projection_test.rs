//! Integration tests for owner-filtered, service-backed World projections.
//!
//! A projection provider is fixed to one World, one exact projection schema and a
//! set of property names owned by the calling extension. Reads of any other
//! property name resolve to `None` even when the World stores a value, and a
//! hostile or broken provider fails closed within bounded rounds, reads and
//! diagnostic sizes.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::contracts::ContractKey;
use rintawa_sdk::services::ServiceCallResult;
use rintawa_sdk::world::{ItemId, PrincipalId, SchemaKey, WorldId};
use rintawa_sdk::world_system::WorldSystemItemKind;
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{StateChange, WorldTransaction};
use rintawa_world_runtime::{
    MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES, MAX_WORLD_PROJECTION_INPUT_BYTES,
    MAX_WORLD_PROJECTION_SERVICE_ROUNDS, ServiceWorldProjection, WorldProjectionError,
    WorldProjectionReadRequest, WorldProjectionReadResult, WorldProjectionServiceRequest,
    WorldProjectionServiceResponse, world_projection_service_contract_key,
};

use common::{create_storage, key, runtime_builder};

fn fixed_client(
    response: WorldProjectionServiceResponse,
) -> impl Fn(&ContractKey, &[u8]) -> ServiceCallResult<Vec<u8>> + Send + Sync + 'static {
    move |_: &ContractKey, _: &[u8]| {
        Ok(serde_json::to_vec(&response).expect("projection response must encode"))
    }
}

fn failing_projection(
    world_id: WorldId,
    projection_schema: SchemaKey,
    response: WorldProjectionServiceResponse,
) -> ServiceWorldProjection {
    ServiceWorldProjection::new(world_id, projection_schema, [], fixed_client(response))
}

fn seed_object(storage: &SqliteWorldStorage, schemas: &common::TestSchemas) -> Result<ItemId> {
    let item = ItemId::new();
    let mut seed = WorldTransaction::new();
    seed.push_change(StateChange::CreateObject { id: item });
    seed.push_change(StateChange::SetProperty {
        item,
        key: schemas.identity.clone(),
        data: serde_json::json!({ "name": "Alice" }),
    });
    seed.push_change(StateChange::SetProperty {
        item,
        key: schemas.mood.clone(),
        data: serde_json::json!({ "level": 2 }),
    });
    storage.apply(&seed)?;
    Ok(item)
}

#[test]
fn test_should_read_only_owned_property_names() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let item = seed_object(&storage, &schemas)?;
    let runtime = runtime_builder(storage, &[])?.start()?;
    let snapshot = runtime.snapshot()?;

    let principal = PrincipalId::new();
    let projection_schema = key("runtime-test.character-view@1");
    let expected_contract = world_projection_service_contract_key(&projection_schema);
    let captured_schema = projection_schema.clone();
    let identity = schemas.identity.clone();
    let foreign = schemas.mood.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let projection = ServiceWorldProjection::new(
        world_id,
        projection_schema.clone(),
        [schemas.identity.id().clone()],
        move |contract: &ContractKey, request: &[u8]| -> ServiceCallResult<Vec<u8>> {
            assert_eq!(contract, &expected_contract);
            let request: WorldProjectionServiceRequest =
                serde_json::from_slice(request).expect("projection request must decode");
            assert_eq!(request.world_id(), world_id);
            assert_eq!(request.projection_schema(), &captured_schema);
            assert_eq!(request.principal(), principal);
            assert_eq!(request.input()["item"], item.to_string().as_str());

            let response = match observed_calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert!(request.reads().is_empty());
                    WorldProjectionServiceResponse::Read {
                        requests: vec![
                            WorldProjectionReadRequest::Item { item_id: item },
                            WorldProjectionReadRequest::Property {
                                item_id: item,
                                key: identity.clone(),
                            },
                            WorldProjectionReadRequest::Property {
                                item_id: item,
                                key: foreign.clone(),
                            },
                        ],
                    }
                }
                1 => {
                    assert_eq!(request.reads().len(), 3);
                    match &request.reads()[0] {
                        WorldProjectionReadResult::Item {
                            item_id,
                            value: Some(record),
                        } => {
                            assert_eq!(*item_id, item);
                            assert_eq!(record.kind, WorldSystemItemKind::Object);
                            assert!(!record.is_relation());
                        }
                        other => panic!("unexpected item read result: {other:?}"),
                    }
                    match &request.reads()[1] {
                        WorldProjectionReadResult::Property {
                            value: Some(record),
                            ..
                        } => {
                            assert_eq!(record.data()["name"], "Alice");
                        }
                        other => panic!("unexpected owned property result: {other:?}"),
                    }
                    assert!(matches!(
                        &request.reads()[2],
                        WorldProjectionReadResult::Property { value: None, .. }
                    ));
                    WorldProjectionServiceResponse::Complete {
                        value: serde_json::json!({ "name": "Alice" }),
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
        serde_json::json!({ "item": item.to_string() }),
    )?;
    assert_eq!(view.world_id(), world_id);
    assert_eq!(view.schema(), &projection_schema);
    assert_eq!(view.principal(), principal);
    assert_eq!(view.value()["name"], "Alice");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_fail_closed_for_rejected_and_failed_diagnostics() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let world_id = storage.world_id();
    let runtime = runtime_builder(storage, &[])?.start()?;
    let snapshot = runtime.snapshot()?;
    let principal = PrincipalId::new();
    let schema = "runtime-test.character-view@1";

    let rejected = failing_projection(
        world_id,
        key(schema),
        WorldProjectionServiceResponse::Rejected {
            reason: String::from("projection policy denies this reader"),
        },
    );
    let error = rejected
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("policy rejection must propagate");
    match error {
        WorldProjectionError::Rejected(reason) => {
            assert_eq!(reason, "projection policy denies this reader");
        }
        other => panic!("unexpected projection error: {other:?}"),
    }

    let failed = failing_projection(
        world_id,
        key(schema),
        WorldProjectionServiceResponse::Failed {
            reason: String::from("provider could not build a view"),
        },
    );
    let error = failed
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("provider failure must propagate");
    match error {
        WorldProjectionError::Failed(reason) => {
            assert_eq!(reason, "provider could not build a view");
        }
        other => panic!("unexpected projection error: {other:?}"),
    }

    let oversized = failing_projection(
        world_id,
        key(schema),
        WorldProjectionServiceResponse::Failed {
            reason: "x".repeat(MAX_WORLD_PROJECTION_DIAGNOSTIC_BYTES + 1),
        },
    );
    let error = oversized
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("oversized diagnostic must fail closed");
    match error {
        WorldProjectionError::Failed(reason) => {
            assert!(reason.contains("oversized"));
        }
        other => panic!("unexpected projection error: {other:?}"),
    }

    let empty_round = failing_projection(
        world_id,
        key(schema),
        WorldProjectionServiceResponse::Read { requests: vec![] },
    );
    let error = empty_round
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("empty read round must fail closed");
    match error {
        WorldProjectionError::Failed(reason) => {
            assert!(reason.contains("empty read round"));
        }
        other => panic!("unexpected projection error: {other:?}"),
    }

    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_oversized_projection_input_before_provider_call() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let world_id = storage.world_id();
    let runtime = runtime_builder(storage, &[])?.start()?;
    let snapshot = runtime.snapshot()?;
    let principal = PrincipalId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let projection = ServiceWorldProjection::new(
        world_id,
        key("runtime-test.character-view@1"),
        [],
        move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            let response = WorldProjectionServiceResponse::Complete {
                value: serde_json::json!({}),
            };
            Ok(serde_json::to_vec(&response).expect("projection response must encode"))
        },
    );

    let input = serde_json::json!({ "blob": "x".repeat(MAX_WORLD_PROJECTION_INPUT_BYTES + 1) });
    let error = projection
        .project(&snapshot, principal, input)
        .expect_err("oversized input must be rejected");
    match error {
        WorldProjectionError::Rejected(reason) => {
            assert!(reason.contains("too large"));
        }
        other => panic!("unexpected projection error: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_repeated_projection_snapshot_read() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let world_id = storage.world_id();
    let runtime = runtime_builder(storage, &[])?.start()?;
    let snapshot = runtime.snapshot()?;
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let projection = ServiceWorldProjection::new(
        world_id,
        key("runtime-test.character-view@1"),
        [],
        move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            let response = WorldProjectionServiceResponse::Read {
                requests: vec![WorldProjectionReadRequest::Item { item_id: item }],
            };
            Ok(serde_json::to_vec(&response).expect("projection response must encode"))
        },
    );

    let error = projection
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("a repeated snapshot read must fail closed");
    match error {
        WorldProjectionError::Failed(reason) => {
            assert!(reason.contains("repeated"));
        }
        other => panic!("unexpected projection error: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_bound_projection_evaluation_rounds() -> Result<()> {
    let (_root, storage, _schemas) = create_storage()?;
    let world_id = storage.world_id();
    let runtime = runtime_builder(storage, &[])?.start()?;
    let snapshot = runtime.snapshot()?;
    let principal = PrincipalId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let projection = ServiceWorldProjection::new(
        world_id,
        key("runtime-test.character-view@1"),
        [],
        move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            let response = WorldProjectionServiceResponse::Read {
                requests: vec![WorldProjectionReadRequest::Item {
                    item_id: ItemId::new(),
                }],
            };
            Ok(serde_json::to_vec(&response).expect("projection response must encode"))
        },
    );

    let error = projection
        .project(&snapshot, principal, serde_json::json!({}))
        .expect_err("unbounded continuation must fail closed");
    match error {
        WorldProjectionError::Failed(reason) => {
            assert!(reason.contains("round limit"));
        }
        other => panic!("unexpected projection error: {other:?}"),
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        MAX_WORLD_PROJECTION_SERVICE_ROUNDS
    );
    runtime.shutdown()?;
    Ok(())
}
