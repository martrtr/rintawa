//! Integration tests for Host-managed world lifecycle behavior.

use rintawa_host::{HostError, HostHome};
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
    assert_eq!(
        state.manifest().as_value()["metadata"]["title"],
        "New World"
    );
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
fn test_should_persist_world_metadata_in_manifest_across_restart() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;

    let renamed = home.update_world_metadata(
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
    let reopened_manifest = reopened.load_world_state(created.id)?;
    let metadata = &reopened_manifest.manifest().as_value()["metadata"];
    assert_eq!(metadata["title"], "Tavern Night");
    assert_eq!(metadata["description"], "A warm tavern");
    Ok(())
}

#[test]
fn test_should_persist_verified_cover_asset_reference_after_restart() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let world = home.create_world()?;
    let import = home
        .asset_store()
        .import_bytes(b"immutable cover bytes", "image/png".to_string())?;
    let cover = import.asset_ref();
    let selected =
        home.update_world_metadata(world.id, "Covered World", None, Some(cover.clone()))?;
    assert_eq!(selected.cover, Some(cover.clone()));

    let mut incorrect = cover.clone();
    incorrect.size += 1;
    assert!(matches!(
        home.update_world_metadata(world.id, "Wrong Cover", None, Some(incorrect)),
        Err(HostError::Asset(_))
    ));
    assert_eq!(home.list_worlds()?[0].title, "Covered World");
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let restored = &reopened.list_worlds()?[0];
    assert_eq!(restored.title, "Covered World");
    assert_eq!(restored.cover.as_ref(), Some(cover));
    reopened.asset_store().verify(cover)?;
    Ok(())
}

#[test]
fn test_should_list_worlds_without_creating_catalog_sidecars() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let world = home.create_world()?;
    let directory = home.root().join("worlds").join(world.id.to_string());
    let sidecar = directory.join("catalog.toml");
    assert!(!sidecar.exists());

    let database = directory.join("world.sqlite");
    let original_bytes = std::fs::read(&database)?;
    let listed = home.list_worlds()?;
    assert_eq!(listed, vec![world]);
    assert!(!sidecar.exists());
    assert_eq!(std::fs::read(&database)?, original_bytes);
    Ok(())
}

#[test]
fn test_should_preserve_other_manifest_data_when_updating_metadata() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let world = home.create_world()?;
    let database = home
        .root()
        .join("worlds")
        .join(world.id.to_string())
        .join("world.sqlite");
    let storage = SqliteWorldStorage::open(&database)?;
    let previous = storage.load_session()?.manifest().as_value().clone();
    let mut document = previous
        .as_object()
        .expect("host manifest must be an object")
        .clone();
    document.insert("custom".to_string(), serde_json::json!({"keep": true}));
    storage.update_manifest(&rintawa_world::WorldManifest::new(
        serde_json::Value::Object(document),
    ))?;
    drop(storage);
    let updated = home.update_world_metadata(world.id, "Renamed", None, None)?;
    assert_eq!(updated.title, "Renamed");
    let manifest = home.load_world_state(world.id)?;
    assert_eq!(manifest.manifest().as_value()["custom"]["keep"], true);
    assert_eq!(
        manifest.manifest().as_value()["metadata"]["title"],
        "Renamed"
    );
    Ok(())
}

#[test]
fn test_should_fail_closed_for_world_without_manifest_metadata() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let world = home.create_world()?;
    let database = home
        .root()
        .join("worlds")
        .join(world.id.to_string())
        .join("world.sqlite");
    SqliteWorldStorage::open(&database)?
        .update_manifest(&rintawa_world::WorldManifest::default())?;
    let before = std::fs::read(&database)?;
    assert!(matches!(
        home.list_worlds(),
        Err(HostError::InvalidWorldMetadata(_))
    ));
    assert_eq!(std::fs::read(&database)?, before);
    assert!(!database.with_file_name("catalog.toml").exists());
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

#[test]
fn test_should_preserve_item_asset_ref_and_relation_across_restart() -> anyhow::Result<()> {
    use std::io::Read;

    use rintawa_artifacts::AssetRef;
    use rintawa_host::HostRuntime;
    use rintawa_sdk::world::ItemId;
    use rintawa_world::{StateChange, WorldTransaction};

    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let world = home.create_world()?;

    // CAS must finish publishing and validating immutable bytes before any
    // durable World property refers to them. Core cannot infer domain-specific
    // AssetRefs from arbitrary extension-owned JSON without an asset contract.
    let bytes = b"portrait content stored before committing the World reference";
    let imported = home
        .asset_store()
        .import_bytes(bytes, "image/png".to_string())?;
    let asset = imported.asset_ref().clone();
    home.asset_store().verify(&asset)?;

    let property_key: SchemaKey = "character.portrait@1".parse()?;
    let storage_path = home
        .root()
        .join("worlds")
        .join(world.id.to_string())
        .join("world.sqlite");
    let storage = SqliteWorldStorage::open(&storage_path)?;
    storage.register_definition(&PropertyDefinition::new(
        property_key.clone(),
        ExtensionId::new("test.character"),
        serde_json::json!({
            "type": "object",
            "properties": {
                "asset": {
                    "type": "object",
                    "required": ["digest", "size", "media_type"]
                }
            },
            "required": ["asset"],
            "additionalProperties": false
        }),
    ))?;

    let character = ItemId::new();
    let location = ItemId::new();
    let link = ItemId::new();
    let mut changes = WorldTransaction::new();
    changes.push_change(StateChange::CreateObject { id: character });
    changes.push_change(StateChange::CreateObject { id: location });
    changes.push_change(StateChange::CreateRelation {
        id: link,
        subject: character,
        object: location,
    });
    changes.push_change(StateChange::SetProperty {
        item: character,
        key: property_key.clone(),
        data: serde_json::json!({ "asset": asset }),
    });
    storage.apply(&changes)?;
    drop(storage);

    let mut runtime = HostRuntime::start(&home)?;
    runtime.activate_world(&home, world.id)?;
    runtime.deactivate_world(world.id)?;
    runtime.shutdown()?;
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let storage = SqliteWorldStorage::open(&storage_path)?;
    let snapshot = storage.snapshot()?;
    let restored = snapshot
        .load_property(character, &property_key)?
        .expect("character asset reference must survive the Host restart");
    let restored_asset: AssetRef = serde_json::from_value(restored.data()["asset"].clone())?;
    assert_eq!(restored_asset, asset);

    reopened.asset_store().verify(&restored_asset)?;
    let mut file = reopened.asset_store().open_asset(&restored_asset)?;
    let mut restored_bytes = Vec::new();
    file.read_to_end(&mut restored_bytes)?;
    assert_eq!(restored_bytes, bytes);

    let relations = snapshot.relations_touching_any(&[character, location], None, 10)?;
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].id(), link);
    assert_eq!(relations[0].subject(), Some(character));
    assert_eq!(relations[0].object_target(), Some(location));

    drop(snapshot);
    drop(storage);
    let mut runtime = HostRuntime::start(&reopened)?;
    runtime.activate_world(&reopened, world.id)?;
    runtime.shutdown()?;
    assert_eq!(reopened.list_worlds()?, vec![world]);
    Ok(())
}
