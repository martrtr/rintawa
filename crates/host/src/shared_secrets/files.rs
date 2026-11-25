//! Linux directory-relative storage for private, bounded credential records.

use crate::shared_secrets::SecretStorageError;
use rustix::{
    fs::{self, AtFlags, FileType, Mode, OFlags},
    io::Errno,
    process::geteuid,
};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, SecretStorageError>;
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

pub(crate) struct Directory(File);

impl Directory {
    pub(crate) fn credentials(home: &Path, create: bool) -> Result<Option<Self>> {
        let mut parent = File::from(fs::open("/", DIRECTORY_FLAGS, Mode::empty()).map_err(io)?);
        for component in home.components() {
            let Component::Normal(name) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(SecretStorageError::UnsafeStorage);
            };
            validate_parent(&parent)?;
            parent =
                File::from(fs::openat(&parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io)?);
        }
        let stat = fs::fstat(&parent).map_err(io)?;
        if stat.st_uid != geteuid().as_raw() || stat.st_mode & 0o022 != 0 {
            return Err(SecretStorageError::UnsafeStorage);
        }
        Self(parent).child("credentials", create)
    }

    pub(crate) fn child(&self, name: &str, create: bool) -> Result<Option<Self>> {
        if create {
            match fs::mkdirat(&self.0, name, Mode::from_raw_mode(0o700)) {
                Ok(()) => self.sync()?,
                Err(Errno::EXIST) => {}
                Err(error) => return Err(io(error)),
            }
        }
        let directory = match fs::openat(&self.0, name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => Self(File::from(fd)),
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        let stat = fs::fstat(&directory.0).map_err(io)?;
        if stat.st_uid != geteuid().as_raw() || stat.st_mode & 0o7777 != 0o700 {
            return Err(SecretStorageError::UnsafeStorage);
        }
        Ok(Some(directory))
    }

    pub(crate) fn lock(&self) -> Result<File> {
        let file = File::from(
            fs::openat(
                &self.0,
                ".lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(io)?,
        );
        validate_file(&file, false)?;
        file.lock().map_err(|_| SecretStorageError::Unavailable)?;
        Ok(file)
    }

    fn open(&self, name: &str) -> Result<Option<File>> {
        match fs::openat(&self.0, name, FILE_FLAGS, Mode::empty()) {
            Ok(fd) => {
                let file = File::from(fd);
                // Rename/unlink may retire this inode after open. Its admitted
                // reader can still consume the complete old record.
                validate_file(&file, true)?;
                Ok(Some(file))
            }
            Err(Errno::NOENT) => Ok(None),
            Err(error) => Err(io(error)),
        }
    }

    pub(crate) fn read(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let Some(file) = self.open(name)? else {
            return Ok(None);
        };
        let size = file
            .metadata()
            .map_err(|_| SecretStorageError::Unavailable)?
            .len();
        if size > limit as u64 {
            return Err(SecretStorageError::CorruptStorage);
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SecretStorageError::Unavailable)?;
        if bytes.len() > limit {
            return Err(SecretStorageError::CorruptStorage);
        }
        Ok(Some(bytes))
    }

    pub(crate) fn names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::Dir::read_from(&self.0).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| SecretStorageError::CorruptStorage)?;
            if matches!(name, "." | ".." | ".lock") {
                continue;
            }
            if names.len() >= 1024 {
                return Err(SecretStorageError::QuotaExceeded);
            }
            names.push(name.to_owned());
        }
        Ok(names)
    }

    /// Only called while holding the stable writer lock.
    pub(crate) fn reclaim(&self) -> Result<()> {
        for name in self.names()? {
            if let Some(suffix) = name.strip_prefix(".tmp_") {
                if Uuid::parse_str(suffix).is_err() || self.open(&name)?.is_none() {
                    return Err(SecretStorageError::CorruptStorage);
                }
                self.unlink(&name)?;
            }
        }
        Ok(())
    }

    /// The caller holds the writer lock. No raw filesystem errors leave this module.
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        // Check existing destination even though rename itself never follows it.
        self.open(name)?;
        let temporary = format!(".tmp_{}", Uuid::new_v4().simple());
        let mut file = File::from(
            fs::openat(
                &self.0,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(io)?,
        );
        let staged = (|| {
            validate_file(&file, false)?;
            file.write_all(bytes)
                .map_err(|_| SecretStorageError::Unavailable)?;
            file.sync_all()
                .map_err(|_| SecretStorageError::Unavailable)?;
            #[cfg(test)]
            fail_at(WriteStep::BeforeRename)?;
            fs::renameat(&self.0, &temporary, &self.0, name).map_err(io)
        })();
        if let Err(error) = staged {
            if self.unlink(&temporary).is_err() {
                return Err(SecretStorageError::CleanupFailed);
            }
            return Err(error);
        }
        #[cfg(test)]
        fail_at(WriteStep::AfterRename).map_err(|_| SecretStorageError::DurabilityUnknown)?;
        self.sync()
            .map_err(|_| SecretStorageError::DurabilityUnknown)
    }

    pub(crate) fn unlink(&self, name: &str) -> Result<bool> {
        if self.open(name)?.is_none() {
            return Ok(false);
        }
        fs::unlinkat(&self.0, name, AtFlags::empty()).map_err(io)?;
        #[cfg(test)]
        fail_at(WriteStep::AfterUnlink).map_err(|_| SecretStorageError::DurabilityUnknown)?;
        self.sync()
            .map_err(|_| SecretStorageError::DurabilityUnknown)?;
        Ok(true)
    }

    fn sync(&self) -> Result<()> {
        self.0
            .sync_all()
            .map_err(|_| SecretStorageError::Unavailable)
    }
}

