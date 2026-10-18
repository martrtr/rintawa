//! Content-addressed storage for immutable non-RTW assets.

use std::{
    fmt,
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{ArtifactDigest, ImportDisposition};

const SHA256_DIRECTORY: &str = "sha256";
const TEMPORARY_DIRECTORY: &str = "tmp";
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_MEDIA_TYPE_BYTES: usize = 127;

/// SHA-256 identity of one immutable asset byte sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetDigest(ArtifactDigest);

impl AssetDigest {
    /// Computes the digest of an in-memory asset.
    pub fn sha256(bytes: &[u8]) -> Self {
        Self(ArtifactDigest::sha256(bytes))
    }

    /// Parses canonical `sha256:<lowercase-hex>` text.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::InvalidDigest`] when the digest is malformed.
    pub fn parse(value: impl AsRef<str>) -> AssetResult<Self> {
        let value = value.as_ref();
        ArtifactDigest::parse(value)
            .map(Self)
            .map_err(|_| AssetError::InvalidDigest(value.to_string()))
    }

    /// Returns the canonical lowercase hexadecimal digest without the algorithm prefix.
    pub fn hex(&self) -> String {
        self.0.hex()
    }
}

impl fmt::Display for AssetDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for AssetDigest {
    type Err = AssetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for AssetDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AssetDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Canonical MIME-style media type attached to an asset reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetMediaType(String);

impl AssetMediaType {
    /// Parses a bounded `type/subtype` media type without parameters.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::InvalidMediaType`] if the value is empty, too long,
    /// contains parameters, or uses characters outside the media-type token set.
    pub fn parse(value: impl Into<String>) -> AssetResult<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_MEDIA_TYPE_BYTES {
            return Err(invalid_media_type(
                &value,
                "media type must contain 1..=127 bytes",
            ));
        }
        let Some((kind, subtype)) = value.split_once('/') else {
            return Err(invalid_media_type(&value, "expected `type/subtype`"));
        };
        if kind.is_empty() || subtype.is_empty() || subtype.contains('/') {
            return Err(invalid_media_type(
                &value,
                "expected exactly one `/` separator",
            ));
        }
        if !kind.bytes().all(is_media_type_token) || !subtype.bytes().all(is_media_type_token) {
            return Err(invalid_media_type(
                &value,
                "type and subtype must use MIME token characters",
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// Returns the canonical media type string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetMediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for AssetMediaType {
    type Err = AssetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for AssetMediaType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AssetMediaType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Stable reference to one immutable asset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRef {
    /// SHA-256 identity of the exact asset bytes.
    pub digest: AssetDigest,
    /// Exact byte length of the asset.
    pub size: u64,
    /// Media type used by content handlers and renderers.
    pub media_type: AssetMediaType,
}

impl AssetRef {
    /// Creates a validated asset reference.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::InvalidMediaType`] when `media_type` is invalid.
    pub fn new(digest: AssetDigest, size: u64, media_type: impl Into<String>) -> AssetResult<Self> {
        Ok(Self {
            digest,
            size,
            media_type: AssetMediaType::parse(media_type)?,
        })
    }
}

/// Result of importing one immutable asset into the content-addressed store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetImport {
    reference: AssetRef,
    disposition: ImportDisposition,
}

impl AssetImport {
    /// Returns the immutable reference to the imported bytes.
    pub fn asset_ref(&self) -> &AssetRef {
        &self.reference
    }

    /// Returns whether this import published bytes or reused an existing object.
    pub const fn disposition(&self) -> ImportDisposition {
        self.disposition
    }
}

/// Immutable content-addressed store for raw assets referenced by RTW content.
pub struct AssetStore {
    root: PathBuf,
    sha256_directory: PathBuf,
    temporary_directory: PathBuf,
    maximum_asset_bytes: u64,
}

impl AssetStore {
    /// Opens or creates an asset store rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns an I/O or integrity error when reserved store paths cannot be created
    /// as real directories.
    pub fn open(root: impl AsRef<Path>, maximum_asset_bytes: u64) -> AssetResult<Self> {
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
            maximum_asset_bytes,
        })
    }

    /// Returns the canonical filesystem root of this asset store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Imports one file into the immutable asset store.
    ///
    /// # Errors
    ///
    /// Returns a media-type validation, size-limit, I/O, or store-integrity error.
    pub fn import(
        &self,
        source: impl AsRef<Path>,
        media_type: impl Into<String>,
    ) -> AssetResult<AssetImport> {
        let media_type = AssetMediaType::parse(media_type)?;
        let mut source_file = File::open(source.as_ref())?;
        if !source_file.metadata()?.is_file() {
            return Err(AssetError::UnsupportedSource(
                source.as_ref().display().to_string(),
            ));
        }
        self.import_reader(&mut source_file, media_type)
    }

    /// Imports in-memory bytes into the immutable asset store.
    ///
    /// # Errors
    ///
    /// Returns a media-type validation, size-limit, I/O, or store-integrity error.
    pub fn import_bytes(
        &self,
        bytes: &[u8],
        media_type: impl Into<String>,
    ) -> AssetResult<AssetImport> {
        let media_type = AssetMediaType::parse(media_type)?;
        let mut source = Cursor::new(bytes);
        self.import_reader(&mut source, media_type)
    }

    fn import_reader(
        &self,
        source: &mut impl Read,
        media_type: AssetMediaType,
    ) -> AssetResult<AssetImport> {
        let mut temporary = NamedTempFile::new_in(&self.temporary_directory)?;
        let (digest, size) =
            copy_and_hash(source, temporary.as_file_mut(), self.maximum_asset_bytes)?;
        temporary.as_file_mut().sync_all()?;
        let reference = AssetRef {
            digest,
            size,
            media_type,
        };
        let destination = self.path_for(&reference.digest);
        let disposition = match fs::hard_link(temporary.path(), &destination) {
            Ok(()) => ImportDisposition::Imported,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = self.open_asset(&reference)?;
                drop(existing);
                ImportDisposition::AlreadyPresent
            }
            Err(error) => return Err(error.into()),
        };
        sync_published_file(&destination)?;
        sync_directory(&self.sha256_directory)?;
        Ok(AssetImport {
            reference,
            disposition,
        })
    }

    /// Opens an asset after verifying digest, exact size, and store entry type.
    ///
    /// The returned file is rewound to byte zero after verification.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::StoredAssetNotFound`] if absent, a size/integrity error
    /// if stored bytes no longer match the reference, or an I/O error.
    pub fn open_asset(&self, reference: &AssetRef) -> AssetResult<File> {
        let path = self.path_for(&reference.digest);
        ensure_regular_store_file(&path, &reference.digest)?;
        let mut file = File::open(&path)?;
        let (actual_digest, actual_size) = hash_open_file(&mut file, self.maximum_asset_bytes)?;
        if actual_digest != reference.digest {
            return Err(AssetError::StoreCorruption(reference.digest.to_string()));
        }
        if actual_size != reference.size {
            return Err(AssetError::SizeMismatch {
                digest: reference.digest.to_string(),
                expected: reference.size,
                actual: actual_size,
            });
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    /// Verifies one stored asset reference without retaining an open handle.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::open_asset`].
    pub fn verify(&self, reference: &AssetRef) -> AssetResult<()> {
        let file = self.open_asset(reference)?;
        drop(file);
        Ok(())
    }

    fn path_for(&self, digest: &AssetDigest) -> PathBuf {
        self.sha256_directory.join(digest.hex())
    }
}

/// Errors produced by immutable asset references and storage.
#[derive(Debug, Error)]
pub enum AssetError {
    /// A filesystem or stream operation failed.
    #[error("asset I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A digest string is malformed.
    #[error("invalid asset digest `{0}`")]
    InvalidDigest(String),
    /// A media type is malformed or non-canonical.
    #[error("invalid asset media type `{value}`: {reason}")]
    InvalidMediaType {
        /// Rejected media type.
        value: String,
        /// Validation rule that was violated.
        reason: &'static str,
    },
    /// A source path is not a regular file.
    #[error("asset source `{0}` is not a regular file")]
    UnsupportedSource(String),
    /// Imported or stored bytes exceed the configured host limit.
    #[error("asset is {actual} bytes, exceeding the {maximum}-byte limit")]
    AssetTooLarge {
        /// Observed byte length.
        actual: u64,
        /// Maximum accepted byte length.
        maximum: u64,
    },
    /// A requested asset is not present in the store.
    #[error("asset `{0}` is not present in the asset store")]
    StoredAssetNotFound(String),
    /// Bytes stored under a digest path no longer match that digest.
    #[error("stored asset `{0}` failed content-address verification")]
    StoreCorruption(String),
    /// Stored bytes do not match the exact byte length in an [`AssetRef`].
    #[error("stored asset `{digest}` is {actual} bytes; reference requires {expected}")]
    SizeMismatch {
        /// Referenced digest.
        digest: String,
        /// Size recorded by the reference.
        expected: u64,
        /// Size found in the store.
        actual: u64,
    },
    /// A reserved store path has an unsafe filesystem type.
    #[error("invalid asset store entry `{path}`: {reason}")]
    InvalidStoreEntry {
        /// Rejected path.
        path: String,
        /// Required filesystem invariant.
        reason: &'static str,
    },
}

/// Result type used by asset operations.
pub type AssetResult<T> = Result<T, AssetError>;

fn invalid_media_type(value: &str, reason: &'static str) -> AssetError {
    AssetError::InvalidMediaType {
        value: value.to_string(),
        reason,
    }
}

fn is_media_type_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn ensure_store_directory(path: &Path) -> AssetResult<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AssetError::InvalidStoreEntry {
                    path: path.display().to_string(),
                    reason: "reserved asset-store path must be a real directory",
                });
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_regular_store_file(path: &Path, digest: &AssetDigest) -> AssetResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AssetError::StoredAssetNotFound(digest.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AssetError::InvalidStoreEntry {
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
) -> AssetResult<(AssetDigest, u64)> {
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
            return Err(AssetError::AssetTooLarge {
                actual: total,
                maximum: maximum_bytes,
            });
        }
        destination.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((
        AssetDigest(ArtifactDigest::from_sha256_bytes(digest)),
        total,
    ))
}

fn hash_open_file(file: &mut File, maximum_bytes: u64) -> AssetResult<(AssetDigest, u64)> {
    let declared = file.metadata()?.len();
    if declared > maximum_bytes {
        return Err(AssetError::AssetTooLarge {
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
            return Err(AssetError::AssetTooLarge {
                actual: total,
                maximum: maximum_bytes,
            });
        }
        hasher.update(&buffer[..read]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((
        AssetDigest(ArtifactDigest::from_sha256_bytes(digest)),
        total,
    ))
}

#[cfg(windows)]
fn sync_published_file(path: &Path) -> AssetResult<()> {
    File::options().write(true).open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(windows))]
fn sync_published_file(path: &Path) -> AssetResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> AssetResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> AssetResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_should_round_trip_asset_reference_json() -> AssetResult<()> {
        let reference = AssetRef::new(AssetDigest::sha256(b"image"), 5, "image/png")?;
        let json = serde_json::to_string(&reference).expect("serialize asset ref");
        let decoded: AssetRef = serde_json::from_str(&json).expect("deserialize asset ref");
        assert_eq!(decoded, reference);
        assert_eq!(
            AssetMediaType::parse("Image/VND.Example+JSON")?.as_str(),
            "image/vnd.example+json"
        );
        assert!(AssetMediaType::parse("image/png; charset=utf-8").is_err());
        Ok(())
    }

    #[test]
    fn test_should_import_deduplicate_and_verify_asset_bytes() -> AssetResult<()> {
        let root = TempDir::new()?;
        let store = AssetStore::open(root.path().join("assets"), 1024)?;
        let first = store.import_bytes(b"immutable-image", "image/png")?;
        let second = store.import_bytes(b"immutable-image", "image/png")?;

        assert_eq!(first.disposition(), ImportDisposition::Imported);
        assert_eq!(second.disposition(), ImportDisposition::AlreadyPresent);
        assert_eq!(first.asset_ref(), second.asset_ref());

        let mut file = store.open_asset(first.asset_ref())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"immutable-image");
        Ok(())
    }

    #[test]
    fn test_should_fail_closed_for_wrong_size_and_tampered_asset() -> AssetResult<()> {
        let root = TempDir::new()?;
        let store = AssetStore::open(root.path().join("assets"), 1024)?;
        let imported = store.import_bytes(b"original", "application/octet-stream")?;
        let mut wrong_size = imported.asset_ref().clone();
        wrong_size.size += 1;
        assert!(matches!(
            store.open_asset(&wrong_size),
            Err(AssetError::SizeMismatch { .. })
        ));

        fs::write(store.path_for(&imported.asset_ref().digest), b"tampered")?;
        assert!(matches!(
            store.verify(imported.asset_ref()),
            Err(AssetError::StoreCorruption(_))
        ));
        Ok(())
    }

    #[test]
    fn test_should_enforce_asset_size_limit() -> AssetResult<()> {
        let root = TempDir::new()?;
        let store = AssetStore::open(root.path().join("assets"), 4)?;
        assert!(matches!(
            store.import_bytes(b"12345", "text/plain"),
            Err(AssetError::AssetTooLarge {
                actual: 5,
                maximum: 4
            })
        ));
        Ok(())
    }
}
