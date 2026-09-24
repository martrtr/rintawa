//! Public world-model identifiers and schema keys.
//!
//! These types are feature-neutral contracts shared by the authoritative world
//! runtime and extensions. They intentionally do not define Chat, Character,
//! AI, inventory, or other feature semantics.

use std::{fmt, num::NonZeroU32, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MAX_SCHEMA_ID_BYTES: usize = 128;

/// Error returned when an opaque 128-bit world identifier cannot be parsed.
#[derive(Debug, Error)]
#[error("invalid Rintawa 128-bit identifier: {0}")]
pub struct IdentifierParseError(#[from] uuid::Error);

macro_rules! opaque_world_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a new time-ordered UUIDv7 identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Creates the identifier from an existing UUID value.
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Creates the identifier from its exact 16-byte representation.
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }

            /// Returns the exact 16-byte representation.
            pub fn into_bytes(self) -> [u8; 16] {
                self.0.into_bytes()
            }

            /// Returns the wrapped UUID value.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = IdentifierParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }
    };
}

opaque_world_id!(
    WorldId,
    "Opaque globally unique identifier of one authoritative world."
);
opaque_world_id!(EntityId, "Opaque globally unique identifier of one entity.");
opaque_world_id!(
    RelationId,
    "Opaque globally unique identifier of one relation."
);
opaque_world_id!(
    CommandId,
    "Opaque globally unique identifier of one submitted world command."
);
opaque_world_id!(
    WorldEventId,
    "Opaque globally unique identifier of one durable world event."
);
opaque_world_id!(
    EffectJobId,
    "Opaque globally unique identifier of one durable external effect job."
);
opaque_world_id!(
    PrincipalId,
    "Opaque globally unique identifier of one authenticated outside principal."
);
opaque_world_id!(
    ControlGrantId,
    "Opaque globally unique identifier of one durable actor-control grant."
);
opaque_world_id!(
    CorrelationId,
    "Opaque globally unique identifier that correlates one causal operation chain."
);

/// Signed Unix timestamp in milliseconds used for durable provenance metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixTimeMillis(i64);

impl UnixTimeMillis {
    /// Creates a timestamp from milliseconds relative to the Unix epoch.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the signed millisecond value relative to the Unix epoch.
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Semantic category of one versioned world schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemaKind {
    /// Schema identifying an entity type.
    Entity,
    /// Schema identifying a relation type.
    Relation,
    /// Schema describing one extension-owned facet payload.
    Facet,
    /// Schema describing one world command payload.
    Command,
    /// Schema describing one durable world-event payload.
    Event,
    /// Schema describing one durable external effect/job payload.
    Effect,
    /// Schema describing one extension-owned policy-filtered projection payload.
    Projection,
}

impl fmt::Display for SchemaKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Entity => "entity",
            Self::Relation => "relation",
            Self::Facet => "facet",
            Self::Command => "command",
            Self::Event => "event",
            Self::Effect => "effect",
            Self::Projection => "projection",
        })
    }
}

/// Error returned when a schema identifier is not canonical.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid schema identifier {value}: {reason}")]
pub struct SchemaIdentifierError {
    value: String,
    reason: &'static str,
}

impl SchemaIdentifierError {
    fn new(value: impl Into<String>, reason: &'static str) -> Self {
        Self {
            value: value.into(),
            reason,
        }
    }

    /// Returns the rejected identifier text.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the canonical-format rule that was violated.
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Canonical unversioned identifier of one world schema.
///
/// Schema IDs are lowercase ASCII namespaces such as rintawa.character.identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaId(String);

impl SchemaId {
    /// Parses and validates a canonical schema identifier.
    ///
    /// # Errors
    ///
    /// Returns SchemaIdentifierError when the identifier is empty, too long,
    /// not namespaced, or contains a non-canonical segment.
    pub fn parse(value: impl Into<String>) -> Result<Self, SchemaIdentifierError> {
        let value = value.into();
        validate_schema_id(&value)?;
        Ok(Self(value))
    }

    /// Returns the canonical identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SchemaId {
    type Err = SchemaIdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Error returned when schema version zero is requested.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("schema version must be greater than zero")]
pub struct SchemaVersionError;

/// Positive major version of one world schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(NonZeroU32);

impl SchemaVersion {
    /// Creates a positive schema version.
    ///
    /// # Errors
    ///
    /// Returns SchemaVersionError when value is zero.
    pub fn new(value: u32) -> Result<Self, SchemaVersionError> {
        NonZeroU32::new(value).map(Self).ok_or(SchemaVersionError)
    }

