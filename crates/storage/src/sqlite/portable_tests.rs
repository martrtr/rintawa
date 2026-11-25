//! Regressions for untrusted World seeds and consistent native copies.

use rintawa_sdk::{
    types::ExtensionId,
    world::{ItemId, SchemaKey, WorldId},
};
use rintawa_world::{PropertyDefinition, StateChange, WorldManifest, WorldTransaction};
use rusqlite::Connection;

use super::SqliteWorldStorage;

fn seeded(path: &std::path::Path) -> anyhow::Result<(SqliteWorldStorage, ItemId, ItemId, ItemId)> {
    let storage = SqliteWorldStorage::create(path, WorldId::new())?;
    let key: SchemaKey = "story.card@1".parse()?;
    storage.register_definition(&PropertyDefinition::new(
        key.clone(), ExtensionId::new("story.owner"),
        serde_json::json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}),
    ))?;
    let (character, conversation, link) = (ItemId::new(), ItemId::new(), ItemId::new());
    let mut tx = WorldTransaction::new();
    tx.push_change(StateChange::CreateObject { id: character });
    tx.push_change(StateChange::CreateObject { id: conversation });
    tx.push_change(StateChange::CreateRelation {
        id: link,
        subject: conversation,
        object: character,
    });
    tx.push_change(StateChange::SetProperty {
        item: character,
        key,
        data: serde_json::json!({"name":"Alice"}),
    });
    storage.apply(&tx)?;
    Ok((storage, character, conversation, link))
}

#[test]
fn portable_seed_clone_keeps_graph_and_uses_new_world_identity() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.sqlite");
    let destination = dir.path().join("clone.sqlite");
    let (original, character, conversation, link) = seeded(&source)?;
    let source_bytes = std::fs::read(&source)?;
    let seed = SqliteWorldStorage::validate_seed(&source)?;
    assert_eq!(seed.id(), original.world_id());
    let next = WorldId::new();
    let manifest = WorldManifest::new(serde_json::json!({"metadata":{"title":"Fork"}}));
    let cloned = SqliteWorldStorage::create_from_seed(&source, &destination, next, &manifest)?;
    assert_eq!(cloned.world_id(), next);
    assert_ne!(cloned.world_id(), original.world_id());
    assert_eq!(
        cloned.load_session()?.manifest().as_value()["metadata"]["title"],
        "Fork"
    );
    assert_eq!(cloned.load_item(link)?, original.load_item(link)?);
    assert_eq!(
        cloned.load_item(conversation)?,
        original.load_item(conversation)?
    );
    let key: SchemaKey = "story.card@1".parse()?;
    assert_eq!(
        cloned.load_property(character, &key)?,
        original.load_property(character, &key)?
    );
    let replacement = serde_json::json!({"name":"Changed only in clone"});
    let mut tx = WorldTransaction::new();
    tx.push_change(StateChange::SetProperty {
        item: character,
        key: key.clone(),
        data: replacement,
    });
    cloned.apply(&tx)?;
    assert_eq!(
        original.load_property(character, &key)?.unwrap().data()["name"],
        "Alice"
    );
    assert_eq!(std::fs::read(&source)?, source_bytes);
    Ok(())
}

#[test]
fn portable_seed_rejects_invalid_payload_and_cleans_destination() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.sqlite");
    let destination = dir.path().join("clone.sqlite");
    let (storage, character, _, _) = seeded(&source)?;
    // Simulate a valid four-table file whose content bypassed the System validator.
    Connection::open(&source)?.execute(
        "UPDATE properties SET data = '{\"name\":3}' WHERE item = ?1",
        [character.into_bytes().as_slice()],
    )?;
    assert!(SqliteWorldStorage::validate_seed(&source).is_err());
    assert!(
        SqliteWorldStorage::create_from_seed(
            &source,
            &destination,
            WorldId::new(),
            &WorldManifest::default()
        )
        .is_err()
    );
    assert!(!destination.exists());
    drop(storage);
    Ok(())
}

#[test]
fn portable_seed_rejects_relation_to_relation_even_with_valid_foreign_keys() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.sqlite");
    let (storage, _character, conversation, relation) = seeded(&source)?;
    let database = Connection::open(&source)?;
    let trigger: String = database.query_row(
        "SELECT sql FROM sqlite_master WHERE name = 'items_relation_endpoints_ins'",
        [],
        |row| row.get(0),
    )?;
    database.execute_batch("DROP TRIGGER items_relation_endpoints_ins")?;
    database.execute(
        "INSERT INTO items (id,type,subject,object) VALUES (?1,2,?2,?3)",
        rusqlite::params![
            ItemId::new().into_bytes().as_slice(),
            conversation.into_bytes().as_slice(),
            relation.into_bytes().as_slice()
        ],
    )?;
    database.execute_batch(&trigger)?;
    assert!(SqliteWorldStorage::validate_seed(&source).is_err());
    drop(storage);
    Ok(())
}

#[test]
fn portable_snapshot_includes_wal_and_never_overwrites_existing_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let existing = dir.path().join("existing.sqlite");
    let (storage, character, _, _) = seeded(&source)?;
    // The writer stays open in WAL mode while VACUUM INTO captures committed rows.
    storage.export_snapshot(&backup)?;
    let copied = SqliteWorldStorage::open(&backup)?;
    let key: SchemaKey = "story.card@1".parse()?;
    assert_eq!(
        copied.load_property(character, &key)?.unwrap().data()["name"],
        "Alice"
    );
    assert_eq!(copied.world_id(), storage.world_id());
    std::fs::write(&existing, b"existing bytes")?;
    assert!(storage.export_snapshot(&existing).is_err());
    assert_eq!(std::fs::read(&existing)?, b"existing bytes");
    assert!(storage.export_snapshot(&backup).is_err());
    assert!(
        SqliteWorldStorage::create_from_seed(
            &source,
            &existing,
            WorldId::new(),
            &WorldManifest::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&existing)?, b"existing bytes");
    Ok(())
}
