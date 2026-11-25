//! Shared-vault lifecycle, private filesystem and fault-injection regressions.

use super::*;
use crate::shared_secrets::files::{WRITE_FAILURE, WriteStep};
use std::{
    collections::HashMap,
    fs::{self, Permissions},
    os::unix::fs::{PermissionsExt, symlink},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
struct FakeVault {
    values: Mutex<HashMap<SecretPath, String>>,
    unavailable: AtomicBool,
    delete_fails: AtomicBool,
}

impl SecretVault for FakeVault {
    fn store(&self, path: &SecretPath, value: &SecretValue) -> Result<(), SecretAccessError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(SecretAccessError::Unavailable);
        }
        self.values
            .lock()
            .unwrap()
            .insert(path.clone(), value.expose_secret().to_owned());
        Ok(())
    }
    fn load(&self, path: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(SecretAccessError::Unavailable);
        }
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(path)
            .map(|v| SecretValue::new(v.clone())))
    }
    fn delete(&self, path: &SecretPath) -> Result<bool, SecretAccessError> {
        if self.unavailable.load(Ordering::Acquire) || self.delete_fails.load(Ordering::Acquire) {
            return Err(SecretAccessError::Unavailable);
        }
        Ok(self.values.lock().unwrap().remove(path).is_some())
    }
}

fn store(home: &HostHome, backend: SecretBackend, system: &Arc<FakeVault>) -> ManagedSecrets {
    ManagedSecrets {
        home: home.root().to_owned(),
        backend,
        system: system.clone(),
    }
}

fn local(home: &HostHome) -> ManagedSecrets {
    store(
        home,
        SecretBackend::LocalFile,
        &Arc::new(FakeVault::default()),
    )
}

fn record_path(home: &HostHome, backend: SecretBackend, id: Uuid) -> PathBuf {
    home.root()
        .join("credentials")
        .join(backend.as_str())
        .join(id.simple().to_string())
}

#[test]
fn shared_lifecycle_survives_reopen_rotation_rename_delete_and_same_label_recreate()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    home.select_secret_backend(SecretBackend::LocalFile)?;
    let sentinel = "  synthetic multiline\nprivate value\n\0";
    let entry = home.create_shared_secret("shared", &SecretValue::new(sentinel))?;
    let other = home.create_shared_secret("shared", &SecretValue::new("other"))?;
    assert_ne!(entry.id, other.id);
    let vault = local(&home);
    assert_eq!(
        vault
            .load_bound(&entry_path(entry.id))?
            .unwrap()
            .expose_secret(),
        sentinel
    );
    let metadata = serde_json::to_string(&home.list_shared_secrets()?)?;
    assert!(!metadata.contains("synthetic"));
    assert!(!format!("{:?}", home.list_shared_secrets()?).contains("private value"));
    let renamed = home.rename_shared_secret(entry.id, entry.revision, "renamed")?;
    assert_eq!(renamed.id, entry.id);
    assert_eq!(renamed.revision, 2);
    assert!(matches!(
        vault.edit(entry.id, 1, None, Some(&SecretValue::new("stale"))),
        Err(SecretStorageError::RevisionConflict)
    ));
    let replacement = home.replace_shared_secret(
        entry.id,
        renamed.revision,
        &SecretValue::new("replacement\n"),
    )?;
    let reopened = HostHome::open(root.path())?;
    assert_eq!(reopened.secret_backend()?, SecretBackend::LocalFile);
    assert_eq!(
        local(&reopened)
            .load_bound(&entry_path(entry.id))?
            .unwrap()
            .expose_secret(),
        "replacement\n"
    );
    home.delete_shared_secret(entry.id, replacement.revision)?;
    assert!(vault.load_bound(&entry_path(entry.id))?.is_none());
    let recreated = home.create_shared_secret("renamed", &SecretValue::new("new value"))?;
    assert_ne!(recreated.id, entry.id);
    assert!(vault.load_bound(&entry_path(entry.id))?.is_none());
    Ok(())
}

