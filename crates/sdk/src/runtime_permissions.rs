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
}

impl fmt::Display for RuntimePermission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackgroundTask => formatter.write_str("background-task"),
            Self::LoopbackListen => formatter.write_str("loopback-listen"),
            Self::LoopbackConnect => formatter.write_str("loopback-connect"),
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
            _ => Err(RuntimePermissionParseError(value.to_string())),
        }
    }
}
