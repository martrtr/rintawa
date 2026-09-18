//! Portable UI runtime errors exposed across host boundaries.

use thiserror::Error;

/// Failure while registering, mounting, patching, or dispatching portable UI.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UiError {
    /// An extension runtime instance has already registered UI metadata.
    #[error("extension instance `{0}` is already registered with the UI Runtime")]
    InstanceAlreadyRegistered(String),
    /// An extension runtime instance has no UI Runtime registration.
    #[error("extension instance `{0}` is not registered with the UI Runtime")]
    InstanceNotRegistered(String),
    /// The UI Runtime cannot access its internal state.
    #[error("UI Runtime is unavailable")]
    RuntimeUnavailable,
    /// No UI Layer is currently attached.
    #[error("UI Layer is unavailable")]
    LayerUnavailable,
    /// The host-owned semantic action queue reached its bounded capacity.
    #[error("UI action queue is full")]
    ActionQueueFull,
    /// A component registered more than one UI Layer descriptor.
    #[error("UI Layer descriptor is already registered for this component")]
    LayerAlreadyRegistered,
    /// The selected component did not register a UI Layer descriptor.
    #[error("component did not register a UI Layer descriptor")]
    LayerNotRegistered,
    /// Another UI Layer is already attached.
    #[error("a different UI Layer is already attached")]
    LayerAlreadyAttached,
    /// The caller is not the active UI Layer.
    #[error("caller is not the active UI Layer")]
    LayerNotOwner,
    /// The active layer cannot access a surface in another runtime scope.
    #[error("UI target runtime scope is not visible to the active layer")]
    ScopeNotVisible,
    /// The UI Layer uses an unsupported protocol major version.
    #[error("unsupported portable UI protocol version: expected {expected}, got {actual}")]
    UnsupportedProtocol {
        /// Protocol version implemented by the runtime.
        expected: u32,
        /// Protocol version declared by the layer.
        actual: u32,
    },
    /// The active UI Layer cannot render one required capability.
    #[error("unsupported UI capability `{0}`")]
    UnsupportedCapability(String),
    /// A surface identifier is already registered by another contribution.
    #[error("UI surface `{0}` is already registered")]
    SurfaceAlreadyRegistered(String),
    /// The requested surface has no static contribution.
    #[error("UI surface `{0}` is not registered")]
    SurfaceNotRegistered(String),
    /// A component attempted to mutate a surface owned by another component.
    #[error("UI surface `{0}` is owned by another component")]
    SurfaceNotOwned(String),
    /// The requested surface has no current presentation snapshot.
    #[error("UI surface `{0}` is not mounted")]
    SurfaceNotMounted(String),
    /// The requested surface already has a current presentation snapshot.
    #[error("UI surface `{0}` is already mounted")]
    SurfaceAlreadyMounted(String),
    /// A patch or action references a stale surface revision.
    #[error("UI surface revision mismatch: expected {expected}, got {actual}")]
    RevisionMismatch {
        /// Current runtime revision.
        expected: u64,
        /// Revision supplied by the caller.
        actual: u64,
    },
    /// A patch batch does not advance by exactly one revision.
    #[error("invalid UI revision transition {base} -> {next}")]
    InvalidRevision {
        /// Base revision supplied by the batch.
        base: u64,
        /// Next revision supplied by the batch.
        next: u64,
    },
    /// A surface snapshot exceeds the runtime node limit.
    #[error("UI surface contains {actual} nodes, exceeding the {maximum}-node limit")]
    SurfaceTooLarge {
        /// Number of supplied nodes.
        actual: usize,
        /// Maximum accepted nodes.
        maximum: usize,
    },
    /// A patch batch exceeds the runtime operation limit.
    #[error("UI patch batch contains {actual} operations, exceeding the {maximum}-operation limit")]
    PatchBatchTooLarge {
        /// Number of supplied patch operations.
        actual: usize,
        /// Maximum accepted patch operations.
        maximum: usize,
    },
    /// A snapshot contains the same node identifier more than once.
    #[error("duplicate UI node `{0}`")]
    DuplicateNode(String),
    /// A referenced node does not exist.
    #[error("UI node `{0}` does not exist")]
    NodeNotFound(String),
    /// The declared root node does not exist.
    #[error("UI root node `{0}` does not exist")]
    RootNotFound(String),
    /// A non-container node was used as a parent.
    #[error("UI node `{0}` cannot contain children")]
    NodeNotContainer(String),
    /// A child index is outside the current container bounds.
    #[error("UI child index {index} is invalid for parent `{parent}`")]
    InvalidChildIndex {
        /// Parent node identifier.
        parent: String,
        /// Invalid zero-based index.
        index: u32,
    },
    /// A parent does not currently reference the requested child.
    #[error("UI parent `{parent}` does not contain child `{child}`")]
    ChildNotFound {
        /// Parent node identifier.
        parent: String,
        /// Missing child node identifier.
        child: String,
    },
    /// The declared root is referenced as a child.
    #[error("UI root node `{0}` cannot have a parent")]
    RootHasParent(String),
    /// A node is referenced by more than one parent.
    #[error("UI node `{0}` has multiple parents")]
    MultipleParents(String),
    /// A node cannot be reached from the declared root.
    #[error("UI node `{0}` is orphaned from the surface root")]
    OrphanNode(String),
    /// The UI tree contains a cycle.
    #[error("UI tree contains a cycle at node `{0}`")]
    CycleDetected(String),
    /// The layer emitted an action not bound to the source node.
    #[error("UI action `{action}` is not bound to node `{node}` on surface `{surface}`")]
    ActionNotBound {
        /// Surface identifier.
        surface: String,
        /// Source node identifier.
        node: String,
        /// Unrecognized action identifier.
        action: String,
    },
    /// The source control is currently disabled.
    #[error("UI action `{action}` on node `{node}` is disabled")]
    ActionDisabled {
        /// Source node identifier.
        node: String,
        /// Disabled action identifier.
        action: String,
    },
    /// The supplied action payload does not match the source control.
    #[error("invalid payload for UI action `{action}` on node `{node}`")]
    InvalidActionPayload {
        /// Source node identifier.
        node: String,
        /// Action identifier.
        action: String,
    },
    /// The owner component is not active for action dispatch.
    #[error("UI surface owner is not active")]
    OwnerInactive,
}

/// Result returned by portable UI runtime operations.
pub type UiResult<T> = Result<T, UiError>;