#[test]
fn explicit_backend_switch_does_not_copy_fall_back_or_cross_home_boundaries() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let first = HostHome::open(root.path().join("first"))?;
    let second = HostHome::open(root.path().join("second"))?;
    assert_eq!(first.secret_backend()?, SecretBackend::System);
    let system = Arc::new(FakeVault::default());
    let a = store(&first, SecretBackend::System, &system);
    let b = store(&second, SecretBackend::System, &system);
    let entry = a.create("system", &SecretValue::new("system sentinel"))?;
    assert!(
        b.load_bound(&entry_path(entry.id))?.is_none(),
        "system value exists but foreign home has no metadata"
    );
    let local_entry = local(&first).create("file", &SecretValue::new("file sentinel"))?;
    assert!(
        local(&second)
            .load_bound(&entry_path(local_entry.id))?
            .is_none()
    );
    assert!(local(&first).load_bound(&entry_path(entry.id))?.is_none());
    assert!(a.load_bound(&entry_path(local_entry.id))?.is_none());
    system.unavailable.store(true, Ordering::Release);
    assert_eq!(
        a.load_bound(&entry_path(entry.id)).unwrap_err(),
        SecretAccessError::Unavailable
    );
    assert_eq!(
        a.create("locked", &SecretValue::new("sentinel"))
            .unwrap_err(),
        SecretStorageError::Unavailable
    );
    assert_eq!(a.list()?.len(), 1);
    assert_eq!(local(&first).list()?.len(), 1);
    first.select_secret_backend(SecretBackend::LocalFile)?;
    assert_eq!(first.list_shared_secrets()?.len(), 1);
    first.select_secret_backend(SecretBackend::System)?;
    assert_eq!(first.list_shared_secrets()?.len(), 1);
    let legacy = SecretPath::parse("legacy.test")?;
    system.unavailable.store(false, Ordering::Release);
    a.store(&legacy, &SecretValue::new("legacy"))?;
    assert_eq!(b.load(&legacy)?.unwrap().expose_secret(), "legacy");
    let imported = a.import_legacy(&legacy, "imported")?;
    assert_eq!(
        a.load_entry(imported.id)?.unwrap().expose_secret(),
        "legacy"
    );
    assert_eq!(a.load(&legacy)?.unwrap().expose_secret(), "legacy");
    assert!(b.load_entry(imported.id)?.is_none());
    assert_eq!(
        local(&first)
            .import_legacy(&legacy, "imported")
            .unwrap_err(),
        SecretStorageError::InvalidInput
    );
    assert_eq!(
        local(&first).load(&legacy).unwrap_err(),
        SecretAccessError::Unavailable
    );
    let formerly_valid_direct =
        SecretPath::parse("managed.entry_00000000000000000000000000000001")?;
    a.store(
        &formerly_valid_direct,
        &SecretValue::new("legacy managed-looking path"),
    )?;
    assert_eq!(
        b.load(&formerly_valid_direct)?.unwrap().expose_secret(),
        "legacy managed-looking path"
    );
    assert!(
        b.load_bound(&formerly_valid_direct)?.is_none(),
        "a binding requires home-local metadata even when a legacy physical name exists"
    );
    Ok(())
}

#[test]
fn files_and_directories_are_private_and_unsafe_permissions_links_and_parents_are_rejected()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let vault = local(&home);
    let entry = vault.create("private", &SecretValue::new("sentinel"))?;
    let path = record_path(&home, SecretBackend::LocalFile, entry.id);
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    for directory in [
        home.root().join("credentials"),
        path.parent().unwrap().to_owned(),
    ] {
        assert_eq!(
            fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o700
        );
    }
    fs::set_permissions(&path, Permissions::from_mode(0o644))?;
    assert_eq!(vault.list().unwrap_err(), SecretStorageError::UnsafeStorage);
    fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    let target = path.with_extension("target");
    fs::rename(&path, &target)?;
    symlink(&target, &path)?;
    assert_eq!(
        vault.load_entry(entry.id).unwrap_err(),
        SecretStorageError::UnsafeStorage
    );
    fs::remove_file(&path)?;
    fs::hard_link(&target, &path)?;
    assert_eq!(
        vault.load_entry(entry.id).unwrap_err(),
        SecretStorageError::UnsafeStorage
    );
    fs::remove_file(&path)?;
    fs::rename(&target, &path)?;
    fs::set_permissions(home.root(), Permissions::from_mode(0o777))?;
    assert_eq!(vault.list().unwrap_err(), SecretStorageError::UnsafeStorage);
    fs::set_permissions(home.root(), Permissions::from_mode(0o755))?;
    let credentials = home.root().join("credentials");
    let moved = home.root().join("moved");
    fs::rename(&credentials, &moved)?;
    symlink(&moved, &credentials)?;
    assert_eq!(vault.list().unwrap_err(), SecretStorageError::UnsafeStorage);
    Ok(())
}

