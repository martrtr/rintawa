use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
};

use anyhow::{Result, anyhow};
use rintawa_artifacts::{
    ArtifactPath, ArtifactStore, ImportDisposition, RtwError, RtwLimits, pack_directory,
};
use tempfile::TempDir;

fn build_artifact(root: &TempDir, name: &str) -> Result<PathBuf> {
    let source = root.path().join(format!("{name}-source"));
    fs::create_dir_all(source.join("assets"))?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(source.join("manifest.toml"), format!("id = \"{name}\"\n"))?;
    fs::write(source.join("assets/data.txt"), format!("{name}-data\n"))?;
    let artifact = root.path().join(format!("{name}.rtw"));
    pack_directory(&source, &artifact, RtwLimits::default())?;
    Ok(artifact)
}

fn stored_path(store: &ArtifactStore, digest_hex: &str) -> PathBuf {
    store
        .root()
        .join("sha256")
        .join(format!("{digest_hex}.rtw"))
}
#[test]
fn test_should_import_valid_rtw_under_its_sha256_digest() -> Result<()> {
    let root = TempDir::new()?;
    let artifact = build_artifact(&root, "example")?;
    let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;

    let imported = store.import(&artifact)?;
    let expected_digest = rintawa_artifacts::ArtifactDigest::sha256(&fs::read(&artifact)?);
    assert_eq!(imported.digest(), &expected_digest);
    assert_eq!(imported.disposition(), ImportDisposition::Imported);
    let expected_path = stored_path(&store, &imported.digest().hex());
    assert!(expected_path.is_file());

    let mut archive = store.open_artifact(imported.digest())?;
    assert_eq!(
        archive.manifest().content.to_string(),
        "rintawa.extension@1"
    );
    let data = ArtifactPath::parse("assets/data.txt")?;
    assert_eq!(archive.read(&data)?, b"example-data\n");
    Ok(())
}

#[test]
fn test_should_deduplicate_repeated_imports_without_rewriting() -> Result<()> {
    let root = TempDir::new()?;
    let artifact = build_artifact(&root, "same")?;
    let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;

    let first = store.import(&artifact)?;
    let second = store.import(&artifact)?;
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.disposition(), ImportDisposition::Imported);
    assert_eq!(second.disposition(), ImportDisposition::AlreadyPresent);
    assert_eq!(fs::read_dir(store.root().join("sha256"))?.count(), 1);
    Ok(())
}
#[test]
fn test_should_store_copied_bytes_independently_from_source_changes() -> Result<()> {
    let root = TempDir::new()?;
    let artifact = build_artifact(&root, "stable")?;
    let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;
    let imported = store.import(&artifact)?;

    fs::write(&artifact, b"source changed after import")?;

    let mut archive = store.open_artifact(imported.digest())?;
    let data = ArtifactPath::parse("assets/data.txt")?;
    assert_eq!(archive.read(&data)?, b"stable-data\n");
    Ok(())
}

#[test]
fn test_should_not_publish_invalid_rtw_bytes() -> Result<()> {
    let root = TempDir::new()?;
    let invalid = root.path().join("invalid.rtw");
    fs::write(&invalid, b"not a zip")?;
    let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;

    assert!(store.import(&invalid).is_err());
    assert_eq!(fs::read_dir(store.root().join("sha256"))?.count(), 0);
    assert_eq!(fs::read_dir(store.root().join("tmp"))?.count(), 0);
    Ok(())
}
#[test]
fn test_should_detect_corruption_in_an_existing_digest_entry() -> Result<()> {
    let root = TempDir::new()?;
    let artifact = build_artifact(&root, "corruption")?;
    let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;
    let imported = store.import(&artifact)?;
    let path = stored_path(&store, &imported.digest().hex());

    fs::write(&path, b"corrupt")?;

    assert!(matches!(
        store.verify(imported.digest()),
        Err(RtwError::StoreCorruption(digest)) if digest == imported.digest().to_string()
    ));
    assert!(matches!(
        store.import(&artifact),
        Err(RtwError::StoreCorruption(digest)) if digest == imported.digest().to_string()
    ));
    assert_eq!(fs::read(&path)?, b"corrupt");
    Ok(())
}

#[test]
fn test_should_converge_concurrent_equal_imports_on_one_store_object() -> Result<()> {
    let root = TempDir::new()?;
    let artifact = Arc::new(build_artifact(&root, "concurrent")?);
    let store = Arc::new(ArtifactStore::open(
        root.path().join("store"),
        RtwLimits::default(),
    )?);
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let artifact = Arc::clone(&artifact);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            store.import(artifact.as_ref())
        }));
    }

    let first = handles
        .remove(0)
        .join()
        .map_err(|_| anyhow!("first import thread panicked"))??;
    let second = handles
        .remove(0)
        .join()
        .map_err(|_| anyhow!("second import thread panicked"))??;

    assert_eq!(first.digest(), second.digest());
    let dispositions = [first.disposition(), second.disposition()];
    assert!(dispositions.contains(&ImportDisposition::Imported));
    assert!(dispositions.contains(&ImportDisposition::AlreadyPresent));
    assert_eq!(fs::read_dir(store.root().join("sha256"))?.count(), 1);
    store.verify(first.digest())?;
    Ok(())
}
#[test]
fn test_should_use_canonical_algorithm_qualified_digest_text() -> Result<()> {
    let digest = rintawa_artifacts::ArtifactDigest::sha256(b"abc");
    assert_eq!(
        digest.to_string(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        rintawa_artifacts::ArtifactDigest::parse(digest.to_string())?,
        digest
    );
    assert!(
        rintawa_artifacts::ArtifactDigest::parse(
            "sha256:BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn test_should_reject_non_directory_reserved_store_path() -> Result<()> {
    let root = TempDir::new()?;
    let store_root = root.path().join("store");
    fs::create_dir_all(&store_root)?;
    fs::write(store_root.join("sha256"), b"not a directory")?;

    assert!(matches!(
        ArtifactStore::open(&store_root, RtwLimits::default()),
        Err(RtwError::InvalidStoreEntry { .. })
    ));
    Ok(())
}