    /// Returns the numeric version.
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

/// Error returned when a versioned schema key cannot be parsed.
#[derive(Debug, Error)]
pub enum SchemaKeyParseError {
    /// The version suffix is missing.
    #[error("schema key must use <schema-id>@<version>")]
    MissingVersion,
    /// The schema identifier is not canonical.
    #[error(transparent)]
    Identifier(#[from] SchemaIdentifierError),
    /// The version is not a positive integer.
    #[error("invalid schema version {0}")]
    Version(String),
}

/// Stable versioned identity of one world schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SchemaKey {
    id: SchemaId,
    version: SchemaVersion,
}

impl SchemaKey {
    /// Creates a versioned schema key.
    pub const fn new(id: SchemaId, version: SchemaVersion) -> Self {
        Self { id, version }
    }

    /// Returns the unversioned schema identifier.
    pub const fn id(&self) -> &SchemaId {
        &self.id
    }

    /// Returns the positive schema version.
    pub const fn version(&self) -> SchemaVersion {
        self.version
    }
}

impl fmt::Display for SchemaKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.id, self.version)
    }
}

impl FromStr for SchemaKey {
    type Err = SchemaKeyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((id, version)) = value.rsplit_once('@') else {
            return Err(SchemaKeyParseError::MissingVersion);
        };
        let id = SchemaId::parse(id)?;
        let parsed = version
            .parse::<u32>()
            .map_err(|_| SchemaKeyParseError::Version(version.to_string()))?;
        let version = SchemaVersion::new(parsed)
            .map_err(|_| SchemaKeyParseError::Version(version.to_string()))?;
        Ok(Self::new(id, version))
    }
}

fn validate_schema_id(value: &str) -> Result<(), SchemaIdentifierError> {
    if value.is_empty() {
        return Err(SchemaIdentifierError::new(value, "identifier is empty"));
    }
    if value.len() > MAX_SCHEMA_ID_BYTES {
        return Err(SchemaIdentifierError::new(value, "identifier is too long"));
    }

    let mut segments = value.split('.');
    let Some(first) = segments.next() else {
        return Err(SchemaIdentifierError::new(value, "identifier is empty"));
    };
    let Some(second) = segments.next() else {
        return Err(SchemaIdentifierError::new(
            value,
            "identifier must contain at least two namespace segments",
        ));
    };
    if !is_schema_segment(first) || !is_schema_segment(second) || !segments.all(is_schema_segment) {
        return Err(SchemaIdentifierError::new(
            value,
            "segments must start with lowercase ASCII letters and contain only lowercase letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

fn is_schema_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_generate_distinct_uuid_v7_world_ids() {
        let first = WorldId::new();
        let second = WorldId::new();

        assert_ne!(first, second);
        assert_eq!(first.as_uuid().get_version_num(), 7);
        assert_eq!(second.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn test_should_round_trip_world_id_text_and_bytes() -> Result<(), IdentifierParseError> {
        let original = WorldId::new();
        let parsed: WorldId = original.to_string().parse()?;
        let from_bytes = WorldId::from_bytes(original.into_bytes());

        assert_eq!(parsed, original);
        assert_eq!(from_bytes, original);
        Ok(())
    }

    #[test]
    fn test_should_accept_namespaced_schema_id() -> Result<(), SchemaIdentifierError> {
        let id = SchemaId::parse("rintawa.character.identity")?;
        assert_eq!(id.as_str(), "rintawa.character.identity");
        Ok(())
    }

    #[test]
    fn test_should_reject_non_canonical_schema_id() {
        let error = SchemaId::parse("Rintawa.Character").unwrap_err();
        assert_eq!(error.value(), "Rintawa.Character");
    }

    #[test]
    fn test_should_round_trip_schema_key() -> Result<(), Box<dyn std::error::Error>> {
        let key: SchemaKey = "rintawa.character.identity@1".parse()?;

        assert_eq!(key.to_string(), "rintawa.character.identity@1");
        assert_eq!(key.version().get(), 1);
        Ok(())
    }

    #[test]
    fn test_should_reject_zero_schema_version() {
        let error = "rintawa.character.identity@0"
            .parse::<SchemaKey>()
            .unwrap_err();
        assert!(matches!(error, SchemaKeyParseError::Version(_)));
    }
}
