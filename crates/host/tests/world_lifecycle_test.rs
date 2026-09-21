use rintawa_host::{HostError, HostHome};
use rintawa_sdk::world::WorldId;

#[test]
fn test_should_create_list_and_restore_world_across_host_restart() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");

    let home = HostHome::open(&home_path)?;
    let created = home.create_world()?;
    assert_eq!(created.commit_position, 0);

    let listed = home.list_worlds()?;
    assert_eq!(listed, vec![created]);
    drop(home);

    let reopened = HostHome::open(&home_path)?;
    let state = reopened.load_world_state(created.id)?;
    assert_eq!(state.id(), created.id);
    assert_eq!(state.commit_position(), 0);
    assert!(state.schemas().is_empty());
    assert_eq!(reopened.list_worlds()?, vec![created]);
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
