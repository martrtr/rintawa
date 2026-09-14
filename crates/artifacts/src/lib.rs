//! RTW artifact container primitives.
//!
//! An RTW artifact is a ZIP container with one root content type described by
//! a small `rtw.toml` manifest. This crate intentionally does not implement
//! repositories, dependency resolution, updates, or runtime activation.

mod archive;
mod error;
mod manifest;
mod pack;
mod path;

pub use archive::{ArtifactEntry, RtwArchive, RtwLimits};
pub use error::{RtwError, RtwResult};
pub use manifest::{ContentType, RTW_FORMAT_VERSION, RTW_MANIFEST_PATH, RtwManifest};
pub use pack::pack_directory;
pub use path::ArtifactPath;
