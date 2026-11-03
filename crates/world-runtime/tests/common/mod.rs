//! Shared fixtures and helpers for World Runtime integration tests.
//!
//! The fixtures describe the four-table item/property World: object and relation
//! items, immutable versioned property definitions and one atomic change set per
//! command. There is deliberately no command ledger, event log, effect job,
//! commit position or control grant to fixture.

#![allow(dead_code)]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::{
    types::ExtensionId,
    world::{ItemId, PrincipalId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    PropertyDefinition, RuntimeSchemaDefinition, RuntimeSignal, SchemaKind, StateChange,
    WorldCommand, WorldTransaction,
};
use rintawa_world_runtime::{
    SystemError, SystemResult, WorldRuntimeBuilder, WorldSnapshot, WorldSystem,
};
use tempfile::TempDir;

/// Extension that owns every fixture schema definition.
pub const EXTENSION: &str = "rintawa.runtime-test";
/// Primary fixture command schema.
pub const COMMAND: &str = "runtime-test.command@1";
/// Fixture property holding an object's display name.
pub const IDENTITY: &str = "runtime-test.identity@1";
/// Fixture property holding a bounded integer.
pub const MOOD: &str = "runtime-test.mood@1";
/// Fixture property used as a monotonically increasing sequence value.
pub const SEQUENCE: &str = "runtime-test.sequence@1";
/// Fixture best-effort post-COMMIT signal schema.
pub const SIGNAL: &str = "runtime-test.changed@1";

/// Canonical fixture schema keys shared by the integration-test binaries.
#[derive(Debug, Clone)]
pub struct TestSchemas {
    /// Primary command schema.
    pub command: SchemaKey,
    /// Object display-name property.
    pub identity: SchemaKey,
    /// Bounded integer property.
    pub mood: SchemaKey,
    /// Sequence property.
    pub sequence: SchemaKey,
    /// Post-COMMIT signal schema.
    pub signal: SchemaKey,
}

/// Parses one canonical fixture schema key.
pub fn key(value: &str) -> SchemaKey {
    value.parse().expect("fixture schema key must be canonical")
}

/// Returns the fixture schema keys.
pub fn schemas() -> TestSchemas {
    TestSchemas {
        command: key(COMMAND),
        identity: key(IDENTITY),
        mood: key(MOOD),
        sequence: key(SEQUENCE),
        signal: key(SIGNAL),
    }
}

fn identity_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn mood_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "level": { "type": "integer", "minimum": 1, "maximum": 5 } },
        "required": ["level"],
        "additionalProperties": false
    })
}

fn property_definition(key: &SchemaKey, schema: serde_json::Value) -> PropertyDefinition {
    PropertyDefinition::new(key.clone(), ExtensionId::new(EXTENSION), schema)
}

/// Creates one empty World database with the fixture property vocabulary.
///
/// The persisted vocabulary is restored by [`WorldRuntimeBuilder::start`], so the
/// tests never need to re-register definitions after a restart.
pub fn create_storage() -> Result<(TempDir, SqliteWorldStorage, TestSchemas)> {
    let directory = TempDir::new()?;
    let path = directory.path().join("world.sqlite");
    let storage = SqliteWorldStorage::create(&path, WorldId::new())?;
    let schemas = schemas();
    storage.register_definitions(&[
        property_definition(&schemas.identity, identity_schema()),
        property_definition(&schemas.mood, mood_schema()),
        property_definition(
            &schemas.sequence,
            serde_json::json!({ "type": "integer", "minimum": 0 }),
        ),
    ])?;
    Ok((directory, storage, schemas))
}

/// Creates a runtime builder with transient `Command` runtime API schemas.
///
/// Command schemas are extension runtime-API descriptions; they are never part of
/// the persisted property vocabulary.
pub fn runtime_builder(
    storage: SqliteWorldStorage,
    command_schemas: &[&SchemaKey],
) -> Result<WorldRuntimeBuilder> {
    let mut builder = WorldRuntimeBuilder::new(storage);
    for schema in command_schemas {
        builder.register_runtime_schema(RuntimeSchemaDefinition::new(
            (*schema).clone(),
            SchemaKind::Command,
            ExtensionId::new(EXTENSION),
            serde_json::json!({ "type": "object" }),
        ))?;
    }
    Ok(builder)
}

/// Builds one command envelope for a fixture schema.
pub fn command(
    schema: &SchemaKey,
    principal: PrincipalId,
    payload: serde_json::Value,
) -> WorldCommand {
    WorldCommand::new(schema.clone(), principal, payload)
}

/// Decodes one item identity from a command payload field.
///
/// # Errors
///
/// Returns [`SystemError::Failed`] when the field is absent or not a UUID.
pub fn payload_item(command: &WorldCommand, field: &str) -> SystemResult<ItemId> {
    serde_json::from_value(command.payload()[field].clone())
        .map_err(|_| SystemError::Failed(format!("field {field} is not an item")))
}

/// System that creates one object-item per command and records evaluation order.
///
/// The payload carries both an `item` identity and a numeric `sequence`, which
/// lets a test observe the exact order in which the single writer evaluated
/// commands without relying on any durable event or receipt.
#[derive(Clone)]
pub struct RecordingSystem {
    command_schema: SchemaKey,
    evaluations: Arc<AtomicUsize>,
    observed_sequences: Arc<Mutex<Vec<i64>>>,
}

impl RecordingSystem {
    /// Creates one recording System for an exact command schema.
    pub fn new(command_schema: SchemaKey) -> Self {
        Self {
            command_schema,
            evaluations: Arc::new(AtomicUsize::new(0)),
            observed_sequences: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the shared evaluation counter.
    pub fn evaluations(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.evaluations)
    }

    /// Returns the sequence numbers observed in evaluation order.
    pub fn observed_sequences(&self) -> Arc<Mutex<Vec<i64>>> {
        Arc::clone(&self.observed_sequences)
    }
}

impl WorldSystem for RecordingSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        self.evaluations.fetch_add(1, Ordering::SeqCst);
        let sequence = command.payload()["sequence"].as_i64().unwrap_or_default();
        self.observed_sequences
            .lock()
            .map_err(|_| SystemError::Failed(String::from("observed sequence lock poisoned")))?
            .push(sequence);

        let item = payload_item(command, "item")?;
        let mut transaction = WorldTransaction::new();
        transaction.push_change(StateChange::CreateObject { id: item });
        Ok(transaction)
    }
}

/// System that returns one pre-built change set.
///
/// This is how the commit and rollback tests describe exactly which items and
/// properties one command proposes, including deliberately invalid combinations.
#[derive(Clone)]
pub struct ScriptedSystem {
    command_schema: SchemaKey,
    changes: Vec<StateChange>,
    signals: Vec<RuntimeSignal>,
}

impl ScriptedSystem {
    /// Creates a System that proposes exactly the supplied change set and signals.
    pub fn new(
        command_schema: SchemaKey,
        changes: Vec<StateChange>,
        signals: Vec<RuntimeSignal>,
    ) -> Self {
        Self {
            command_schema,
            changes,
            signals,
        }
    }
}

impl WorldSystem for ScriptedSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        _command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        let mut transaction = WorldTransaction::new();
        for change in &self.changes {
            transaction.push_change(change.clone());
        }
        for signal in &self.signals {
            transaction.push_signal(signal.clone());
        }
        Ok(transaction)
    }
}
