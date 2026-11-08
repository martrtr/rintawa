//! Integration tests for extension-provided World System services.
//!
//! An ordinary extension System lives behind one platform service contract. The
//! runtime encodes the immutable command envelope, resolves bounded read rounds
//! from the same pinned snapshot, decodes an item/property proposal and still
//! enforces the authoritative property vocabulary before COMMIT.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::contracts::ContractKey;
use rintawa_sdk::services::{ServiceCallError, ServiceCallResult};
use rintawa_sdk::world::{CommandId, CorrelationId, ItemId, PrincipalId, UnixTimeMillis, WorldId};
use rintawa_sdk::world_system::{
    WorldSystemCausation, WorldSystemCommand, WorldSystemMutation, WorldSystemSignalProposal,
    WorldSystemTransaction,
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{StateChange, WorldTransaction};
use rintawa_world_runtime::{
    MAX_WORLD_SYSTEM_SERVICE_ROUNDS, ServiceWorldSystem, SystemError, WorldRuntime,
    WorldRuntimeError, WorldSystemReadRequest, WorldSystemReadResult, WorldSystemServiceRequest,
    WorldSystemServiceResponse, world_system_service_contract_key,
};

use common::{command, create_storage, key, runtime_builder};

/// One over the private 8 KiB runtime diagnostic ceiling.
const OVERSIZED_DIAGNOSTIC_LEN: usize = 8 * 1024 + 1;

fn fixed_client(
    response: WorldSystemServiceResponse,
) -> impl Fn(&ContractKey, &[u8]) -> ServiceCallResult<Vec<u8>> + Send + Sync + 'static {
    move |_: &ContractKey, _: &[u8]| {
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    }
}

