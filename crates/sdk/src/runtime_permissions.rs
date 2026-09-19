//! Host-controlled runtime permissions requested by extension components.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Coarse runtime capability that a component may request from the host.
///
/// A manifest request never grants access by itself. The host must explicitly
/// approve the exact permission for the concrete component principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePermission {
    /// Allows owner-scoped cooperative background task scheduling.
    BackgroundTask,
    /// Allows binding a TCP listener only on the local loopback interface.
    LoopbackListen,
    /// Allows opening a TCP connection only to the local loopback interface.
    LoopbackConnect,
    /// Allows bounded outbound HTTPS GET requests through the host HTTP client.
    HttpFetch,
    /// Allows importing validated RTW bytes into the local immutable artifact store.
    ArtifactImport,
    /// Allows reading the selected host composition without mutating it.
    CompositionRead,
    /// Allows changing exact artifact selections and enabled state in host composition.
    CompositionWrite,
    /// Allows inspecting requested/granted runtime permission policy for baseline components.
    RuntimePolicyRead,
    /// Allows granting/revoking requested runtime permissions for exact baseline components.
    RuntimePolicyWrite,
}

impl fmt::Display for RuntimePermission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackgroundTask => formatter.write_str("background-task"),
            Self::LoopbackListen => formatter.write_str("loopback-listen"),
            Self::LoopbackConnect => formatter.write_str("loopback-connect"),
            Self::HttpFetch => formatter.write_str("http-fetch"),
            Self::ArtifactImport => formatter.write_str("artifact-import"),
            Self::CompositionRead => formatter.write_str("composition-read"),
            Self::CompositionWrite => formatter.write_str("composition-write"),
            Self::RuntimePolicyRead => formatter.write_str("runtime-policy-read"),
            Self::RuntimePolicyWrite => formatter.write_str("runtime-policy-write"),
        }
    }
}

/// Error returned when textual runtime permission policy is not recognized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePermissionParseError(String);

impl fmt::Display for RuntimePermissionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown runtime permission `{}`", self.0)
    }
}

impl std::error::Error for RuntimePermissionParseError {}

impl FromStr for RuntimePermission {
    type Err = RuntimePermissionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "background-task" => Ok(Self::BackgroundTask),
            "loopback-listen" => Ok(Self::LoopbackListen),
            "loopback-connect" => Ok(Self::LoopbackConnect),
            "http-fetch" => Ok(Self::HttpFetch),
            "artifact-import" => Ok(Self::ArtifactImport),
            "composition-read" => Ok(Self::CompositionRead),
            "composition-write" => Ok(Self::CompositionWrite),
            "runtime-policy-read" => Ok(Self::RuntimePolicyRead),
            "runtime-policy-write" => Ok(Self::RuntimePolicyWrite),
            _ => Err(RuntimePermissionParseError(value.to_string())),
        }
    }
}
