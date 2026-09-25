//! RTW artifact container primitives.
//!
//! An RTW artifact is a ZIP container with one root content type described by
//! a small `rtw.toml` manifest. This crate intentionally does not implement
//! repositories, dependency resolution, updates, or runtime activation.

mod archive;
mod asset;
mod digest;
mod error;
mod manifest;
mod pack;
mod path;
mod store;

pub use archive::{ArtifactEntry, RtwArchive, RtwLimits};
pub use asset::{
    AssetDigest, AssetError, AssetImport, AssetMediaType, AssetRef, AssetResult, AssetStore,
};
pub use digest::ArtifactDigest;
pub use error::{RtwError, RtwResult};
pub use manifest::{ContentType, RTW_FORMAT_VERSION, RTW_MANIFEST_PATH, RtwManifest};
pub use pack::{RtwPackEntry, pack_directory, pack_entries};
pub use path::ArtifactPath;
pub use store::{ArtifactImport, ArtifactStore, ImportDisposition};
