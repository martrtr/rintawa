//! Bounded in-memory queue for ephemeral world-scoped runtime signals.

use std::collections::VecDeque;

use rintawa_sdk::runtime_signals::RuntimeSignal;

use crate::{HostError, HostResult};

/// Default number of ephemeral signals that may wait per active world.
pub const DEFAULT_RUNTIME_SIGNAL_QUEUE_CAPACITY: usize = 128;
/// Maximum UTF-8 bytes accepted for one runtime-signal topic.
pub const MAX_RUNTIME_SIGNAL_TOPIC_BYTES: usize = 256;
/// Maximum encoded envelope bytes accepted for one queued runtime signal.
pub const MAX_RUNTIME_SIGNAL_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) struct QueuedRuntimeSignal {
    topic: String,
    encoded: Vec<u8>,
}

impl QueuedRuntimeSignal {
    pub(crate) fn topic(&self) -> &str {
        &self.topic
    }

    pub(crate) fn encoded(&self) -> &[u8] {
        &self.encoded
    }
}

pub(crate) struct RuntimeSignalQueue {
    capacity: usize,
    queue: VecDeque<QueuedRuntimeSignal>,
}

impl Default for RuntimeSignalQueue {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_RUNTIME_SIGNAL_QUEUE_CAPACITY)
    }
}

impl RuntimeSignalQueue {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
        }
    }

    pub(crate) fn enqueue(&mut self, signal: RuntimeSignal) -> HostResult<()> {
        let topic = signal.topic();
        if topic.trim().is_empty() || topic.len() > MAX_RUNTIME_SIGNAL_TOPIC_BYTES {
            return Err(HostError::InvalidRuntimeSignalTopic);
        }
        if self.queue.len() >= self.capacity {
            return Err(HostError::RuntimeSignalQueueFull(signal.world_id()));
        }
        let encoded = serde_json::to_vec(&signal)
            .map_err(|source| HostError::RuntimeSignalEncode { source })?;
        if encoded.len() > MAX_RUNTIME_SIGNAL_MESSAGE_BYTES {
            return Err(HostError::RuntimeSignalMessageTooLarge {
                actual_bytes: encoded.len(),
                maximum_bytes: MAX_RUNTIME_SIGNAL_MESSAGE_BYTES,
            });
        }
        self.queue.push_back(QueuedRuntimeSignal {
            topic: topic.to_string(),
            encoded,
        });
        Ok(())
    }

    pub(crate) fn pop_front(&mut self) -> Option<QueuedRuntimeSignal> {
        self.queue.pop_front()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }
}
