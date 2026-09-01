//! Reversible effects that an active extension component can own.
//!
//! Runtime effects describe current participation in a host runtime. They are
//! not world schema or other durable compatibility contracts.

use serde::{Deserialize, Serialize};

/// A reversible operation installed by an active component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEffect {
    /// Delivers matching runtime events to the owning component.
    EventSubscription {
        /// The versioned event topic to receive.
        topic: String,
    },
}

impl RuntimeEffect {
    /// Creates an event-subscription runtime effect for a topic.
    pub fn event_subscription(topic: impl Into<String>) -> Self {
        Self::EventSubscription {
            topic: topic.into(),
        }
    }
}
