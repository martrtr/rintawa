//! Local project discovery, source fingerprinting, builds, and RTW snapshot preparation.

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Component as PathComponent, Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rintawa_artifacts::{ArtifactDigest, ArtifactStore, RtwLimits, pack_directory};
use tempfile::TempDir;

use crate::{DEV_CONFIG_FILE, DevConfig, DevError, DevResult, RustComponentBuild};

const DEV_CONFIG_SCHEMA: u32 = 1;
const DEFAULT_WATCH_IGNORE_PATTERNS: [&str; 4] =
    [".git/", ".rintawa-dev/", "target/", "node_modules/"];
const RUST_COMPONENT_TARGET: &str = "wasm32-unknown-unknown";
const MAX_COMPONENTIZE_DIAGNOSTIC_BYTES: usize = 4 * 1024;

/// Local extension project.
#[derive(Debug, Clone)]
pub struct DevProject {
    root: PathBuf,
    config: Option<DevConfig>,
}

/// Opaque source revision used by watch mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRevision(u64);

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
        self.build_rust_components(&artifact_root)?;
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

    /// Computes a source revision for watch mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the project cannot be scanned or ignore patterns are invalid.
    pub fn source_revision(&self) -> DevResult<SourceRevision> {
        let ignore_matcher = self.watch_ignore_matcher()?;
        let generated_root = self.generated_artifact_root();
        let generated_files = self.generated_component_outputs();
        let mut hasher = DefaultHasher::new();
        fingerprint_directory(
            &self.root,
            &self.root,
            &ignore_matcher,
            generated_root.as_deref(),
            &generated_files,
            &mut hasher,
        )?;
        Ok(SourceRevision(hasher.finish()))
    }

    fn watch_ignore_matcher(&self) -> DevResult<Gitignore> {
        let mut builder = GitignoreBuilder::new(&self.root);
        for pattern in DEFAULT_WATCH_IGNORE_PATTERNS {
            add_watch_ignore_pattern(&mut builder, pattern)?;
        }
        if let Some(config) = &self.config {
            for pattern in &config.watch_ignore_patterns {
                add_watch_ignore_pattern(&mut builder, pattern)?;
            }
        }
        builder
            .build()
            .map_err(|error| DevError::InvalidWatchIgnore(error.to_string()))
    }

    fn generated_artifact_root(&self) -> Option<PathBuf> {
        let config = self.config.as_ref()?;
        if config.build.is_none() || config.artifact_root == Path::new(".") {
            return None;
        }
        Some(self.root.join(&config.artifact_root))
    }

    fn generated_component_outputs(&self) -> Vec<PathBuf> {
        let Some(config) = &self.config else {
            return Vec::new();
        };
        config
            .rust_components
            .iter()
            .map(|component| {
                self.root
                    .join(&config.artifact_root)
                    .join(&component.output)
            })
            .collect()
    }

    fn build_rust_components(&self, artifact_root: &Path) -> DevResult<()> {
        let Some(config) = &self.config else {
            return Ok(());
        };
        if config.rust_components.is_empty() {
            return Ok(());
        }

        let target_dir = self.root.join(".rintawa-dev/target");
        for component in &config.rust_components {
            let manifest = self.resolve_component_manifest(component)?;
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = Command::new(cargo)
                .arg("build")
                .arg("--locked")
                .arg("--manifest-path")
                .arg(&manifest)
                .arg("--target")
                .arg(RUST_COMPONENT_TARGET)
                .arg("--target-dir")
                .arg(&target_dir)
                .arg("--lib")
                .current_dir(&self.root)
                .status()?;
            if !status.success() {
                return Err(DevError::RustComponentBuildFailed {
                    manifest: component.manifest_path.display().to_string(),
                    status: status.to_string(),
                });
            }

            let module_path = target_dir
                .join(RUST_COMPONENT_TARGET)
                .join("debug")
                .join(format!("{}.wasm", component.artifact));
            let module = fs::read(&module_path).map_err(|error| {
                DevError::InvalidRustComponentBuild(format!(
                    "Cargo did not produce `{}` for `{}`: {error}",
                    module_path.display(),
                    component.manifest_path.display()
                ))
            })?;
            let mut encoder = wit_component::ComponentEncoder::default()
                .module(&module)
                .map_err(|error| DevError::ComponentizeFailed {
                    artifact: component.artifact.clone(),
                    reason: bounded_componentize_diagnostic(&error.to_string()),
                })?
                .validate(true);
            let encoded = encoder
                .encode()
                .map_err(|error| DevError::ComponentizeFailed {
                    artifact: component.artifact.clone(),
                    reason: bounded_componentize_diagnostic(&error.to_string()),
                })?;
            let output = artifact_root.join(&component.output);
            let parent = output.parent().ok_or_else(|| {
                DevError::InvalidRustComponentBuild(format!(
                    "component output `{}` has no parent",
                    component.output.display()
                ))
            })?;
            fs::create_dir_all(parent)?;
            let parent = parent.canonicalize()?;
            if !parent.starts_with(artifact_root) {
                return Err(DevError::InvalidRustComponentBuild(format!(
                    "component output `{}` escapes the artifact root",
                    component.output.display()
                )));
            }
            let file_name = output.file_name().ok_or_else(|| {
                DevError::InvalidRustComponentBuild(format!(
                    "component output `{}` has no file name",
                    component.output.display()
                ))
            })?;
            let output = parent.join(file_name);
            let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
            temporary.write_all(&encoded)?;
            temporary.as_file_mut().sync_all()?;
            temporary
                .persist(&output)
                .map_err(|error| DevError::Io(error.error))?;
        }
        Ok(())
    }

    fn resolve_component_manifest(&self, component: &RustComponentBuild) -> DevResult<PathBuf> {
        let manifest = self.root.join(&component.manifest_path).canonicalize()?;
        if !manifest.starts_with(&self.root) || !manifest.is_file() {
            return Err(DevError::InvalidRustComponentBuild(format!(
                "manifest `{}` escapes the project or is not a file",
                component.manifest_path.display()
            )));
        }
        Ok(manifest)
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
    validate_watch_ignore_patterns(&config.watch_ignore_patterns)?;
    validate_rust_components(config)?;
    if let Some(build) = &config.build
        && (build.is_empty() || build[0].trim().is_empty())
    {
        return Err(DevError::EmptyBuildCommand);
    }
    Ok(())
}