fn validate_file(file: &File, allow_unlinked: bool) -> Result<()> {
    let stat = fs::fstat(file).map_err(io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o600
        || (stat.st_nlink != 1 && !(allow_unlinked && stat.st_nlink == 0))
    {
        return Err(SecretStorageError::UnsafeStorage);
    }
    Ok(())
}

fn validate_parent(file: &File) -> Result<()> {
    let stat = fs::fstat(file).map_err(io)?;
    let owner_safe = stat.st_uid == 0 || stat.st_uid == geteuid().as_raw();
    // Root-owned sticky directories such as /tmp cannot rename our owned child.
    let permissions_safe =
        stat.st_mode & 0o022 == 0 || (stat.st_uid == 0 && stat.st_mode & 0o1000 != 0);
    if !owner_safe || !permissions_safe {
        return Err(SecretStorageError::UnsafeStorage);
    }
    Ok(())
}

fn io(error: Errno) -> SecretStorageError {
    match error {
        Errno::LOOP | Errno::NOTDIR => SecretStorageError::UnsafeStorage,
        _ => SecretStorageError::Unavailable,
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteStep {
    BeforeRename,
    AfterRename,
    AfterUnlink,
}

#[cfg(test)]
thread_local! {
    pub(crate) static WRITE_FAILURE: std::cell::Cell<Option<WriteStep>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn fail_at(step: WriteStep) -> Result<()> {
    WRITE_FAILURE.with(|failure| {
        if failure.get() == Some(step) {
            failure.set(None);
            Err(SecretStorageError::Unavailable)
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admitted_reader_keeps_complete_old_inode_after_atomic_replacement() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let directory = Directory::credentials(home.path(), true)?.unwrap();
        let _lock = directory.lock()?;
        directory.write("record", b"old record")?;
        // Open, then let a writer publish before the reader's fstat/read.
        let mut old = File::from(fs::openat(
            &directory.0,
            "record",
            FILE_FLAGS,
            Mode::empty(),
        )?);
        directory.write("record", b"new record")?;
        assert_eq!(fs::fstat(&old)?.st_nlink, 0);
        validate_file(&old, true)?;
        assert_eq!(
            validate_file(&old, false),
            Err(SecretStorageError::UnsafeStorage)
        );
        let mut bytes = Vec::new();
        old.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"old record");
        assert_eq!(directory.read("record", 32)?.unwrap(), b"new record");
        Ok(())
    }
}
