//! Command System contract used by the world runtime.

use rintawa_sdk::world::SchemaKey;
use rintawa_world::{WorldCommand, WorldTransaction};

use crate::{SystemResult, WorldSnapshot};

/// Deterministic command evaluator for one exact versioned command schema.
///
/// Systems read a pinned, property-owner-scoped snapshot and return one atomic
/// item/property change set. The owner is taken from the registered command
/// schema, not from the command Principal or a guest-supplied payload. They must
/// not perform irreversible external side effects during evaluation:
/// the runtime validates the change set and commits it as one SQLite transaction.
/// Any external work belongs outside authoritative command evaluation.
pub trait WorldSystem: Send + Sync + 'static {
    /// Returns the exact command schema owned by this System.
    fn command_schema(&self) -> &SchemaKey;

    /// Evaluates one command against a fixed snapshot.
    ///
    /// # Errors
    ///
    /// Returns a domain rejection, read failure, or System execution failure.
    fn evaluate(
        &self,
        snapshot: &WorldSnapshot,
        command: &WorldCommand,
    ) -> SystemResult<WorldTransaction>;
}