#[test]
fn malformed_oversize_records_and_selection_never_become_missing_or_echo_their_contents()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let vault = local(&home);
    let entry = vault.create("label", &SecretValue::new("test sentinel"))?;
    let path = record_path(&home, SecretBackend::LocalFile, entry.id);
    for invalid in [
        b"secret raw sentinel".to_vec(),
        vec![b'x'; MAX_RECORD + 1],
        br#"{"id":"wrong"}"#.to_vec(),
    ] {
        fs::write(&path, invalid)?;
        let error = vault.load_entry(entry.id).unwrap_err();
        assert_eq!(error, SecretStorageError::CorruptStorage);
        assert!(!format!("{error:?}: {error}").contains("sentinel"));
        assert_eq!(
            vault.list().unwrap_err(),
            SecretStorageError::CorruptStorage
        );
    }
    home.select_secret_backend(SecretBackend::LocalFile)?;
    fs::write(
        home.root().join("credentials/backend"),
        "raw selection sentinel",
    )?;
    assert_eq!(
        home.secret_backend().unwrap_err(),
        SecretStorageError::CorruptStorage
    );
    Ok(())
}

#[test]
fn encoded_bounds_empty_input_and_entry_quota_are_enforced_without_mutation() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let vault = local(&home);
    for (label, value) in [
        ("".to_owned(), "value".to_owned()),
        ("x".repeat(MAX_LABEL + 1), "value".to_owned()),
        ("label".to_owned(), String::new()),
        ("label".to_owned(), "x".repeat(MAX_VALUE + 1)),
    ] {
        assert_eq!(
            vault.create(&label, &SecretValue::new(value)).unwrap_err(),
            SecretStorageError::InvalidInput
        );
    }
    let escaped = "\0".repeat(MAX_VALUE);
    let entry = vault.create(&"\0".repeat(MAX_LABEL), &SecretValue::new(&escaped))?;
    assert_eq!(
        vault.load_entry(entry.id)?.unwrap().expose_secret(),
        escaped
    );
    for _ in 1..MAX_ENTRIES {
        vault.create("same", &SecretValue::new("x"))?;
    }
    assert_eq!(
        vault
            .create("overflow", &SecretValue::new("x"))
            .unwrap_err(),
        SecretStorageError::QuotaExceeded
    );
    assert_eq!(vault.list()?.len(), MAX_ENTRIES);
    Ok(())
}

#[test]
fn atomic_write_failures_preserve_old_values_or_report_publication_and_reclaim_temporaries()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let vault = local(&home);
    let entry = vault.create("label", &SecretValue::new("old"))?;
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::BeforeRename)));
    assert_eq!(
        vault
            .edit(entry.id, 1, None, Some(&SecretValue::new("new")))
            .unwrap_err(),
        SecretStorageError::Unavailable
    );
    assert_eq!(vault.load_entry(entry.id)?.unwrap().expose_secret(), "old");
    let directory = vault.directory(false)?.unwrap();
    assert!(
        !directory
            .names()?
            .iter()
            .any(|name| name.starts_with(".tmp_"))
    );
    let abandoned = record_path(&home, SecretBackend::LocalFile, entry.id)
        .with_file_name(format!(".tmp_{}", Uuid::new_v4().simple()));
    fs::write(&abandoned, "abandoned secret input")?;
    fs::set_permissions(&abandoned, Permissions::from_mode(0o600))?;
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::AfterRename)));
    assert_eq!(
        vault
            .edit(entry.id, 1, None, Some(&SecretValue::new("new")))
            .unwrap_err(),
        SecretStorageError::DurabilityUnknown
    );
    assert!(!abandoned.exists());
    assert_eq!(vault.load_entry(entry.id)?.unwrap().expose_secret(), "new");
    assert_eq!(vault.list()?[0].revision, 2);
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::AfterUnlink)));
    assert_eq!(
        vault.remove(entry.id, 2).unwrap_err(),
        SecretStorageError::DurabilityUnknown
    );
    assert!(vault.load_entry(entry.id)?.is_none());
    Ok(())
}

