//! Deterministic RTW ZIP packing.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use crate::{
    ArtifactPath, RTW_MANIFEST_PATH, RtwArchive, RtwError, RtwLimits, RtwManifest, RtwResult,
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
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::DEFAULT)
        .unix_permissions(0o644);

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