fn expect_service_failure(
    runtime: &WorldRuntime,
    schemas: &common::TestSchemas,
    principal: PrincipalId,
    item: ItemId,
    fragment: &str,
) -> Result<()> {
    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": item.to_string() }),
    ))?;
    let error = ticket.wait().expect_err("service System must fail closed");
    match error {
        WorldRuntimeError::SystemFailed { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                SystemError::Failed(reason) => {
                    assert!(reason.contains(fragment));
                }
                other => panic!("unexpected System error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    Ok(())
}

#[test]
fn test_should_round_trip_reads_and_commit_proposal() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();

    let mut seed = WorldTransaction::new();
    seed.push_change(StateChange::CreateObject { id: item });
    seed.push_change(StateChange::SetProperty {
        item,
        key: schemas.identity.clone(),
        data: serde_json::json!({ "name": "Alice" }),
    });
    storage.apply(&seed)?;

    let expected_contract = world_system_service_contract_key(&schemas.command);
    let expected_payload = serde_json::json!({ "item": item.to_string() });
    let submitted = command(&schemas.command, principal, expected_payload.clone());
    let command_id = submitted.id();
    let command_schema = schemas.command.clone();
    let identity = schemas.identity.clone();
    let mood = schemas.mood.clone();
    let signal = schemas.signal.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |contract: &ContractKey, request: &[u8]| -> ServiceCallResult<Vec<u8>> {
        assert_eq!(contract, &expected_contract);
        let request: WorldSystemServiceRequest =
            serde_json::from_slice(request).expect("System request must decode");
        assert_eq!(request.world_id(), world_id);
        assert_eq!(request.command().id, command_id);
        assert_eq!(request.command().schema, command_schema);
        assert_eq!(request.command().principal, principal);
        assert_eq!(request.command().payload, expected_payload);

        let response = match observed_calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert!(request.reads().is_empty());
                WorldSystemServiceResponse::Read {
                    requests: vec![
                        WorldSystemReadRequest::Item { item_id: item },
                        WorldSystemReadRequest::Property {
                            item_id: item,
                            key: identity.clone(),
                        },
                    ],
                }
            }
            1 => {
                assert_eq!(request.reads().len(), 2);
                match &request.reads()[0] {
                    WorldSystemReadResult::Item {
                        item_id,
                        value: Some(record),
                    } => {
                        assert_eq!(*item_id, item);
                        assert!(!record.is_relation());
                    }
                    other => panic!("unexpected item read result: {other:?}"),
                }
                match &request.reads()[1] {
                    WorldSystemReadResult::Property {
                        value: Some(record),
                        ..
                    } => {
                        assert_eq!(record.data()["name"], "Alice");
                    }
                    other => panic!("unexpected property read result: {other:?}"),
                }

                let mut transaction = WorldSystemTransaction::new();
                transaction.push_mutation(WorldSystemMutation::SetProperty {
                    item_id: item,
                    key: mood.clone(),
                    data: serde_json::json!({ "level": 4 }),
                });
                transaction.push_signal(WorldSystemSignalProposal {
                    schema: signal.clone(),
                    payload: serde_json::json!({ "level": 4 }),
                });
                WorldSystemServiceResponse::Transaction { transaction }
            }
            other => panic!("unexpected System service call {other}"),
        };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let outcome = runtime.submit(submitted)?.wait()?;
    assert_eq!(outcome.command_id(), command_id);
    assert_eq!(outcome.signals().len(), 1);
    assert_eq!(outcome.signals()[0].schema(), &schemas.signal);
    assert_eq!(outcome.signals()[0].payload()["level"], 4);

    let snapshot = runtime.snapshot()?;
    let mood_record = snapshot
        .load_property(item, &schemas.mood)?
        .expect("mood committed");
    assert_eq!(mood_record.data()["level"], 4);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_round_trip_system_service_transport() -> Result<()> {
    let item = ItemId::new();
    let world_id = WorldId::new();
    let command_id = CommandId::new();
    let correlation_id = CorrelationId::new();
    let principal = PrincipalId::new();
    let causation = CommandId::new();
    let schema = key("runtime-test.command@1");
    let property = key("runtime-test.identity@1");

    let request = WorldSystemServiceRequest {
        world_id,
        command: WorldSystemCommand {
            id: command_id,
            schema: schema.clone(),
            principal,
            causation: Some(WorldSystemCausation::Command(causation)),
            correlation_id,
            effective_at: Some(UnixTimeMillis::new(1_700_000_000_000)),
            payload: serde_json::json!({ "item": item.to_string() }),
        },
        reads: Vec::new(),
    };
    let encoded = serde_json::to_vec(&request)?;
    let decoded: WorldSystemServiceRequest = serde_json::from_slice(&encoded)?;
    assert_eq!(decoded, request);

    let mut transaction = WorldSystemTransaction::new();
    transaction.push_mutation(WorldSystemMutation::SetProperty {
        item_id: item,
        key: property,
        data: serde_json::json!({ "name": "Alice" }),
    });
    transaction.push_signal(WorldSystemSignalProposal {
        schema,
        payload: serde_json::json!({ "created": true }),
    });
    let response = WorldSystemServiceResponse::Transaction { transaction };
    let encoded = serde_json::to_vec(&response)?;
    let decoded: WorldSystemServiceResponse = serde_json::from_slice(&encoded)?;
    assert_eq!(decoded, response);
    Ok(())
}

#[test]
fn test_should_reject_cross_world_binding_before_service_call() -> Result<()> {
    let (root, storage, schemas) = create_storage()?;
    let path = root.path().join("world.sqlite");
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Failed {
            reason: String::from("must not be reached"),
        };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(WorldId::new(), schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": item.to_string() }),
    ))?;
    let error = ticket.wait().expect_err("cross-world binding must fail");
    match error {
        WorldRuntimeError::SystemFailed { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                SystemError::Failed(reason) => {
                    assert!(reason.contains("another world"));
                }
                other => panic!("unexpected System error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.shutdown()?;

    let storage = SqliteWorldStorage::open(path)?;
    assert!(storage.load_item(item)?.is_none());
    Ok(())
}

#[test]
fn test_should_reject_repeated_snapshot_read() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Read {
            requests: vec![WorldSystemReadRequest::Item { item_id: item }],
        };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    expect_service_failure(&runtime, &schemas, principal, item, "repeated")?;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_bound_system_service_read_rounds() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Read {
            requests: vec![WorldSystemReadRequest::Item {
                item_id: ItemId::new(),
            }],
        };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    expect_service_failure(&runtime, &schemas, principal, item, "round limit")?;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        MAX_WORLD_SYSTEM_SERVICE_ROUNDS
    );
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_empty_read_round() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Read { requests: vec![] };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    expect_service_failure(&runtime, &schemas, principal, item, "empty read round")?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_reject_oversized_diagnostic() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);

    let client = move |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        let response = WorldSystemServiceResponse::Failed {
            reason: "x".repeat(OVERSIZED_DIAGNOSTIC_LEN),
        };
        Ok(serde_json::to_vec(&response).expect("System response must encode"))
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    expect_service_failure(&runtime, &schemas, principal, item, "oversized")?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_preserve_domain_and_transport_failures() -> Result<()> {
    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let principal = PrincipalId::new();
    let item = ItemId::new();
    let client = fixed_client(WorldSystemServiceResponse::Rejected {
        reason: String::from("domain rejection"),
    });
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), client);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    let ticket = runtime.submit(command(
        &schemas.command,
        principal,
        serde_json::json!({ "item": item.to_string() }),
    ))?;
    let error = ticket.wait().expect_err("domain rejection must propagate");
    match error {
        WorldRuntimeError::SystemFailed { schema, source } => {
            assert_eq!(schema, schemas.command);
            match source {
                SystemError::Rejected(reason) => assert_eq!(reason, "domain rejection"),
                other => panic!("unexpected System error: {other:?}"),
            }
        }
        other => panic!("unexpected runtime error: {other:?}"),
    }
    runtime.shutdown()?;

    let (_root, storage, schemas) = create_storage()?;
    let world_id = storage.world_id();
    let unavailable = |_: &ContractKey, _: &[u8]| -> ServiceCallResult<Vec<u8>> {
        Err(ServiceCallError::Unavailable)
    };
    let system = ServiceWorldSystem::new(world_id, schemas.command.clone(), unavailable);

    let mut builder = runtime_builder(storage, &[&schemas.command])?;
    builder.register_system(system)?;
    let runtime = builder.start()?;

    expect_service_failure(&runtime, &schemas, principal, item, "unavailable")?;
    runtime.shutdown()?;
    Ok(())
}