#[test]
fn system_stages_before_metadata_and_reports_cleanup_and_cross_store_failure() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let fake = Arc::new(FakeVault::default());
    let vault = store(&home, SecretBackend::System, &fake);
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::BeforeRename)));
    assert_eq!(
        vault
            .create("label", &SecretValue::new("value"))
            .unwrap_err(),
        SecretStorageError::Unavailable
    );
    assert!(fake.values.lock().unwrap().is_empty());
    assert!(vault.list()?.is_empty());
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::AfterRename)));
    assert_eq!(
        vault
            .create("uncertain", &SecretValue::new("value"))
            .unwrap_err(),
        SecretStorageError::DurabilityUnknown
    );
    let published = vault.list()?.pop().unwrap();
    assert_eq!(
        vault.load_entry(published.id)?.unwrap().expose_secret(),
        "value"
    );
    vault.remove(published.id, published.revision)?;
    fake.delete_fails.store(true, Ordering::Release);
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::BeforeRename)));
    assert_eq!(
        vault
            .create("orphan", &SecretValue::new("value"))
            .unwrap_err(),
        SecretStorageError::CleanupFailed
    );
    assert_eq!(fake.values.lock().unwrap().len(), 1);
    assert!(vault.list()?.is_empty());
    let entry = vault.create("published", &SecretValue::new("old"))?;
    WRITE_FAILURE.with(|f| f.set(Some(WriteStep::BeforeRename)));
    assert_eq!(
        vault
            .edit(entry.id, 1, None, Some(&SecretValue::new("new")))
            .unwrap_err(),
        SecretStorageError::DurabilityUnknown
    );
    assert_eq!(vault.load_entry(entry.id)?.unwrap().expose_secret(), "new");
    assert_eq!(
        vault.remove(entry.id, 1).unwrap_err(),
        SecretStorageError::CleanupFailed
    );
    assert!(vault.load_entry(entry.id)?.is_none());
    Ok(())
}

#[test]
fn readers_observe_complete_records_during_concurrent_replace_and_delete() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let vault = local(&home);
    let first = "α".repeat(16384);
    let second = "b\n".repeat(16384);
    let entry = vault.create("label", &SecretValue::new(&first))?;
    let writer = vault.clone();
    let id = entry.id;
    let worker = std::thread::spawn(move || {
        for revision in 1..=20 {
            writer
                .edit(
                    id,
                    revision,
                    None,
                    Some(&SecretValue::new(if revision % 2 == 0 {
                        "α".repeat(16384)
                    } else {
                        "b\n".repeat(16384)
                    })),
                )
                .unwrap();
        }
        writer.remove(id, 21).unwrap();
    });
    for _ in 0..100 {
        if let Some(value) = vault.load_entry(id)? {
            assert!(value.expose_secret() == first || value.expose_secret() == second);
        }
    }
    worker.join().unwrap();
    assert!(vault.load_entry(id)?.is_none());
    Ok(())
}

#[test]
fn cross_process_writer_child() -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("RINTAWA_SECRET_TEST_HOME") else {
        return Ok(());
    };
    let home = HostHome::open(path)?;
    for _ in 0..12 {
        local(&home).create("worker", &SecretValue::new("synthetic sentinel"))?;
    }
    Ok(())
}

#[test]
fn independent_processes_share_one_writer_lock_and_enforce_private_creation() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    // Set umask in child shells only: changing the test process umask races other tests.
    let mut workers = Vec::new();
    for _ in 0..3 {
        workers.push(std::process::Command::new("sh")
            .args(["-c", "umask 000; exec \"$1\" --exact shared_secrets::tests::cross_process_writer_child", "sh"])
            .arg(std::env::current_exe()?).env("RINTAWA_SECRET_TEST_HOME", root.path())
            .stdout(std::process::Stdio::null()).spawn()?);
    }
    for mut worker in workers {
        assert!(worker.wait()?.success());
    }
    assert_eq!(local(&home).list()?.len(), 36);
    for entry in local(&home).list()? {
        assert_eq!(
            fs::metadata(record_path(&home, SecretBackend::LocalFile, entry.id))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Ok(())
}
