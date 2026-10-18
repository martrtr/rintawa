//! Portable UI snapshots and incremental patch batches.

use serde::{Deserialize, Serialize};

use crate::ui::{UiNode, UiNodeId, UiSurfaceId};

/// Full presentation snapshot for one mounted surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSurfaceSnapshot {
    /// Surface receiving this snapshot.
    pub surface_id: UiSurfaceId,
    /// Monotonic presentation revision.
    pub revision: u64,
    /// Root node of the presentation tree.
    pub root: UiNodeId,
    /// Flat node table referenced by node identifiers.
    ///
    /// Vector order has no rendering semantics. Renderers must traverse from
    /// [`Self::root`] through container child references.
    pub nodes: Vec<UiNode>,
}

/// Atomic ordered patch batch for one mounted surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPatchBatch {
    /// Surface receiving the patches.
    pub surface_id: UiSurfaceId,
    /// Revision the batch expects to modify.
    pub base_revision: u64,
    /// Revision produced after the batch succeeds.
    pub next_revision: u64,
    /// Ordered patch operations applied atomically.
    pub patches: Vec<UiPatch>,
}

/// Minimal incremental operations supported by portable UI protocol v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UiPatch {
    /// Inserts a new node or replaces the data of an existing node.
    UpsertNode {
        /// Complete replacement node.
        node: UiNode,
    },
    /// Removes one node from the node table.
    RemoveNode {
        /// Node to remove.
        node_id: UiNodeId,
    },
    /// Inserts a child reference into a container.
    InsertChild {
        /// Parent container.
        parent: UiNodeId,
        /// Zero-based insertion position.
        index: u32,
        /// Existing child node to reference.
        child: UiNodeId,
    },
    /// Removes one child reference from a container.
    RemoveChild {
        /// Parent container.
        parent: UiNodeId,
        /// Child reference to remove.
        child: UiNodeId,
    },
    /// Moves one existing child within the same container.
    MoveChild {
        /// Parent container.
        parent: UiNodeId,
        /// Existing child to move.
        child: UiNodeId,
        /// New zero-based position after the child has been removed from its old position.
        index: u32,
    },
}
