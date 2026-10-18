//! Canonical paths inside RTW artifacts.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{RtwError, RtwResult};

/// A canonical relative UTF-8 file path inside an RTW artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    /// Parses and validates a canonical artifact path.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::InvalidPath`] for absolute paths, traversal, empty
    /// segments, Windows separators or drive syntax, control characters, or a
    /// trailing slash.
    pub fn parse(value: impl Into<String>) -> RtwResult<Self> {
        let value = value.into();
        validate_file_path(&value)?;
        Ok(Self(value))
    }

    /// Returns the canonical slash-separated path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for ArtifactPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ArtifactPath {
    type Err = RtwError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ArtifactPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArtifactPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}
pub(crate) fn validate_archive_name(value: &str, is_directory: bool) -> RtwResult<String> {
    let candidate = if is_directory {
        value.strip_suffix('/').unwrap_or(value)
    } else {
        value
    };
    validate_segments(candidate, is_directory)?;
    if !is_directory && value.ends_with('/') {
        return Err(invalid_path(value, "regular files cannot end with `/`"));
    }
    if is_directory && !value.ends_with('/') {
        return Err(invalid_path(value, "directory entries must end with `/`"));
    }
    Ok(candidate.to_string())
}

fn validate_file_path(value: &str) -> RtwResult<()> {
    validate_segments(value, false)?;
    if value.ends_with('/') {
        return Err(invalid_path(value, "file paths cannot end with `/`"));
    }
    Ok(())
}

fn validate_segments(value: &str, is_directory: bool) -> RtwResult<()> {
    if value.is_empty() {
        return Err(invalid_path(value, "path cannot be empty"));
    }
    if value.starts_with('/') {
        return Err(invalid_path(value, "absolute paths are not allowed"));
    }
    if value.contains('\\') {
        return Err(invalid_path(value, "backslashes are not allowed"));
    }
    if value.contains(':') {
        return Err(invalid_path(
            value,
            "colon is not allowed in portable artifact paths",
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid_path(value, "control characters are not allowed"));
    }

    let normalized = if is_directory {
        value.strip_suffix('/').unwrap_or(value)
    } else {
        value
    };
    if normalized.is_empty() {
        return Err(invalid_path(
            value,
            "root directory entries are not allowed",
        ));
    }
    if normalized.split('/').any(|segment| segment.is_empty()) {
        return Err(invalid_path(value, "empty path segments are not allowed"));
    }
    if normalized
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(invalid_path(
            value,
            "`.` and `..` path segments are not allowed",
        ));
    }
    Ok(())
}
fn invalid_path(path: &str, reason: &'static str) -> RtwError {
    RtwError::InvalidPath {
        path: path.to_string(),
        reason,
    }
}

pub(crate) fn parent_paths(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(|(index, _)| &path[..index])
}
