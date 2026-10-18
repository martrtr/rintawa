//! Secret path and value types shared by Rintawa hosts and extensions.
//!
//! A secret path is an identifier, never a filesystem path. It consists of
//! lowercase ASCII segments separated by dots, for example
//! `ai.api_keys.openai`. Permission patterns are intentionally more limited:
//! `ai.api_keys.*` grants access only below that exact segment boundary.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A validated, host-owned identifier for a secret.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretPath(String);

impl SecretPath {
    /// Parses a canonical dotted secret path.
    ///
    /// # Errors
    ///
    /// Returns [`SecretPathError::InvalidPath`] when the path has an empty
    /// segment or a segment outside the allowed lowercase ASCII alphabet.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, SecretPathError> {
        let value = value.as_ref();
        if value.is_empty()
            || value.split('.').any(|segment| {
                segment.is_empty()
                    || !segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            })
        {
            return Err(SecretPathError::InvalidPath(value.to_string()));
        }

        Ok(Self(value.to_string()))
    }

    /// Returns the canonical dotted path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Debug for SecretPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SecretPath").field(&self.0).finish()
    }
}

impl Serialize for SecretPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecretPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A validated pattern used only by host permission policy.
///
/// An exact pattern such as `ai.api_keys.openai` grants one secret. A suffix
/// `.*` grants descendants below the preceding segment boundary; it never
/// matches a similarly named sibling such as `ai.api_keys_backup.openai`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SecretPathPattern {
    /// Grants exactly one secret path.
    Exact(SecretPath),
    /// Grants every descendant of the prefix path.
    Descendants(SecretPath),
}

impl SecretPathPattern {
    /// Parses an exact path or a trailing `.*` domain pattern.
    ///
    /// # Errors
    ///
    /// Returns [`SecretPathError::InvalidPattern`] when the wildcard is not a
    /// complete trailing segment or the remaining prefix is not a valid path.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, SecretPathError> {
        let value = value.as_ref();
        if let Some(prefix) = value.strip_suffix(".*") {
            if prefix.is_empty() || prefix.contains('*') {
                return Err(SecretPathError::InvalidPattern(value.to_string()));
            }
            return SecretPath::parse(prefix)
                .map(Self::Descendants)
                .map_err(|_| SecretPathError::InvalidPattern(value.to_string()));
        }

        if value.contains('*') {
            return Err(SecretPathError::InvalidPattern(value.to_string()));
        }

        SecretPath::parse(value).map(Self::Exact)
    }

    /// Returns whether this pattern permits the supplied path.
    pub fn matches(&self, path: &SecretPath) -> bool {
        match self {
            Self::Exact(exact) => exact == path,
            Self::Descendants(prefix) => path
                .as_str()
                .strip_prefix(prefix.as_str())
                .is_some_and(|remaining| remaining.starts_with('.')),
        }
    }

    /// Returns whether this requested pattern can authorize the supplied grant.
    ///
    /// A host may narrow a request, but it may never expand it. For example,
    /// a request for `ai.api_keys.*` can be granted as
    /// `ai.api_keys.openai`, while a request for one exact key cannot be
    /// expanded into a domain grant.
    pub fn allows_pattern(&self, grant: &Self) -> bool {
        match (self, grant) {
            (Self::Exact(requested), Self::Exact(granted)) => requested == granted,
            (Self::Descendants(_), Self::Exact(granted)) => self.matches(granted),
            (Self::Descendants(requested), Self::Descendants(granted)) => {
                granted == requested
                    || granted
                        .as_str()
                        .strip_prefix(requested.as_str())
                        .is_some_and(|remaining| remaining.starts_with('.'))
            }
            (Self::Exact(_), Self::Descendants(_)) => false,
        }
    }

    /// Returns the canonical policy pattern.
    pub fn as_str(&self) -> String {
        match self {
            Self::Exact(path) => path.as_str().to_string(),
            Self::Descendants(prefix) => format!("{}.*", prefix.as_str()),
        }
    }
}

impl fmt::Display for SecretPathPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(formatter)
    }
}

impl fmt::Debug for SecretPathPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretPathPattern")
            .field(&self.as_str())
            .finish()
    }
}

impl Serialize for SecretPathPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecretPathPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A value that redacts itself in debug output and zeroizes its owned buffer.
pub struct SecretValue(SecretString);

impl SecretValue {
    /// Wraps a secret supplied by a trusted Rintawa host configuration flow.
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    /// Exposes the value to code that already holds an explicitly granted capability.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

/// Validation failures for secret paths and policy patterns.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SecretPathError {
    /// The supplied path is not a canonical dotted secret path.
    #[error("invalid secret path `{0}`")]
    InvalidPath(String),
    /// The supplied pattern is not an exact path or a trailing `.*` domain pattern.
    #[error("invalid secret path pattern `{0}`")]
    InvalidPattern(String),
}

/// Failures visible to extension code when reading an explicitly requested secret.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SecretAccessError {
    /// The requested path is malformed.
    #[error("invalid secret path")]
    InvalidPath,
    /// The component has no active grant for the requested path.
    #[error("secret access denied")]
    AccessDenied,
    /// No secret has been configured at the requested path.
    #[error("secret is not configured")]
    NotFound,
    /// The host credential store is unavailable.
    #[error("secret store is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_match_secret_domain_only_at_segment_boundaries() {
        let pattern = SecretPathPattern::parse("ai.api_keys.*").unwrap();
        let allowed = SecretPath::parse("ai.api_keys.openai").unwrap();
        let nested = SecretPath::parse("ai.api_keys.openai.production").unwrap();
        let denied = SecretPath::parse("ai.api_keys_backup.openai").unwrap();
        let prefix = SecretPath::parse("ai.api_keys").unwrap();

        assert!(pattern.matches(&allowed));
        assert!(pattern.matches(&nested));
        assert!(!pattern.matches(&denied));
        assert!(!pattern.matches(&prefix));
    }

    #[test]
    fn test_should_reject_noncanonical_secret_paths_and_patterns() {
        assert!(SecretPath::parse("ai..openai").is_err());
        assert!(SecretPath::parse("ai.API_KEYS.openai").is_err());
        assert!(SecretPathPattern::parse("ai.*.openai").is_err());
        assert!(SecretPathPattern::parse("ai.api_keys*").is_err());
    }

    #[test]
    fn test_should_allow_host_to_narrow_but_not_expand_secret_request() {
        let requested = SecretPathPattern::parse("ai.api_keys.*").unwrap();
        let narrowed = SecretPathPattern::parse("ai.api_keys.openai").unwrap();
        let expanded = SecretPathPattern::parse("ai.*").unwrap();

        assert!(requested.allows_pattern(&narrowed));
        assert!(!narrowed.allows_pattern(&requested));
        assert!(!requested.allows_pattern(&expanded));
    }

    #[test]
    fn test_should_redact_secret_value_debug_output() {
        let value = SecretValue::new("do-not-log-me");

        assert_eq!(format!("{value:?}"), "SecretValue([REDACTED])");
    }
}
