//! Safe RTW ZIP validation and bounded reads.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{Read, Seek},
    path::Path,
};

use zip::{CompressionMethod, ZipArchive};

use crate::{
    ArtifactPath, RTW_MANIFEST_PATH, RtwError, RtwManifest, RtwResult,
    path::{parent_paths, validate_archive_name},
};

/// Host policy limits applied while opening an RTW artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtwLimits {
    /// Maximum physical ZIP size in bytes.
    pub max_archive_bytes: u64,
    /// Maximum number of ZIP entries.
    pub max_entries: usize,
    /// Maximum uncompressed size of one regular file.
    pub max_entry_bytes: u64,
    /// Maximum sum of declared uncompressed entry sizes.
    pub max_uncompressed_bytes: u64,
    /// Maximum uncompressed size of root `rtw.toml`.
    pub max_manifest_bytes: u64,
}

impl Default for RtwLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 1024 * 1024 * 1024,
            max_entries: 100_000,
            max_entry_bytes: 256 * 1024 * 1024,
            max_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024,
        }
    }
}
/// Metadata for one regular file inside an RTW artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEntry {
    /// Canonical path inside the artifact.
    pub path: ArtifactPath,
    /// Compressed size recorded by the ZIP central directory.
    pub compressed_size: u64,
    /// Uncompressed size recorded by the ZIP central directory.
    pub uncompressed_size: u64,
}

/// A validated RTW ZIP archive backed by a file.
pub struct RtwArchive {
    source_file: File,
    archive: ZipArchive<File>,
    manifest: RtwManifest,
    entries: BTreeMap<ArtifactPath, ArtifactEntry>,
    entry_indices: HashMap<ArtifactPath, usize>,
    limits: RtwLimits,
}

impl RtwArchive {
    /// Opens and fully validates RTW container metadata without extracting it.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed ZIP data, unsafe paths, unsupported ZIP
    /// features, host-limit violations, an invalid `rtw.toml`, or a missing
    /// handler entry declared by the manifest.
    pub fn open(path: impl AsRef<Path>, limits: RtwLimits) -> RtwResult<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        Self::from_file(file, limits)
    }

    pub(crate) fn from_file(file: File, limits: RtwLimits) -> RtwResult<Self> {
        let archive_bytes = file.metadata()?.len();
        if archive_bytes > limits.max_archive_bytes {
            return Err(RtwError::ArchiveTooLarge {
                actual: archive_bytes,
                maximum: limits.max_archive_bytes,
            });
        }

        let source_file = file.try_clone()?;
        let mut archive = ZipArchive::new(file)?;
        let (entries, entry_indices) = validate_entries(&mut archive, limits)?;
        let manifest = read_manifest(&mut archive, &entry_indices, limits)?;
        manifest.validate()?;
        if !entries.contains_key(&manifest.entry) {
            return Err(RtwError::MissingContentEntry(manifest.entry.to_string()));
        }

        Ok(Self {
            source_file,
            archive,
            manifest,
            entries,
            entry_indices,
            limits,
        })
    }
    /// Returns the validated root RTW manifest.
    pub fn manifest(&self) -> &RtwManifest {
        &self.manifest
    }
    /// Creates an independently seekable, fully revalidated view of this artifact.
    ///
    /// The fork refers to the same already-open artifact file but owns an independent
    /// file descriptor and ZIP cursor. Validation is repeated so callers never receive
    /// a read view based on stale or partially trusted archive metadata.
    ///
    /// # Errors
    ///
    /// Returns an I/O or RTW validation error if the underlying file cannot be cloned
    /// or no longer satisfies the original archive policy.
    pub fn fork(&self) -> RtwResult<Self> {
        Self::from_file(self.source_file.try_clone()?, self.limits)
    }

    /// Returns metadata for regular files in canonical path order.
    pub fn entries(&self) -> impl Iterator<Item = &ArtifactEntry> {
        self.entries.values()
    }

    /// Returns whether a regular file exists at the given artifact path.
    pub fn contains(&self, path: &ArtifactPath) -> bool {
        self.entries.contains_key(path)
    }

    /// Reads one regular file while enforcing the configured per-entry limit.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::EntryNotFound`] for an unknown path, or another RTW
    /// error if decompression or bounded reading fails.
    pub fn read(&mut self, path: &ArtifactPath) -> RtwResult<Vec<u8>> {
        self.read_with_limit(path, self.limits.max_entry_bytes)
    }

    /// Reads one regular file with an additional caller-provided byte limit.
    ///
    /// The effective limit never exceeds the archive policy configured at open
    /// time. Declared uncompressed size is checked before allocating or
    /// decompressing the entry.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::EntryNotFound`] for an unknown path,
    /// [`RtwError::EntryTooLarge`] when the entry exceeds the effective limit,
    /// or another RTW error if bounded decompression fails.
    pub fn read_with_limit(
        &mut self,
        path: &ArtifactPath,
        maximum_bytes: u64,
    ) -> RtwResult<Vec<u8>> {
        let index = self
            .entry_indices
            .get(path)
            .copied()
            .ok_or_else(|| RtwError::EntryNotFound(path.to_string()))?;
        let entry = self
            .entries
            .get(path)
            .ok_or_else(|| RtwError::EntryNotFound(path.to_string()))?;
        read_zip_entry(
            &mut self.archive,
            index,
            path.as_str(),
            entry.uncompressed_size,
            maximum_bytes.min(self.limits.max_entry_bytes),
        )
    }
}
fn validate_entries(
    archive: &mut ZipArchive<File>,
    limits: RtwLimits,
) -> RtwResult<(
    BTreeMap<ArtifactPath, ArtifactEntry>,
    HashMap<ArtifactPath, usize>,
)> {
    if archive.len() > limits.max_entries {
        return Err(RtwError::TooManyEntries {
            actual: archive.len(),
            maximum: limits.max_entries,
        });
    }

    let mut files = HashSet::new();
    let mut directories = HashSet::new();
    let mut entries = BTreeMap::new();
    let mut entry_indices = HashMap::new();
    let mut total_uncompressed = 0_u64;

    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let raw_name = std::str::from_utf8(file.name_raw()).map_err(|_| RtwError::InvalidPath {
            path: String::from("<non-utf8>"),
            reason: "ZIP entry names must be UTF-8",
        })?;
        if raw_name != file.name() {
            return Err(RtwError::InvalidPath {
                path: file.name().to_string(),
                reason: "ZIP entry name must use canonical UTF-8 encoding",
            });
        }
        let is_directory = file.is_dir();
        let normalized = validate_archive_name(raw_name, is_directory)?;
        validate_zip_entry_type(&file, &normalized, is_directory)?;
        validate_compression(&file, &normalized)?;
        if file.encrypted() {
            return Err(RtwError::EncryptedEntry(normalized));
        }

        total_uncompressed = total_uncompressed.saturating_add(file.size());
        if total_uncompressed > limits.max_uncompressed_bytes {
            return Err(RtwError::UncompressedArchiveTooLarge {
                actual: total_uncompressed,
                maximum: limits.max_uncompressed_bytes,
            });
        }

        if is_directory {
            if files.contains(&normalized) {
                return Err(RtwError::PathConflict(normalized));
            }
            if !directories.insert(normalized.clone()) {
                return Err(RtwError::DuplicateEntry(normalized));
            }
            continue;
        }

        if file.size() > limits.max_entry_bytes {
            return Err(RtwError::EntryTooLarge {
                path: normalized,
                actual: file.size(),
                maximum: limits.max_entry_bytes,
            });
        }
        if directories.contains(&normalized) {
            return Err(RtwError::PathConflict(normalized));
        }
        if !files.insert(normalized.clone()) {
            return Err(RtwError::DuplicateEntry(normalized));
        }

        let path = ArtifactPath::parse(normalized)?;
        entry_indices.insert(path.clone(), index);
        entries.insert(
            path.clone(),
            ArtifactEntry {
                path,
                compressed_size: file.compressed_size(),
                uncompressed_size: file.size(),
            },
        );
    }
    for path in files.iter().chain(directories.iter()) {
        if let Some(parent) = parent_paths(path).find(|parent| files.contains(*parent)) {
            return Err(RtwError::PathConflict(parent.to_string()));
        }
    }

    if !files.contains(RTW_MANIFEST_PATH) {
        return Err(RtwError::MissingManifest);
    }

    Ok((entries, entry_indices))
}

