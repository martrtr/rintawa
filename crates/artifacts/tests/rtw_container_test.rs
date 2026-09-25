//! Integration tests for bounded RTW container packing and validation.

use std::{fs, io::Write};

use anyhow::Result;
use rintawa_artifacts::{
    ArtifactPath, ContentType, RTW_FORMAT_VERSION, RtwArchive, RtwError, RtwLimits, RtwManifest,
    RtwPackEntry, pack_directory, pack_entries,
};
use tempfile::TempDir;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter, write::SimpleFileOptions};

fn write_source(root: &TempDir) -> Result<std::path::PathBuf> {
    let source = root.path().join("source");
    fs::create_dir_all(source.join("assets"))?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(source.join("manifest.toml"), "id = \"example\"\n")?;
    fs::write(source.join("assets/data.txt"), "portable\n")?;
    Ok(source)
}

fn example_manifest(entry: &str) -> Result<RtwManifest> {
    Ok(RtwManifest {
        format: RTW_FORMAT_VERSION,
        content: ContentType::parse("rintawa.extension@1")?,
        entry: ArtifactPath::parse(entry)?,
    })
}

fn memory_entries() -> Result<Vec<RtwPackEntry>> {
    Ok(vec![
        RtwPackEntry::new(ArtifactPath::parse("assets/data.txt")?, b"portable\n"),
        RtwPackEntry::new(ArtifactPath::parse("manifest.toml")?, b"id = \"example\"\n"),
    ])
}

fn write_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in entries {
        writer.start_file(*name, options)?;
        writer.write_all(bytes)?;
    }
    writer.finish()?;
    Ok(())
}

#[test]
fn test_should_pack_deterministically_with_stable_zip_metadata() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let first = root.path().join("first.rtw");
    let second = root.path().join("second.rtw");

    pack_directory(&source, &first, RtwLimits::default())?;
    pack_directory(&source, &second, RtwLimits::default())?;

    assert_eq!(fs::read(&first)?, fs::read(&second)?);
    let mut archive = ZipArchive::new(fs::File::open(&first)?)?;
    assert_eq!(archive.len(), 3);
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        assert_eq!(entry.last_modified(), Some(DateTime::DEFAULT));
        assert_eq!(entry.unix_mode().map(|mode| mode & 0o777), Some(0o644));
    }
    Ok(())
}

#[test]
fn test_should_pack_memory_entries_identically_to_directory_source() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let output = root.path().join("directory.rtw");
    pack_directory(&source, &output, RtwLimits::default())?;

    let bytes = pack_entries(
        &example_manifest("manifest.toml")?,
        memory_entries()?,
        RtwLimits::default(),
    )?;

    assert_eq!(bytes, fs::read(output)?);
    Ok(())
}

#[test]
fn test_should_reject_duplicate_memory_pack_entries() -> Result<()> {
    let path = ArtifactPath::parse("manifest.toml")?;
    let error = pack_entries(
        &example_manifest("manifest.toml")?,
        [
            RtwPackEntry::new(path.clone(), b"first"),
            RtwPackEntry::new(path, b"second"),
        ],
        RtwLimits::default(),
    )
    .expect_err("duplicate memory entries must be rejected");

    assert!(matches!(error, RtwError::DuplicateEntry(path) if path == "manifest.toml"));
    Ok(())
}

#[test]
fn test_should_reject_caller_supplied_memory_manifest_entry() -> Result<()> {
    let error = pack_entries(
        &example_manifest("manifest.toml")?,
        [
            RtwPackEntry::new(ArtifactPath::parse("rtw.toml")?, b"shadow"),
            RtwPackEntry::new(ArtifactPath::parse("manifest.toml")?, b"content"),
        ],
        RtwLimits::default(),
    )
    .expect_err("the packer must own the root manifest entry");

    assert!(matches!(error, RtwError::DuplicateEntry(path) if path == "rtw.toml"));
    Ok(())
}

#[test]
fn test_should_reject_memory_pack_file_prefix_conflicts() -> Result<()> {
    let error = pack_entries(
        &example_manifest("assets/data.txt")?,
        [
            RtwPackEntry::new(ArtifactPath::parse("assets")?, b"file"),
            RtwPackEntry::new(ArtifactPath::parse("assets/data.txt")?, b"nested"),
        ],
        RtwLimits::default(),
    )
    .expect_err("file-prefix conflict must be rejected");

    assert!(matches!(error, RtwError::PathConflict(path) if path == "assets"));
    Ok(())
}

