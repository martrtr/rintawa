//! Integration tests for Host-managed world lifecycle behavior.

use rintawa_host::{HostError, HostHome, WORLD_CATALOG_METADATA_FILE};
use rintawa_sdk::{
    types::ExtensionId,
    world::{SchemaKey, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::PropertyDefinition;

#[test]
fn test_should_create_list_and_restore_world_across_host_restart() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");

    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;
    assert_eq!(created.title, "New World");
    assert!(created.cover.is_none());

    let listed = home.list_worlds()?;
    assert_eq!(listed, vec![created.clone()]);
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let state = reopened.load_world_state(created.id)?;
    assert_eq!(state.id(), created.id);
    assert!(state.properties().is_empty());
    assert_eq!(state.manifest().as_value(), &serde_json::json!({}));
    assert_eq!(reopened.list_worlds()?, vec![created]);
    Ok(())
}

#[test]
fn test_should_reopen_world_with_its_persisted_property_vocabulary() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;
    let database = home
        .root()
        .join("worlds")
        .join(created.id.to_string())
        .join("world.sqlite");
    let key: SchemaKey = "example.identity@1".parse()?;
    let definition = PropertyDefinition::new(
        key.clone(),
        ExtensionId::new("example.owner"),
        serde_json::json!({ "type": "object" }),
    );
    {
        let storage = SqliteWorldStorage::open(&database)?;
        storage.register_definition(&definition)?;
    }

    let state = home.load_world_state(created.id)?;
    // `property_registry` stores name, version and schema only; the owning package
    // is Host composition metadata and is not persisted with the world.
    assert_eq!(
        state.properties().get(&key).map(PropertyDefinition::schema),
        Some(&serde_json::json!({ "type": "object" }))
    );
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let restored = reopened.load_world_state(created.id)?;
    assert_eq!(restored.properties().len(), 1);
    assert_eq!(
        restored
            .properties()
            .get(&key)
            .map(PropertyDefinition::schema),
        Some(&serde_json::json!({ "type": "object" }))
    );
    Ok(())
}

#[test]
fn test_should_persist_renamed_world_catalog_metadata_across_restart() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;

    let renamed = home.update_world_catalog_metadata(
        created.id,
        "  Tavern Night  ",
        Some(String::from("  A warm tavern  ")),
        None,
    )?;
    assert_eq!(renamed.title, "Tavern Night");
    assert_eq!(renamed.description.as_deref(), Some("A warm tavern"));
    assert_eq!(home.list_worlds()?[0].title, "Tavern Night");
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let reopened_world = &reopened.list_worlds()?[0];
    assert_eq!(reopened_world.title, "Tavern Night");
    assert_eq!(reopened_world.description.as_deref(), Some("A warm tavern"));
    let reopened_metadata = reopened.load_world_catalog_metadata(created.id)?;
    assert_eq!(reopened_metadata.title(), "Tavern Night");
    assert_eq!(reopened_metadata.description(), Some("A warm tavern"));
    Ok(())
}

#[test]
fn test_should_migrate_world_without_catalog_sidecar_on_first_listing() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;
    let metadata = home
        .root()
        .join("worlds")
        .join(created.id.to_string())
        .join(WORLD_CATALOG_METADATA_FILE);
    std::fs::remove_file(&metadata)?;

    let worlds = home.list_worlds()?;
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].title, "World 1");
    assert!(metadata.is_file());
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    assert_eq!(reopened.list_worlds()?[0].title, "World 1");
    Ok(())
}

#[test]
fn test_should_delete_world_owned_storage_and_composition() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let created = home.create_world()?;
    let world_directory = home.root().join("worlds").join(created.id.to_string());
    let composition = world_directory.join("composition.toml");
    assert!(world_directory.is_dir());
    assert!(composition.is_file());

    home.delete_world(created.id)?;

    assert!(!world_directory.exists());
    assert!(home.list_worlds()?.is_empty());
    assert!(matches!(
        home.load_world_state(created.id),
        Err(HostError::WorldNotFound(id)) if id == created.id
    ));
    Ok(())
}

#[test]
fn test_should_report_missing_world_without_creating_storage() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let missing = WorldId::new();

    let error = home.load_world_state(missing).unwrap_err();
    assert!(matches!(error, HostError::WorldNotFound(id) if id == missing));
    assert!(home.list_worlds()?.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_directory_database_world_id_mismatch() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let created = home.create_world()?;
    let replacement = WorldId::new();

    let worlds = home.root().join("worlds");
    std::fs::rename(
        worlds.join(created.id.to_string()),
        worlds.join(replacement.to_string()),
    )?;

    let error = home.list_worlds().unwrap_err();
    assert!(matches!(
        error,
        HostError::WorldDirectoryIdMismatch {
            directory_id,
            database_id,
        } if directory_id == replacement && database_id == created.id
    ));
    Ok(())
}