fn validate_compression<R: Read + Seek>(
    file: &zip::read::ZipFile<'_, R>,
    path: &str,
) -> RtwResult<()> {
    match file.compression() {
        CompressionMethod::Stored | CompressionMethod::Deflated => Ok(()),
        _ => Err(RtwError::UnsupportedCompression(path.to_string())),
    }
}

fn validate_zip_entry_type<R: Read + Seek>(
    file: &zip::read::ZipFile<'_, R>,
    path: &str,
    is_directory: bool,
) -> RtwResult<()> {
    if file.is_symlink() {
        return Err(RtwError::UnsupportedEntryType(path.to_string()));
    }
    let Some(mode) = file.unix_mode() else {
        return Ok(());
    };
    let file_type = mode & 0o170000;
    let expected = if is_directory { 0o040000 } else { 0o100000 };
    if file_type != 0 && file_type != expected {
        return Err(RtwError::UnsupportedEntryType(path.to_string()));
    }
    Ok(())
}
fn read_manifest(
    archive: &mut ZipArchive<File>,
    entry_indices: &HashMap<ArtifactPath, usize>,
    limits: RtwLimits,
) -> RtwResult<RtwManifest> {
    let manifest_path = ArtifactPath::parse(RTW_MANIFEST_PATH)?;
    let index = entry_indices
        .get(&manifest_path)
        .copied()
        .ok_or(RtwError::MissingManifest)?;
    let declared_size = archive.by_index(index)?.size();
    if declared_size > limits.max_manifest_bytes {
        return Err(RtwError::ManifestTooLarge {
            actual: declared_size,
            maximum: limits.max_manifest_bytes,
        });
    }
    let bytes = read_zip_entry(
        archive,
        index,
        RTW_MANIFEST_PATH,
        declared_size,
        limits.max_manifest_bytes,
    )?;
    let source = std::str::from_utf8(&bytes).map_err(|_| RtwError::InvalidManifestEncoding)?;
    let manifest: RtwManifest = toml::from_str(source)?;
    Ok(manifest)
}

fn read_zip_entry(
    archive: &mut ZipArchive<File>,
    index: usize,
    path: &str,
    declared_size: u64,
    maximum: u64,
) -> RtwResult<Vec<u8>> {
    if declared_size > maximum {
        return Err(RtwError::EntryTooLarge {
            path: path.to_string(),
            actual: declared_size,
            maximum,
        });
    }
    let file = archive.by_index(index)?;
    let mut output = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut output)?;
    if u64::try_from(output.len()).unwrap_or(u64::MAX) > maximum {
        return Err(RtwError::EntryTooLarge {
            path: path.to_string(),
            actual: u64::try_from(output.len()).unwrap_or(u64::MAX),
            maximum,
        });
    }
    Ok(output)
}
