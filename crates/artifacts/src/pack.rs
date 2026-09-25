//! Deterministic RTW ZIP packing.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use crate::{
    ArtifactPath, RTW_MANIFEST_PATH, RtwArchive, RtwError, RtwLimits, RtwManifest, RtwResult,
    archive::validate_bytes, path::parent_paths,
};

/// Packs one RTW-layout directory into a deterministic ZIP container.
///
/// The source tree must contain a valid root `rtw.toml`. Files are written in
/// canonical path order with fixed ZIP timestamps and permissions. Existing
/// output files are never overwritten.
///
/// # Errors
///
/// Returns an RTW error for an invalid source tree, unsupported filesystem
/// entries, host-limit violations, ZIP failures, or an existing destination.
pub fn pack_directory(
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    limits: RtwLimits,
) -> RtwResult<()> {
    let source = source.as_ref().canonicalize()?;
    if !source.is_dir() {
        return Err(RtwError::SourceNotDirectory(source.display().to_string()));
    }

    let output = absolute_output_path(output.as_ref())?;
    if output.starts_with(&source) {
        return Err(RtwError::OutputInsideSource(output.display().to_string()));
    }
    if output.exists() {
        return Err(RtwError::OutputAlreadyExists(output.display().to_string()));
    }

    let files = collect_source_files(&source)?;
    enforce_source_limits(&files, limits)?;
    validate_source_manifest(&source, &files)?;
    write_archive(&output, &files, limits)
}

/// One regular file supplied to the in-memory RTW packer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtwPackEntry {
    path: ArtifactPath,
    bytes: Vec<u8>,
}

impl RtwPackEntry {
    /// Creates one already-path-validated RTW file entry.
    pub fn new(path: ArtifactPath, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path,
            bytes: bytes.into(),
        }
    }

    /// Returns the canonical path inside the artifact.
    pub fn path(&self) -> &ArtifactPath {
        &self.path
    }

    /// Returns the exact uncompressed file bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Packs a validated manifest and regular-file entries into deterministic RTW bytes.
///
/// The root `rtw.toml` is generated from `manifest`; callers must not provide it as
/// an entry. Input paths are canonical [`ArtifactPath`] values, and the same RTW
/// entry/count/size/path-conflict limits used by filesystem packing are enforced.
/// Output ZIP metadata and ordering match [`pack_directory`].
///
/// # Errors
///
/// Returns an RTW error for an invalid manifest, duplicate or conflicting paths,
/// a missing declared content entry, configured limit violations, ZIP failures,
/// or post-pack validation failure.
pub fn pack_entries(
    manifest: &RtwManifest,
    entries: impl IntoIterator<Item = RtwPackEntry>,
    limits: RtwLimits,
) -> RtwResult<Vec<u8>> {
    manifest.validate()?;
    let manifest_bytes = toml::to_string(manifest)?.into_bytes();
    let mut entries = entries.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    validate_memory_entries(manifest, &manifest_bytes, &entries, limits)?;

    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = archive_options();
    writer.start_file(RTW_MANIFEST_PATH, options)?;
    writer.write_all(&manifest_bytes)?;
    for entry in &entries {
        writer.start_file(entry.path.as_str(), options)?;
        writer.write_all(&entry.bytes)?;
    }
    let bytes = writer.finish()?.into_inner();
    let archive_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if archive_bytes > limits.max_archive_bytes {
        return Err(RtwError::ArchiveTooLarge {
            actual: archive_bytes,
            maximum: limits.max_archive_bytes,
        });
    }
    validate_bytes(&bytes, limits)?;
    Ok(bytes)
}
#[derive(Debug)]
struct SourceFile {
    filesystem_path: PathBuf,
    artifact_path: ArtifactPath,
    size: u64,
}

fn absolute_output_path(output: &Path) -> RtwResult<PathBuf> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };
    let file_name = absolute.file_name().ok_or_else(|| RtwError::InvalidPath {
        path: absolute.display().to_string(),
        reason: "output path must include a file name",
    })?;
    let parent = absolute.parent().ok_or_else(|| RtwError::InvalidPath {
        path: absolute.display().to_string(),
        reason: "output path must have a parent directory",
    })?;
    fs::create_dir_all(parent)?;
    Ok(parent.canonicalize()?.join(file_name))
}

