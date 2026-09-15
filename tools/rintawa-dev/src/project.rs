use std::{
    fs,
    path::{Component as PathComponent, Path, PathBuf},
    process::Command,
};

use rintawa_artifacts::{ArtifactDigest, ArtifactStore, RtwLimits, pack_directory};
use tempfile::TempDir;

use crate::{DEV_CONFIG_FILE, DevConfig, DevError, DevResult};

const DEV_CONFIG_SCHEMA: u32 = 1;

/// Local extension project.
#[derive(Debug, Clone)]
pub struct DevProject {
    root: PathBuf,
    config: Option<DevConfig>,
}

/// Temporary RTW snapshot stored in a content-addressed store.
pub struct PreparedSnapshot {
    _workspace: TempDir,
    store: ArtifactStore,
    digest: ArtifactDigest,
}

impl DevProject {
    /// Opens a project directory.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid directory or configuration.
    pub fn open(path: impl AsRef<Path>) -> DevResult<Self> {
        let root = path.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(DevError::InvalidArtifactRoot(root.display().to_string()));
        }

        let config_path = root.join(DEV_CONFIG_FILE);
        let config = if config_path.exists() {
            let raw = fs::read_to_string(config_path)?;
            let config: DevConfig = toml::from_str(&raw)?;
            validate_config(&config)?;
            Some(config)
        } else {
            None
        };

        Ok(Self { root, config })
    }

    /// Returns the project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the developer configuration, if present.
    pub fn config(&self) -> Option<&DevConfig> {
        self.config.as_ref()
    }

    /// Builds and packs a temporary RTW snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the build, path validation, or RTW import fails.
    pub fn prepare_snapshot(&self) -> DevResult<PreparedSnapshot> {
        self.run_build()?;
        let artifact_root = self.resolve_artifact_root()?;
        let workspace = tempfile::tempdir()?;
        let artifact_path = workspace.path().join("dev-snapshot.rtw");
        pack_directory(&artifact_root, &artifact_path, RtwLimits::default())?;

        let store = ArtifactStore::open(workspace.path().join("store"), RtwLimits::default())?;
        let digest = store.import(&artifact_path)?.digest().clone();
        Ok(PreparedSnapshot {
            _workspace: workspace,
            store,
            digest,
        })
    }

    fn run_build(&self) -> DevResult<()> {
        let Some(command) = self
            .config
            .as_ref()
            .and_then(|config| config.build.as_ref())
        else {
            return Ok(());
        };
        let (program, arguments) = command.split_first().ok_or(DevError::EmptyBuildCommand)?;
        let status = Command::new(program)
            .args(arguments)
            .current_dir(&self.root)
            .status()?;
        if !status.success() {
            return Err(DevError::BuildFailed(status.to_string()));
        }
        Ok(())
    }

    fn resolve_artifact_root(&self) -> DevResult<PathBuf> {
        let relative = self
            .config
            .as_ref()
            .map(|config| config.artifact_root.as_path())
            .unwrap_or_else(|| Path::new("."));
        validate_relative_path(relative)?;

        let resolved = self.root.join(relative).canonicalize()?;
        if !resolved.starts_with(&self.root) || !resolved.is_dir() {
            return Err(DevError::InvalidArtifactRoot(
                relative.display().to_string(),
            ));
        }
        Ok(resolved)
    }
}

impl PreparedSnapshot {
    /// Returns the snapshot digest.
    pub fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    /// Returns the temporary artifact store.
    pub fn store(&self) -> &ArtifactStore {
        &self.store
    }
}

fn validate_config(config: &DevConfig) -> DevResult<()> {
    if config.schema != DEV_CONFIG_SCHEMA {
        return Err(DevError::UnsupportedConfigSchema(config.schema));
    }
    validate_relative_path(&config.artifact_root)?;
    if let Some(build) = &config.build
        && (build.is_empty() || build[0].trim().is_empty())
    {
        return Err(DevError::EmptyBuildCommand);
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> DevResult<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(DevError::InvalidArtifactRoot(path.display().to_string()));
    }
    for component in path.components() {
        match component {
            PathComponent::CurDir | PathComponent::Normal(_) => {}
            PathComponent::ParentDir | PathComponent::RootDir | PathComponent::Prefix(_) => {
                return Err(DevError::InvalidArtifactRoot(path.display().to_string()));
            }
        }
    }
    Ok(())
}
