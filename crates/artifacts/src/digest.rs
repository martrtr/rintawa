//! Content digests for immutable RTW artifacts.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use crate::{RtwError, RtwResult};

const SHA256_BYTES: usize = 32;
const SHA256_HEX_CHARS: usize = SHA256_BYTES * 2;
const SHA256_PREFIX: &str = "sha256:";

/// SHA-256 identity of the complete byte representation of one RTW artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactDigest([u8; SHA256_BYTES]);

impl ArtifactDigest {
    /// Computes a digest for an in-memory RTW byte sequence.
    pub fn sha256(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(digest.into())
    }

    /// Returns the canonical lowercase hexadecimal digest without the algorithm prefix.
    pub fn hex(&self) -> String {
        encode_hex(&self.0)
    }
    /// Parses the canonical `sha256:<lowercase-hex>` representation.
    ///
    /// # Errors
    ///
    /// Returns [`RtwError::InvalidDigest`] for another algorithm, non-canonical
    /// hexadecimal text, or an invalid SHA-256 length.
    pub fn parse(value: impl AsRef<str>) -> RtwResult<Self> {
        let value = value.as_ref();
        let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
            return Err(RtwError::InvalidDigest(value.to_string()));
        };
        if hex.len() != SHA256_HEX_CHARS
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RtwError::InvalidDigest(value.to_string()));
        }

        let mut bytes = [0_u8; SHA256_BYTES];
        let (pairs, _) = hex.as_bytes().as_chunks::<2>();
        for (index, pair) in pairs.iter().enumerate() {
            bytes[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
        }
        Ok(Self(bytes))
    }

    pub(crate) fn from_sha256_bytes(bytes: [u8; SHA256_BYTES]) -> Self {
        Self(bytes)
    }
}
impl fmt::Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{SHA256_PREFIX}{}", self.hex())
    }
}

impl FromStr for ArtifactDigest {
    type Err = RtwError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ArtifactDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ArtifactDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}
fn encode_hex(bytes: &[u8; SHA256_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(SHA256_HEX_CHARS);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}
