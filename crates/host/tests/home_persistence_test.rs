//! Regression tests for the reduced HostHome persistence boundary.

use rintawa_host::HostHome;

#[test]
fn test_should_keep_cas_and_world_storage_without_legacy_content_index() -> anyhow::Result<()> {
    let temporary = tempfile::tempdir()?;
    let home = HostHome::open(temporary.path().join("home"))?;

    assert!(home.root().join("artifacts").is_dir());
    assert!(home.root().join("assets").is_dir());
    assert!(home.root().join("worlds").is_dir());
    assert!(!home.root().join("content").exists());

    let created = home.create_world()?;
    assert_eq!(home.list_worlds()?.len(), 1);
    assert_eq!(home.list_worlds()?[0].id, created.id);
    assert!(!home.root().join("content").exists());
    Ok(())
}

#[test]
fn test_should_preserve_unmanaged_legacy_content_files_without_loading_them() -> anyhow::Result<()>
{
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("home");
    let directory = root.join("content");
    std::fs::create_dir_all(&directory)?;
    let index = directory.join("library.toml");
    let legacy_bytes = b"legacy data: untouched by the reduced host";
    std::fs::write(&index, legacy_bytes)?;

    let home = HostHome::open(&root)?;
    assert_eq!(std::fs::read(index)?, legacy_bytes);
    assert!(home.root().join("artifacts").is_dir());
    Ok(())
}

#[test]
fn test_should_not_create_persistent_host_principal() -> anyhow::Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("home");
    let home = HostHome::open(&root)?;
    assert!(!home.root().join("identity.toml").exists());
    home.create_world()?;
    drop(home);

    let restarted = HostHome::open(&root)?;
    assert!(!restarted.root().join("identity.toml").exists());
    assert_eq!(restarted.list_worlds()?.len(), 1);
    Ok(())
}

#[test]
fn test_should_ignore_old_identity_file_without_rewriting_it() -> anyhow::Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("home");
    std::fs::create_dir_all(&root)?;
    let identity = root.join("identity.toml");
    let bytes = b"legacy identity: not a source of authority";
    std::fs::write(&identity, bytes)?;

    let home = HostHome::open(&root)?;
    assert_eq!(std::fs::read(&identity)?, bytes);
    home.create_world()?;
    assert_eq!(std::fs::read(&identity)?, bytes);
    Ok(())
}
