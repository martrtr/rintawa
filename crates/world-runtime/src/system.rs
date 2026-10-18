//! Command System contract used by the world runtime.

use rintawa_sdk::world::SchemaKey;
use rintawa_world::{WorldCommand, WorldTransaction};

use crate::{SystemResult, WorldSnapshot};

/// Deterministic command evaluator for one exact versioned command schema.
///
/// Systems read a fixed snapshot and return proposed authoritative mutations,
/// semantic events, and durable effects. They must not perform irreversible
/// external side effects during evaluation; those belong in durable effect jobs.
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
