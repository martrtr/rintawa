//! Content-addressed storage for validated RTW artifacts.

use std::{
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{ArtifactDigest, RtwArchive, RtwError, RtwLimits, RtwResult};

const SHA256_DIRECTORY: &str = "sha256";
const TEMPORARY_DIRECTORY: &str = "tmp";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Result category for one content-addressed import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportDisposition {
    /// New bytes were published under their digest.
    Imported,
    /// Identical bytes were already present and verified.
    AlreadyPresent,
}

/// Result of importing one validated RTW artifact into the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactImport {
    digest: ArtifactDigest,
    disposition: ImportDisposition,
}
impl ArtifactImport {
    /// Returns the SHA-256 identity of the imported artifact bytes.
    pub fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    /// Returns whether the import published new bytes or reused an existing object.
    pub const fn disposition(&self) -> ImportDisposition {
        self.disposition
    }
}

/// Immutable content-addressed store for validated RTW artifacts.
///
/// The supplied root is expected to be the Rintawa store directory itself.
/// SHA-256 artifacts are stored as `sha256/<digest>.rtw` below that root.
pub struct ArtifactStore {
    root: PathBuf,
    sha256_directory: PathBuf,
    temporary_directory: PathBuf,
    limits: RtwLimits,
}

impl ArtifactStore {
    /// Opens or creates an RTW artifact store rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or store-integrity error if the root cannot be created,
    /// canonicalized, or a reserved store subdirectory is not a real directory.
    pub fn open(root: impl AsRef<Path>, limits: RtwLimits) -> RtwResult<Self> {
        fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        let sha256_directory = root.join(SHA256_DIRECTORY);
        let temporary_directory = root.join(TEMPORARY_DIRECTORY);
        ensure_store_directory(&sha256_directory)?;
        ensure_store_directory(&temporary_directory)?;
        sync_directory(&root)?;
        Ok(Self {
            root,
            sha256_directory,
            temporary_directory,
            limits,
        })
    }
    /// Returns the canonical filesystem root of this store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Imports one RTW file after copying, hashing, and validating the copied bytes.
    ///
    /// The source path is never retained as artifact identity. Publication uses an
    /// atomic same-filesystem hard-link creation, so concurrent equal imports
    /// converge on one immutable digest entry without overwriting it.
    ///
    /// # Errors
    ///
    /// Returns an RTW validation error, a host-limit error, an I/O error, or
    /// [`RtwError::StoreCorruption`] if an existing digest path contains other bytes.
    pub fn import(&self, source: impl AsRef<Path>) -> RtwResult<ArtifactImport> {
        let mut source_file = File::open(source.as_ref())?;
        if !source_file.metadata()?.is_file() {
            return Err(RtwError::UnsupportedSourceEntry(
                source.as_ref().display().to_string(),
            ));
        }

        let mut temporary = NamedTempFile::new_in(&self.temporary_directory)?;
        let digest = copy_and_hash(
            &mut source_file,
            temporary.as_file_mut(),
            self.limits.max_archive_bytes,
        )?;
        temporary.as_file_mut().sync_all()?;

        let validated = RtwArchive::open(temporary.path(), self.limits)?;
        drop(validated);

        let destination = self.path_for(&digest);
        let disposition = self.publish_temporary(&temporary, &destination, &digest)?;
        File::open(&destination)?.sync_all()?;
        sync_directory(&self.sha256_directory)?;

        Ok(ArtifactImport {
            digest,
            disposition,
        })
    }

    /// Imports in-memory RTW bytes through the same validation and CAS publication path.
    ///
    /// # Errors
    ///
    /// Returns an RTW validation, size-limit, I/O, or store-integrity error.
    pub fn import_bytes(&self, bytes: &[u8]) -> RtwResult<ArtifactImport> {
        let mut source = Cursor::new(bytes);
        let mut temporary = NamedTempFile::new_in(&self.temporary_directory)?;
        let digest = copy_and_hash(
            &mut source,
            temporary.as_file_mut(),
            self.limits.max_archive_bytes,
        )?;
        temporary.as_file_mut().sync_all()?;

        let validated = RtwArchive::open(temporary.path(), self.limits)?;
        drop(validated);

        let destination = self.path_for(&digest);
        let disposition = self.publish_temporary(&temporary, &destination, &digest)?;
        File::open(&destination)?.sync_all()?;
        sync_directory(&self.sha256_directory)?;

        Ok(ArtifactImport {
            digest,
            disposition,
        })
    }

