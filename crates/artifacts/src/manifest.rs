//! RTW root manifest and content type identity.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{ArtifactPath, RtwError, RtwResult};

/// Current RTW container format version.
pub const RTW_FORMAT_VERSION: u32 = 1;
/// Root manifest path required in every RTW artifact.
pub const RTW_MANIFEST_PATH: &str = "rtw.toml";

/// Identifies one major version of an RTW root content schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentType {
    id: String,
    major: u32,
}

impl ContentType {
    /// Parses a namespaced content type such as `rintawa.extension@1`.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::InvalidContentType`] when the identifier or major
    /// version is malformed.
    pub fn parse(value: impl Into<String>) -> RtwResult<Self> {
        let value = value.into();
        let (id, major) = value
            .rsplit_once('@')
            .ok_or_else(|| invalid_content_type(&value, "expected `<namespace>.<name>@<major>`"))?;
        validate_content_id(id).map_err(|reason| invalid_content_type(&value, reason))?;
        if major.len() > 1 && major.starts_with('0') {
            return Err(invalid_content_type(
                &value,
                "major version must use canonical decimal notation",
            ));
        }
        let major = major.parse::<u32>().map_err(|_| {
            invalid_content_type(&value, "major version must be an unsigned integer")
        })?;
        Ok(Self {
            id: id.to_string(),
            major,
        })
    }

    /// Returns the stable namespaced content identifier without its major version.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the content schema major version.
    pub const fn major(&self) -> u32 {
        self.major
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.id, self.major)
    }
}
impl FromStr for ContentType {
    type Err = RtwError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ContentType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ContentType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Minimal root manifest stored as `rtw.toml` in every RTW artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtwManifest {
    /// RTW container format version.
    pub format: u32,
    /// Root content schema understood by a built-in or extension-provided handler.
    pub content: ContentType,
    /// Handler-specific descriptor entry inside the artifact.
    pub entry: ArtifactPath,
}

impl RtwManifest {
    /// Validates host-independent RTW manifest invariants.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::UnsupportedFormat`] if `format` is not supported.
    pub fn validate(&self) -> RtwResult<()> {
        if self.format != RTW_FORMAT_VERSION {
            return Err(RtwError::UnsupportedFormat {
                found: self.format,
                supported: RTW_FORMAT_VERSION,
            });
        }
        Ok(())
    }
}

fn validate_content_id(value: &str) -> Result<(), &'static str> {
    if value.len() > 128 {
        return Err("identifier must not exceed 128 bytes");
    }
    let segments: Vec<_> = value.split('.').collect();
    if segments.len() < 2 {
        return Err("identifier must contain at least one namespace separator `.`");
    }
    for segment in segments {
        if segment.is_empty() {
            return Err("namespace segments cannot be empty");
        }
        if !segment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        }) {
            return Err("identifier uses characters outside `[a-z0-9_-]` and `.` separators");
        }
        let first = segment.as_bytes().first();
        let last = segment.as_bytes().last();
        if !first.is_some_and(u8::is_ascii_alphanumeric)
            || !last.is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err("namespace segments must start and end with an ASCII letter or digit");
        }
    }
    Ok(())
}

fn invalid_content_type(value: &str, reason: &'static str) -> RtwError {
    RtwError::InvalidContentType {
        value: value.to_string(),
        reason,
    }
}
