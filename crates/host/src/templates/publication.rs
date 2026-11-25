//! Private World construction and one-directory publication.

use crate::{
    CompositionProfile, HostError, HostHome, HostResult, WORLD_COMPOSITION_FILE,
    WORLD_DATABASE_FILE, ensure_real_directory, world_runtime_scope_id, world_summary,
};
use rintawa_sdk::world::WorldId;
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::WorldManifest;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::Path,
};
use tempfile::TempDir;

pub(super) struct Stage {
    pub directory: TempDir,
    pub(super) _ownership: File,
}
fn coordination(home: &HostHome) -> HostResult<File> {
    let path = home.root.join("world-staging.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(HostError::InvalidTemplate(
            "invalid staging coordination lock".into(),
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(HostError::InvalidTemplate(
            "World staging is busy; retry explicitly".into(),
        )),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}
impl Stage {
    pub fn new(home: &HostHome) -> HostResult<Self> {
        let _coordination = coordination(home)?;
        let parent = home.root.join("world-staging");
        ensure_real_directory(&parent)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("world-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let directory = builder.tempdir_in(parent)?;
        let ownership = CompositionProfile::lock(&directory.path().join(WORLD_COMPOSITION_FILE))?;
        Ok(Self {
            directory,
            _ownership: ownership,
        })
    }
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}
impl HostHome {
    pub(crate) fn lock_world_lifecycle(&self, world: WorldId) -> HostResult<File> {
        let path = self.existing_world_directory(world)?.join("runtime.lock");
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(HostError::InvalidTemplate(
                "unsafe World lifecycle lock".into(),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(HostError::WorldLifecycleBusy(world)),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
    /// Reclaims interrupted private construction, skipping any active stage.
    ///
    /// This explicit startup/admin operation is not run from guest Host imports.
    pub fn recover_world_staging(&self) -> HostResult<()> {
        let _coordination = coordination(self)?;
        let parent = self.root.join("world-staging");
        ensure_real_directory(&parent)?;
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let metadata = entry.file_type()?;
            if !metadata.is_dir()
                || metadata.is_symlink()
                || !entry.file_name().to_string_lossy().starts_with("world-")
            {
                return Err(HostError::InvalidTemplate(
                    "unexpected staging entry".into(),
                ));
            }
            let lock_path = entry
                .path()
                .join(WORLD_COMPOSITION_FILE)
                .with_extension("toml.lock");
            if let Ok(metadata) = fs::symlink_metadata(&lock_path)
                && (!metadata.is_file() || metadata.file_type().is_symlink())
            {
                return Err(HostError::InvalidTemplate(
                    "unsafe staging owner lock".into(),
                ));
            }
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?;
            match lock.try_lock() {
                Ok(()) => {
                    fs::remove_dir_all(entry.path())?;
                }
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Ok(())
    }
    pub(crate) fn publish_world(
        &self,
        recipe: &CompositionProfile,
        seed: Option<&Path>,
        manifest: &WorldManifest,
    ) -> HostResult<crate::CreatedTemplateWorld> {
        self.publish_world_with_profile(recipe, seed, manifest, |_| {})
    }

    pub(super) fn publish_world_with_profile(
        &self,
        recipe: &CompositionProfile,
        seed: Option<&Path>,
        manifest: &WorldManifest,
        configure: impl FnOnce(&mut CompositionProfile),
    ) -> HostResult<crate::CreatedTemplateWorld> {
        let stage = Stage::new(self)?;
        let world = WorldId::new();
        let database = stage.path().join(WORLD_DATABASE_FILE);
        let storage = match seed {
            Some(seed) => SqliteWorldStorage::create_from_seed(seed, &database, world, manifest)?,
            None => SqliteWorldStorage::create_with_manifest(&database, world, manifest)?,
        };
        let mut profile = recipe.materialize_all(world_runtime_scope_id(world));
        configure(&mut profile);
        #[cfg(test)]
        crate::BEFORE_INITIAL_COMPOSITION.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook(world);
            }
        });
        profile.save(&stage.path().join(WORLD_COMPOSITION_FILE))?;
        let summary = world_summary(&storage.load_session()?)?;
        let entry = profile.entry.as_ref().map(|entry| {
            rintawa_sdk::contracts::ComponentRef::new(
                entry.instance_id.clone(),
                entry.component_id.clone(),
            )
        });
        drop(storage); // Close/checkpoint SQLite before moving its containing directory.
        File::open(&database)?.sync_all()?;
        sync_directory(stage.path())?;
        let _coordination = coordination(self)?;
        let destination = self.worlds_directory.join(world.to_string());
        if destination.try_exists()? {
            return Err(HostError::WorldIdCollision);
        }
        fs::rename(stage.path(), destination)?;
        let _ = stage.directory.keep();
        // Failure here is an unknown publication outcome. Never delete a published World.
        sync_directory(&self.worlds_directory)?;
        Ok(crate::CreatedTemplateWorld {
            world: summary,
            entry,
        })
    }
}

pub(super) fn sync_directory(path: &Path) -> HostResult<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
