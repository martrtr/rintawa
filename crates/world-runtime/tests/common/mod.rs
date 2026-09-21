#![allow(dead_code)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rintawa_sdk::{
    types::ExtensionId,
    world::{PrincipalId, SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::{
    ActorRef, SchemaDefinition, SchemaKind, WorldCommand, WorldEventDraft, WorldTransaction,
};
use rintawa_world_runtime::{SystemResult, WorldSnapshot, WorldSystem};
use tempfile::TempDir;

#[derive(Debug, Clone)]
pub struct TestSchemas {
    pub command: SchemaKey,
    pub authority_command: SchemaKey,
    pub entity: SchemaKey,
    pub event: SchemaKey,
}

fn schema(key: &str, kind: SchemaKind, definition: serde_json::Value) -> Result<SchemaDefinition> {
    Ok(SchemaDefinition::new(
        key.parse()?,
        kind,
        ExtensionId::new("rintawa.runtime-test"),
        definition,
    ))
}

pub fn create_storage() -> Result<(TempDir, SqliteWorldStorage, TestSchemas)> {
    let root = TempDir::new()?;
    let path = root.path().join("world.sqlite");
    let storage = SqliteWorldStorage::create(&path, WorldId::new())?;

    let definitions = [
        schema(
            "rintawa.runtime-test.command@1",
            SchemaKind::Command,
            serde_json::json!({ "type": "object" }),
        )?,
        schema(
            "rintawa.runtime-test.authority-command@1",
            SchemaKind::Command,
            serde_json::json!({ "type": "object" }),
        )?,
        schema(
            "rintawa.runtime-test.entity@1",
            SchemaKind::Entity,
            serde_json::json!({}),
        )?,
        schema(
            "rintawa.runtime-test.event@1",
            SchemaKind::Event,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "sequence": { "type": "integer" },
                    "kind": { "type": "string" }
                },
                "additionalProperties": true
            }),
        )?,
    ];

    for definition in &definitions {
        storage.register_schema(definition)?;
    }

    Ok((
        root,
        storage,
        TestSchemas {
            command: definitions[0].key().clone(),
            authority_command: definitions[1].key().clone(),
            entity: definitions[2].key().clone(),
            event: definitions[3].key().clone(),
        },
    ))
}

pub fn command(
    schema: &SchemaKey,
    principal: PrincipalId,
    payload: serde_json::Value,
) -> WorldCommand {
    WorldCommand::new(
        schema.clone(),
        principal,
        ActorRef::Principal(principal),
        payload,
    )
}

#[derive(Clone)]
pub struct EchoSystem {
    command_schema: SchemaKey,
    event_schema: SchemaKey,
    pub evaluations: Arc<AtomicUsize>,
}

impl EchoSystem {
    pub fn new(command_schema: SchemaKey, event_schema: SchemaKey) -> Self {
        Self {
            command_schema,
            event_schema,
            evaluations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl WorldSystem for EchoSystem {
    fn command_schema(&self) -> &SchemaKey {
        &self.command_schema
    }

    fn evaluate(
        &self,
        _snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction> {
        self.evaluations.fetch_add(1, Ordering::SeqCst);
        let mut transaction = WorldTransaction::new();
        transaction.push_event(WorldEventDraft::new(
            self.event_schema.clone(),
            command.payload().clone(),
        ));
        Ok(transaction)
    }
}
