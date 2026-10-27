//! Exact, World-scoped cursor for bounded pagination of durable events.
//!
//! Commit positions alone are insufficient: a single authoritative command
//! can emit multiple events. The event index is needed to resume in the
//! middle of that commit without omitting the remaining events.

use rintawa_sdk::world::WorldId;
use rintawa_world::StoredWorldEvent;

/// Exact position immediately after a *particular* committed World event.
///
/// Never treat a partial page as a complete commit. Cursor progression is
/// meaningful only inside the owning World; the storage API checks WorldId
/// before reading from its database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldEventCursor {
    world_id: WorldId,
    commit_position: u64,
    event_index: u32,
}

impl WorldEventCursor {
    /// The cursor immediately after this immutable stored event.
    pub const fn after(event: &StoredWorldEvent) -> Self {
        Self {
            world_id: event.world_id,
            commit_position: event.commit_position,
            event_index: event.event_index,
        }
    }

    /// Legacy position-only reads must skip every event from this commit.
    pub(crate) const fn after_complete_position(world_id: WorldId, commit_position: u64) -> Self {
        Self {
            world_id,
            commit_position,
            event_index: u32::MAX,
        }
    }

    /// World whose ordering domain this cursor belongs to.
    pub const fn world_id(&self) -> WorldId {
        self.world_id
    }

    /// Commit containing the last consumed event.
    pub const fn commit_position(&self) -> u64 {
        self.commit_position
    }

    /// Event's position within its authoritative commit.
    pub const fn event_index(&self) -> u32 {
        self.event_index
    }
}
