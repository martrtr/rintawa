//! RTW container errors.

use std::io;

use thiserror::Error;

/// Errors produced while parsing, validating, reading, or packing RTW artifacts.
#[derive(Debug, Error)]
pub enum RtwError {
    /// A filesystem or stream operation failed.
    #[error("artifact I/O failed: {0}")]
    Io(#[from] io::Error),
    /// ZIP container parsing or decoding failed.
    #[error("invalid ZIP container: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// The root RTW manifest is not valid TOML.
    #[error("invalid rtw.toml: {0}")]
    ManifestParse(#[from] toml::de::Error),
    /// The artifact uses an unsupported RTW container format version.
    #[error("unsupported RTW format {found}; this runtime supports format {supported}")]
    UnsupportedFormat {
        /// Version declared by the artifact.
        found: u32,
        /// Version supported by this runtime.
        supported: u32,
    },
    /// A content type identifier is malformed.
    #[error("invalid RTW content type `{value}`: {reason}")]
    InvalidContentType {
        /// Rejected content type string.
        value: String,
        /// Human-readable validation reason.
        reason: &'static str,
    },
    /// A path inside an RTW artifact is unsafe or non-canonical.
    #[error("invalid artifact path `{path}`: {reason}")]
    InvalidPath {
        /// Rejected path.
        path: String,
        /// Human-readable validation reason.
        reason: &'static str,
    },
    /// The physical RTW file exceeds the configured host limit.
    #[error("RTW archive is {actual} bytes, exceeding the {maximum}-byte limit")]
    ArchiveTooLarge {
        /// Observed archive size.
        actual: u64,
        /// Maximum accepted archive size.
        maximum: u64,
    },
    /// The ZIP central directory contains too many entries.
    #[error("RTW archive contains {actual} entries, exceeding the {maximum}-entry limit")]
    TooManyEntries {
        /// Number of archive entries.
        actual: usize,
        /// Maximum accepted number of entries.
        maximum: usize,
    },
    /// An entry exceeds the configured uncompressed per-entry limit.
    #[error("artifact entry `{path}` is {actual} bytes, exceeding the {maximum}-byte limit")]
    EntryTooLarge {
        /// Entry path.
        path: String,
        /// Uncompressed size observed or declared for the entry.
        actual: u64,
        /// Maximum accepted entry size.
        maximum: u64,
    },
    /// The sum of uncompressed entry sizes exceeds the configured host limit.
    #[error("RTW archive expands to at least {actual} bytes, exceeding the {maximum}-byte limit")]
    UncompressedArchiveTooLarge {
        /// Uncompressed size observed so far.
        actual: u64,
        /// Maximum accepted total uncompressed size.
        maximum: u64,
    },
    /// RTW does not allow encrypted ZIP entries.
    #[error("encrypted ZIP entry `{0}` is not allowed in RTW artifacts")]
    EncryptedEntry(String),
    /// RTW v1 supports only stored and deflated ZIP entries.
    #[error("unsupported ZIP compression method for entry `{0}`")]
    UnsupportedCompression(String),
    /// Symlinks and other special filesystem entries are not valid RTW content.
    #[error("unsupported filesystem entry type at `{0}`")]
    UnsupportedEntryType(String),
    /// The archive contains the same normalized path more than once.
    #[error("duplicate artifact entry `{0}`")]
    DuplicateEntry(String),
    /// A file path conflicts with a directory prefix in the same archive.
    #[error("artifact file/directory path conflict at `{0}`")]
    PathConflict(String),
    /// The root `rtw.toml` file is missing.
    #[error("RTW archive does not contain root rtw.toml")]
    MissingManifest,
    /// The root `rtw.toml` exceeds the configured manifest size limit.
    #[error("rtw.toml is {actual} bytes, exceeding the {maximum}-byte limit")]
    ManifestTooLarge {
        /// Manifest size observed or declared.
        actual: u64,
        /// Maximum accepted manifest size.
        maximum: u64,
    },
    /// The root RTW manifest is not valid UTF-8.
    #[error("rtw.toml must be valid UTF-8")]
    InvalidManifestEncoding,
    /// A requested artifact file does not exist.
    #[error("artifact entry `{0}` does not exist as a regular file")]
    EntryNotFound(String),
    /// The manifest points to an entry that is not present as a regular file.
    #[error("RTW content entry `{0}` does not exist as a regular file")]
    MissingContentEntry(String),
    /// A source directory passed to the packer is not a directory.
    #[error("RTW pack source `{0}` is not a directory")]
    SourceNotDirectory(String),
    /// The source tree contains a symlink or another unsupported filesystem object.
    #[error("unsupported source filesystem entry `{0}`")]
    UnsupportedSourceEntry(String),
    /// The pack destination already exists.
    #[error("RTW pack output `{0}` already exists")]
    OutputAlreadyExists(String),
    /// The pack output would be located inside the source tree.
    #[error("RTW pack output `{0}` must not be inside its source directory")]
    OutputInsideSource(String),
}

/// Result type used by RTW artifact operations.
pub type RtwResult<T> = Result<T, RtwError>;
