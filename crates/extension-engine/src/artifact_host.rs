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

    pub(crate) fn fork_owned(&self) -> Result<OwnedRtwComponentSource, RtwError> {
        Ok(OwnedRtwComponentSource {
            archive: self.archive.fork()?,
            extension_manifest_path: self.extension_manifest_path.clone(),
        })
    }
}

/// Owned validated RTW view used by bounded guest artifact resources.
pub struct OwnedRtwComponentSource {
    archive: RtwArchive,
    extension_manifest_path: ArtifactPath,
}

impl OwnedRtwComponentSource {
    pub(crate) fn resolve_component_entry(&self, entry: &str) -> Result<ArtifactPath, RtwError> {
        resolve_relative_entry(&self.extension_manifest_path, entry)
    }

    pub(crate) fn resolve_relative_to(
        &self,
        base_file: &ArtifactPath,
        entry: &str,
    ) -> Result<ArtifactPath, RtwError> {
        resolve_relative_entry(base_file, entry)
    }

    pub(crate) fn path_strings_with_limit(&self, maximum_bytes: usize) -> Option<Vec<String>> {
        collect_path_strings_with_limit(
            self.archive.entries().map(|entry| entry.path.as_str()),
            maximum_bytes,
        )
    }

    pub(crate) fn read_with_limit(
        &mut self,
        path: &ArtifactPath,
        maximum_bytes: u64,
    ) -> Result<Vec<u8>, RtwError> {
        self.archive.read_with_limit(path, maximum_bytes)
    }
}

/// Creates runtime components for one versioned RTW execution target.
///
/// Hosts are registered through [`crate::ExtensionEngine::register_execution_target_host`].
/// The engine owns target identity, provider ownership, and lifecycle revocation;
/// [`crate::RtwExtensionLoader`] only resolves the current engine registry.
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

fn collect_path_strings_with_limit<'a>(
    paths: impl Iterator<Item = &'a str>,
    maximum_bytes: usize,
) -> Option<Vec<String>> {
    let mut encoded_bytes = std::mem::size_of::<u32>();
    let mut collected = Vec::new();
    for path in paths {
        encoded_bytes = encoded_bytes
            .checked_add(std::mem::size_of::<u32>())?
            .checked_add(path.len())?;
        if encoded_bytes > maximum_bytes {
            return None;
        }
        collected.push(path.to_string());
    }
    Some(collected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_bound_artifact_path_list_before_collecting_past_limit() {
        let paths = ["a", "bbbb"];
        let encoded_bytes = std::mem::size_of::<u32>()
            + std::mem::size_of::<u32>()
            + paths[0].len()
            + std::mem::size_of::<u32>()
            + paths[1].len();

        assert_eq!(
            collect_path_strings_with_limit(paths.iter().copied(), encoded_bytes),
            Some(vec![String::from("a"), String::from("bbbb")])
        );
        assert_eq!(
            collect_path_strings_with_limit(paths.iter().copied(), encoded_bytes - 1),
            None
        );
    }
}