#[test]
fn test_should_reject_memory_pack_missing_declared_entry_and_limits() -> Result<()> {
    assert!(matches!(
        pack_entries(
            &example_manifest("missing.toml")?,
            memory_entries()?,
            RtwLimits::default(),
        ),
        Err(RtwError::MissingContentEntry(path)) if path == "missing.toml"
    ));

    let limits = RtwLimits {
        max_archive_bytes: 1,
        ..RtwLimits::default()
    };
    assert!(matches!(
        pack_entries(
            &example_manifest("manifest.toml")?,
            memory_entries()?,
            limits
        ),
        Err(RtwError::ArchiveTooLarge { .. })
    ));
    Ok(())
}

#[test]
fn test_should_remove_temporary_archive_after_post_pack_validation_failure() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let output = root.path().join("output.rtw");
    let limits = RtwLimits {
        max_archive_bytes: 1,
        ..RtwLimits::default()
    };

    let before = fs::read_dir(root.path())?.count();
    assert!(matches!(
        pack_directory(&source, &output, limits),
        Err(RtwError::ArchiveTooLarge { .. })
    ));
    let after = fs::read_dir(root.path())?.count();

    assert_eq!(after, before);
    assert!(!output.exists());
    Ok(())
}

#[test]
fn test_should_reject_unsafe_zip_paths() -> Result<()> {
    let root = TempDir::new()?;
    let archive_path = root.path().join("unsafe.rtw");
    write_zip(
        &archive_path,
        &[
            (
                "rtw.toml",
                b"format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
            ),
            ("manifest.toml", b"id = \"example\"\n"),
            ("../escape", b"nope"),
        ],
    )?;

    assert!(matches!(
        RtwArchive::open(&archive_path, RtwLimits::default()),
        Err(RtwError::InvalidPath { .. })
    ));
    Ok(())
}

#[test]
fn test_should_reject_missing_declared_content_entry() -> Result<()> {
    let root = TempDir::new()?;
    let archive_path = root.path().join("missing-entry.rtw");
    write_zip(
        &archive_path,
        &[(
            "rtw.toml",
            b"format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"missing.toml\"\n",
        )],
    )?;

    assert!(matches!(
        RtwArchive::open(&archive_path, RtwLimits::default()),
        Err(RtwError::MissingContentEntry(path)) if path == "missing.toml"
    ));
    Ok(())
}

#[test]
fn test_should_read_validated_entry_without_extracting_archive() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let output = root.path().join("example.rtw");
    pack_directory(&source, &output, RtwLimits::default())?;

    let mut archive = RtwArchive::open(&output, RtwLimits::default())?;
    assert_eq!(
        archive.manifest().content.to_string(),
        "rintawa.extension@1"
    );
    let entry = ArtifactPath::parse("assets/data.txt")?;
    assert_eq!(archive.read(&entry)?, b"portable\n");
    Ok(())
}

#[test]
fn test_should_reject_caller_bounded_read_before_decompression() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let output = root.path().join("bounded-read.rtw");
    pack_directory(&source, &output, RtwLimits::default())?;

    let mut archive = RtwArchive::open(&output, RtwLimits::default())?;
    let entry = ArtifactPath::parse("assets/data.txt")?;
    let error = archive
        .read_with_limit(&entry, 4)
        .expect_err("declared entry above caller limit must be rejected");

    assert!(matches!(
        error,
        RtwError::EntryTooLarge {
            path,
            actual,
            maximum,
        } if path == "assets/data.txt" && actual == 9 && maximum == 4
    ));
    Ok(())
}

#[test]
fn test_should_support_serial_reads_across_archive_fork() -> Result<()> {
    let root = TempDir::new()?;
    let source = write_source(&root)?;
    let output = root.path().join("forked-read.rtw");
    pack_directory(&source, &output, RtwLimits::default())?;

    let mut original = RtwArchive::open(&output, RtwLimits::default())?;
    let mut fork = original.fork()?;
    let entry = ArtifactPath::parse("assets/data.txt")?;

    assert_eq!(original.read(&entry)?, b"portable\n");
    assert_eq!(fork.read(&entry)?, b"portable\n");
    assert_eq!(original.read(&entry)?, b"portable\n");
    Ok(())
}
