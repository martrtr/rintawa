//! Ephemeral runtime signals exchanged inside one active composition scope.
//!
//! Runtime signals are deliberately not authoritative world history. They are
//! suitable for transient progress, streaming chunks, telemetry, and hooks that
//! may be dropped when the host or world runtime stops.

use serde::{Deserialize, Serialize};

use crate::world::{CorrelationId, WorldId};

/// One ephemeral message associated with an active authoritative world.
///
/// The host routes `topic` only inside the runtime scope derived from `world_id`.
/// `payload` is opaque to Core; extensions own its versioned application meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RuntimeSignal {
    world_id: WorldId,
    topic: String,
    payload: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correlation_id: Option<CorrelationId>,
}

impl RuntimeSignal {
    /// Creates an uncorrelated ephemeral signal.
    pub fn new(world_id: WorldId, topic: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            world_id,
            topic: topic.into(),
            payload: payload.into(),
            correlation_id: None,
        }
    }

    /// Attaches an optional causal correlation identity used by higher-level operations.
    #[must_use]
    pub fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    /// Returns the active world whose runtime scope owns this signal.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Returns the exact ephemeral routing topic.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Returns the opaque extension-owned signal payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the optional causal correlation identity.
    pub const fn correlation_id(&self) -> Option<CorrelationId> {
        self.correlation_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_round_trip_runtime_signal_envelope() -> Result<(), serde_json::Error> {
        let world_id = WorldId::new();
        let correlation_id = CorrelationId::new();
        let original = RuntimeSignal::new(world_id, "rintawa.operation.chunk@1", b"hello".to_vec())
            .with_correlation_id(correlation_id);

        let encoded = serde_json::to_vec(&original)?;
        let decoded: RuntimeSignal = serde_json::from_slice(&encoded)?;

        assert_eq!(decoded, original);
        assert_eq!(decoded.world_id(), world_id);
        assert_eq!(decoded.topic(), "rintawa.operation.chunk@1");
        assert_eq!(decoded.payload(), b"hello");
        assert_eq!(decoded.correlation_id(), Some(correlation_id));
        Ok(())
    }
}
