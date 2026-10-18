//! Stable host-owned local Principal identity persistence.

use std::{io::Write, path::Path};

use rintawa_sdk::world::PrincipalId;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{HostError, HostResult};

/// Current persistent local-host identity metadata schema.
pub const HOST_IDENTITY_SCHEMA: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostIdentity {
    schema: u32,
    local_principal: PrincipalId,
}

impl HostIdentity {
    fn new() -> Self {
        Self {
            schema: HOST_IDENTITY_SCHEMA,
            local_principal: PrincipalId::new(),
        }
    }

    fn load(path: &Path) -> HostResult<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(HostError::InvalidHostIdentityFile {
                path: path.to_path_buf(),
                reason: "host identity path must be a real regular file",
            });
        }
        let source = std::fs::read_to_string(path)?;
        let identity: Self = toml::from_str(&source).map_err(HostError::HostIdentityDecode)?;
        if identity.schema != HOST_IDENTITY_SCHEMA {
            return Err(HostError::UnsupportedHostIdentitySchema(identity.schema));
        }
        Ok(identity)
    }

    fn persist_new(&self, path: &Path) -> HostResult<bool> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "host identity path has no parent",
            )
        })?;
        let source = toml::to_string_pretty(self).map_err(HostError::HostIdentityEncode)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(source.as_bytes())?;
        temporary.as_file_mut().sync_all()?;
        match temporary.persist_noclobber(path) {
            Ok(_) => Ok(true),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(HostError::Io(error.error)),
        }
    }
}

pub(crate) fn load_or_create_local_principal(path: &Path) -> HostResult<PrincipalId> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(HostIdentity::load(path)?.local_principal),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let generated = HostIdentity::new();
    if generated.persist_new(path)? {
        Ok(generated.local_principal)
    } else {
        Ok(HostIdentity::load(path)?.local_principal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_persist_stable_local_principal() -> anyhow::Result<()> {
        let root = tempfile::TempDir::new()?;
        let path = root.path().join("identity.toml");
        let first = load_or_create_local_principal(&path)?;
        let second = load_or_create_local_principal(&path)?;
        assert_eq!(first, second);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_should_reject_symlink_host_identity() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::TempDir::new()?;
        let target = root.path().join("target.toml");
        std::fs::write(
            &target,
            "schema = 1\nlocal_principal = \"018f0000-0000-7000-8000-000000000001\"\n",
        )?;
        let identity = root.path().join("identity.toml");
        symlink(target, &identity)?;
        assert!(matches!(
            load_or_create_local_principal(&identity),
            Err(HostError::InvalidHostIdentityFile { .. })
        ));
        Ok(())
    }
}