fn collect_source_files(source: &Path) -> RtwResult<Vec<SourceFile>> {
    let mut files = Vec::new();
    collect_directory(source, source, &mut files)?;
    files.sort_by(|left, right| left.artifact_path.cmp(&right.artifact_path));
    Ok(files)
}

fn collect_directory(root: &Path, directory: &Path, files: &mut Vec<SourceFile>) -> RtwResult<()> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);

    for child in children {
        let filesystem_path = child.path();
        let metadata = fs::symlink_metadata(&filesystem_path)?;
        if metadata.file_type().is_symlink() {
            return Err(RtwError::UnsupportedSourceEntry(
                filesystem_path.display().to_string(),
            ));
        }
        if metadata.is_dir() {
            collect_directory(root, &filesystem_path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(RtwError::UnsupportedSourceEntry(
                filesystem_path.display().to_string(),
            ));
        }
        let relative = filesystem_path
            .strip_prefix(root)
            .map_err(|_| RtwError::UnsupportedSourceEntry(filesystem_path.display().to_string()))?;
        let artifact_path = source_path_to_artifact_path(relative)?;
        files.push(SourceFile {
            filesystem_path,
            artifact_path,
            size: metadata.len(),
        });
    }
    Ok(())
}

fn source_path_to_artifact_path(path: &Path) -> RtwResult<ArtifactPath> {
    let mut segments = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(RtwError::InvalidPath {
                path: path.display().to_string(),
                reason: "source path is not relative and canonical",
            });
        };
        let segment = segment.to_str().ok_or_else(|| RtwError::InvalidPath {
            path: path.display().to_string(),
            reason: "artifact paths must be valid UTF-8",
        })?;
        segments.push(segment);
    }
    ArtifactPath::parse(segments.join("/"))
}

fn enforce_source_limits(files: &[SourceFile], limits: RtwLimits) -> RtwResult<()> {
    if files.len() > limits.max_entries {
        return Err(RtwError::TooManyEntries {
            actual: files.len(),
            maximum: limits.max_entries,
        });
    }
    let mut total = 0_u64;
    for file in files {
        if file.size > limits.max_entry_bytes {
            return Err(RtwError::EntryTooLarge {
                path: file.artifact_path.to_string(),
                actual: file.size,
                maximum: limits.max_entry_bytes,
            });
        }
        if file.artifact_path.as_str() == RTW_MANIFEST_PATH && file.size > limits.max_manifest_bytes
        {
            return Err(RtwError::ManifestTooLarge {
                actual: file.size,
                maximum: limits.max_manifest_bytes,
            });
        }
        total = total.saturating_add(file.size);
        if total > limits.max_uncompressed_bytes {
            return Err(RtwError::UncompressedArchiveTooLarge {
                actual: total,
                maximum: limits.max_uncompressed_bytes,
            });
        }
    }
    Ok(())
}