fn validate_rust_components(config: &DevConfig) -> DevResult<()> {
    let mut outputs = std::collections::HashSet::new();
    for component in &config.rust_components {
        validate_relative_file_path(&component.manifest_path, "manifest-path")?;
        validate_relative_file_path(&component.output, "output")?;
        if !outputs.insert(component.output.clone()) {
            return Err(DevError::InvalidRustComponentBuild(format!(
                "duplicate output `{}`",
                component.output.display()
            )));
        }
        if component.artifact.is_empty()
            || component.artifact.len() > 128
            || !component
                .artifact
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(DevError::InvalidRustComponentBuild(format!(
                "artifact `{}` must use 1..=128 bytes of [a-z0-9_]",
                component.artifact
            )));
        }
    }
    Ok(())
}

fn validate_relative_file_path(path: &Path, field: &str) -> DevResult<()> {
    validate_relative_path(path)?;
    if path == Path::new(".") || path.file_name().is_none() {
        return Err(DevError::InvalidRustComponentBuild(format!(
            "{field} `{}` must identify a relative file",
            path.display()
        )));
    }
    Ok(())
}

fn bounded_componentize_diagnostic(value: &str) -> String {
    if value.len() <= MAX_COMPONENTIZE_DIAGNOSTIC_BYTES {
        return value.to_string();
    }
    let mut end = MAX_COMPONENTIZE_DIAGNOSTIC_BYTES;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn validate_watch_ignore_patterns(patterns: &[String]) -> DevResult<()> {
    let mut builder = GitignoreBuilder::new(".");
    for pattern in patterns {
        add_watch_ignore_pattern(&mut builder, pattern)?;
    }
    builder
        .build()
        .map(|_| ())
        .map_err(|error| DevError::InvalidWatchIgnore(error.to_string()))
}

fn add_watch_ignore_pattern(builder: &mut GitignoreBuilder, pattern: &str) -> DevResult<()> {
    builder
        .add_line(None, pattern)
        .map(|_| ())
        .map_err(|error| DevError::InvalidWatchIgnore(error.to_string()))
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

fn fingerprint_directory(
    root: &Path,
    directory: &Path,
    ignore_matcher: &Gitignore,
    generated_root: Option<&Path>,
    generated_files: &[PathBuf],
    hasher: &mut DefaultHasher,
) -> std::io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        let is_directory = file_type.is_dir();
        let is_generated = generated_root.is_some_and(|generated| path.starts_with(generated))
            || generated_files.contains(&path);
        if is_generated || ignore_matcher.matched(&path, is_directory).is_ignore() {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        relative.hash(hasher);
        is_directory.hash(hasher);
        file_type.is_file().hash(hasher);
        file_type.is_symlink().hash(hasher);

        if is_directory {
            fingerprint_directory(
                root,
                &path,
                ignore_matcher,
                generated_root,
                generated_files,
                hasher,
            )?;
            continue;
        }
        if file_type.is_symlink() {
            fs::read_link(&path)?.hash(hasher);
            continue;
        }

        metadata.len().hash(hasher);
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            duration.as_nanos().hash(hasher);
        }
    }
    Ok(())
}