    fn publish_temporary(
        &self,
        temporary: &NamedTempFile,
        destination: &Path,
        digest: &ArtifactDigest,
    ) -> RtwResult<ImportDisposition> {
        match fs::hard_link(temporary.path(), destination) {
            Ok(()) => Ok(ImportDisposition::Imported),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.open_artifact(digest)?;
                Ok(ImportDisposition::AlreadyPresent)
            }
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                self.publish_cross_device(temporary, destination, digest)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn publish_cross_device(
        &self,
        temporary: &NamedTempFile,
        destination: &Path,
        digest: &ArtifactDigest,
    ) -> RtwResult<ImportDisposition> {
        let mut publication = NamedTempFile::new_in(&self.sha256_directory)?;
        let mut source = temporary.as_file().try_clone()?;
        source.seek(SeekFrom::Start(0))?;
        let copied_digest = copy_and_hash(
            &mut source,
            publication.as_file_mut(),
            self.limits.max_archive_bytes,
        )?;
        if &copied_digest != digest {
            return Err(RtwError::StoreCorruption(digest.to_string()));
        }
        publication.as_file_mut().sync_all()?;

        match fs::hard_link(publication.path(), destination) {
            Ok(()) => Ok(ImportDisposition::Imported),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.open_artifact(digest)?;
                Ok(ImportDisposition::AlreadyPresent)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Opens a stored artifact after verifying its content address and RTW structure.
    ///
    /// Hashing and RTW validation use the same open file handle, so replacing the
    /// digest pathname between those checks cannot substitute different bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::StoredArtifactNotFound`] if the digest is absent,
    /// [`RtwError::StoreCorruption`] if its bytes no longer match, or an RTW
    /// validation error if the stored container is invalid.
    pub fn open_artifact(&self, digest: &ArtifactDigest) -> RtwResult<RtwArchive> {
        let path = self.path_for(digest);
        ensure_regular_store_file(&path, digest)?;
        let mut file = File::open(&path)?;
        let actual = hash_open_file(&mut file, self.limits.max_archive_bytes)?;
        if &actual != digest {
            return Err(RtwError::StoreCorruption(digest.to_string()));
        }
        file.seek(SeekFrom::Start(0))?;
        RtwArchive::from_file(file, self.limits)
    }

    /// Verifies both the content address and RTW validity of a stored artifact.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::open_artifact`].
    pub fn verify(&self, digest: &ArtifactDigest) -> RtwResult<()> {
        let archive = self.open_artifact(digest)?;
        drop(archive);
        Ok(())
    }

    fn path_for(&self, digest: &ArtifactDigest) -> PathBuf {
        self.sha256_directory.join(format!("{}.rtw", digest.hex()))
    }
}
fn ensure_store_directory(path: &Path) -> RtwResult<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RtwError::InvalidStoreEntry {
                    path: path.display().to_string(),
                    reason: "reserved store algorithm path must be a real directory",
                });
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_regular_store_file(path: &Path, digest: &ArtifactDigest) -> RtwResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RtwError::StoredArtifactNotFound(digest.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RtwError::InvalidStoreEntry {
            path: path.display().to_string(),
            reason: "digest path must be a regular file",
        });
    }
    Ok(())
}
fn copy_and_hash(
    source: &mut impl Read,
    destination: &mut File,
    maximum_bytes: u64,
) -> RtwResult<ArtifactDigest> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut total = 0_u64;

    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > maximum_bytes {
            return Err(RtwError::ArchiveTooLarge {
                actual: total,
                maximum: maximum_bytes,
            });
        }
        destination.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }

    let digest: [u8; 32] = hasher.finalize().into();
    Ok(ArtifactDigest::from_sha256_bytes(digest))
}

fn hash_open_file(file: &mut File, maximum_bytes: u64) -> RtwResult<ArtifactDigest> {
    let declared = file.metadata()?.len();
    if declared > maximum_bytes {
        return Err(RtwError::ArchiveTooLarge {
            actual: declared,
            maximum: maximum_bytes,
        });
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > maximum_bytes {
            return Err(RtwError::ArchiveTooLarge {
                actual: total,
                maximum: maximum_bytes,
            });
        }
        hasher.update(&buffer[..read]);
    }

    let digest: [u8; 32] = hasher.finalize().into();
    Ok(ArtifactDigest::from_sha256_bytes(digest))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> RtwResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> RtwResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;

    #[test]
    fn test_should_publish_through_cross_device_fallback() -> RtwResult<()> {
        let root = TempDir::new()?;
        let source = root.path().join("source.rtw");
        let file = File::create(&source)?;
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("rtw.toml", options)?;
        writer.write_all(
            b"format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
        )?;
        writer.start_file("manifest.toml", options)?;
        writer.write_all(b"id = \"example\"\n")?;
        writer.finish()?;

        let bytes = fs::read(&source)?;
        let digest = ArtifactDigest::sha256(&bytes);
        let store = ArtifactStore::open(root.path().join("store"), RtwLimits::default())?;
        let staging = NamedTempFile::new_in(root.path())?;
        fs::write(staging.path(), &bytes)?;
        let destination = store.path_for(&digest);

        assert_eq!(
            store.publish_cross_device(&staging, &destination, &digest)?,
            ImportDisposition::Imported
        );
        assert_eq!(
            store.publish_cross_device(&staging, &destination, &digest)?,
            ImportDisposition::AlreadyPresent
        );
        store.verify(&digest)?;
        Ok(())
    }
}