fn validate_memory_entries(
    manifest: &RtwManifest,
    manifest_bytes: &[u8],
    entries: &[RtwPackEntry],
    limits: RtwLimits,
) -> RtwResult<()> {
    let entry_count = entries.len().saturating_add(1);
    if entry_count > limits.max_entries {
        return Err(RtwError::TooManyEntries {
            actual: entry_count,
            maximum: limits.max_entries,
        });
    }

    let manifest_size = u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX);
    enforce_entry_size(RTW_MANIFEST_PATH, manifest_size, limits.max_entry_bytes)?;
    if manifest_size > limits.max_manifest_bytes {
        return Err(RtwError::ManifestTooLarge {
            actual: manifest_size,
            maximum: limits.max_manifest_bytes,
        });
    }

    let mut paths = BTreeSet::from([RTW_MANIFEST_PATH]);
    let mut total = manifest_size;
    if total > limits.max_uncompressed_bytes {
        return Err(RtwError::UncompressedArchiveTooLarge {
            actual: total,
            maximum: limits.max_uncompressed_bytes,
        });
    }
    for entry in entries {
        if !paths.insert(entry.path.as_str()) {
            return Err(RtwError::DuplicateEntry(entry.path.to_string()));
        }
        let size = u64::try_from(entry.bytes.len()).unwrap_or(u64::MAX);
        enforce_entry_size(entry.path.as_str(), size, limits.max_entry_bytes)?;
        total = total.saturating_add(size);
        if total > limits.max_uncompressed_bytes {
            return Err(RtwError::UncompressedArchiveTooLarge {
                actual: total,
                maximum: limits.max_uncompressed_bytes,
            });
        }
    }
    for path in &paths {
        if let Some(parent) = parent_paths(path).find(|parent| paths.contains(*parent)) {
            return Err(RtwError::PathConflict(parent.to_string()));
        }
    }
    if !paths.contains(manifest.entry.as_str()) {
        return Err(RtwError::MissingContentEntry(manifest.entry.to_string()));
    }
    Ok(())
}

fn enforce_entry_size(path: &str, size: u64, maximum: u64) -> RtwResult<()> {
    if size > maximum {
        return Err(RtwError::EntryTooLarge {
            path: path.to_string(),
            actual: size,
            maximum,
        });
    }
    Ok(())
}

fn validate_source_manifest(source: &Path, files: &[SourceFile]) -> RtwResult<()> {
    let manifest_file = files
        .iter()
        .find(|file| file.artifact_path.as_str() == RTW_MANIFEST_PATH)
        .ok_or(RtwError::MissingManifest)?;
    let source_text = fs::read_to_string(&manifest_file.filesystem_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            RtwError::InvalidManifestEncoding
        } else {
            RtwError::Io(error)
        }
    })?;
    let manifest: RtwManifest = toml::from_str(&source_text)?;
    manifest.validate()?;
    if !files
        .iter()
        .any(|file| file.artifact_path == manifest.entry)
    {
        return Err(RtwError::MissingContentEntry(manifest.entry.to_string()));
    }
    if !manifest_file.filesystem_path.starts_with(source) {
        return Err(RtwError::UnsupportedSourceEntry(
            manifest_file.filesystem_path.display().to_string(),
        ));
    }
    Ok(())
}
fn write_archive(output: &Path, files: &[SourceFile], limits: RtwLimits) -> RtwResult<()> {
    let parent = output.parent().ok_or_else(|| RtwError::InvalidPath {
        path: output.display().to_string(),
        reason: "output path must have a parent directory",
    })?;
    let temporary = NamedTempFile::new_in(parent)?;
    let writer_file = temporary.reopen()?;
    let mut writer = ZipWriter::new(writer_file);
    let options = archive_options();

    if let Some(manifest) = files
        .iter()
        .find(|file| file.artifact_path.as_str() == RTW_MANIFEST_PATH)
    {
        write_source_file(&mut writer, manifest, options)?;
    }
    for file in files {
        if file.artifact_path.as_str() == RTW_MANIFEST_PATH {
            continue;
        }
        write_source_file(&mut writer, file, options)?;
    }

    let written_file = writer.finish()?;
    written_file.sync_all()?;
    drop(written_file);
    let _validated = RtwArchive::open(temporary.path(), limits)?;
    drop(_validated);

    temporary.persist_noclobber(output).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            RtwError::OutputAlreadyExists(output.display().to_string())
        } else {
            RtwError::Io(error.error)
        }
    })?;
    Ok(())
}

fn archive_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::DEFAULT)
        .unix_permissions(0o644)
}

fn write_source_file(
    writer: &mut ZipWriter<File>,
    source: &SourceFile,
    options: SimpleFileOptions,
) -> RtwResult<()> {
    writer.start_file(source.artifact_path.as_str(), options)?;
    let mut input = File::open(&source.filesystem_path)?;
    std::io::copy(&mut input, writer)?;
    Ok(())
}
