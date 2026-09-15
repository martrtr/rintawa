//! Host boundary for non-WASM components stored in RTW extension artifacts.

use rintawa_artifacts::{ArtifactPath, RtwArchive, RtwError};
use rintawa_sdk::{manifest::ComponentDescriptor, traits::Component};
use thiserror::Error;

/// Failure returned by an RTW component target host while creating a component.
#[derive(Debug, Error)]
pub enum RtwComponentHostError {
    /// Artifact data required by the host is invalid or unavailable.
    #[error(transparent)]
    Artifact(#[from] RtwError),

    /// Target-specific component metadata is invalid.
    #[error("invalid target descriptor: {0}")]
    InvalidDescriptor(String),

    /// The host could not create the component.
    #[error("{0}")]
    Host(String),
}

/// Result returned while a target host creates an RTW component.
pub type RtwComponentHostResult<T> = Result<T, RtwComponentHostError>;

/// Validated read-only RTW view supplied to one component target host.
pub struct RtwComponentSource<'a> {
    archive: &'a mut RtwArchive,
    extension_manifest_path: &'a ArtifactPath,
}

impl<'a> RtwComponentSource<'a> {
    pub(crate) fn new(
        archive: &'a mut RtwArchive,
        extension_manifest_path: &'a ArtifactPath,
    ) -> Self {
        Self {
            archive,
            extension_manifest_path,
        }
    }

    /// Returns the extension manifest path selected by the root RTW descriptor.
    pub fn extension_manifest_path(&self) -> &ArtifactPath {
        self.extension_manifest_path
    }

    /// Resolves a component entry relative to the extension manifest directory.
    ///
    /// # Errors
    ///
    /// Returns an artifact path error for traversal, absolute paths, or other
    /// non-canonical input.
    pub fn resolve_component_entry(&self, entry: &str) -> Result<ArtifactPath, RtwError> {
        resolve_relative_entry(self.extension_manifest_path, entry)
    }

    /// Resolves an entry relative to another file inside the artifact.
    ///
    /// # Errors
    ///
    /// Returns an artifact path error for traversal, absolute paths, or other
    /// non-canonical input.
    pub fn resolve_relative_to(
        &self,
        base_file: &ArtifactPath,
        entry: &str,
    ) -> Result<ArtifactPath, RtwError> {
        resolve_relative_entry(base_file, entry)
    }

    /// Returns canonical regular-file paths in the artifact.
    pub fn paths(&self) -> Vec<ArtifactPath> {
        self.archive
            .entries()
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Reads one validated regular file from the artifact.
    ///
    /// # Errors
    ///
    /// Returns an artifact error when the path is absent or bounded reading fails.
    pub fn read(&mut self, path: &ArtifactPath) -> Result<Vec<u8>, RtwError> {
        self.archive.read(path)
    }
}

/// Creates runtime components for one versioned RTW execution target.
pub trait RtwComponentHost: Send + Sync {
    /// Returns the exact component target handled by this host.
    fn target(&self) -> &str;

    /// Creates one component from validated RTW artifact content.
    ///
    /// # Errors
    ///
    /// Returns a target-host error when metadata, resources, or host setup are invalid.
    fn load_component(
        &self,
        source: &mut RtwComponentSource<'_>,
        descriptor: &ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>>;
}

pub(crate) fn resolve_relative_entry(
    base_file: &ArtifactPath,
    entry: &str,
) -> Result<ArtifactPath, RtwError> {
    let entry = ArtifactPath::parse(entry)?;
    match base_file.as_str().rsplit_once('/') {
        Some((parent, _)) => ArtifactPath::parse(format!("{parent}/{}", entry.as_str())),
        None => Ok(entry),
    }
}
