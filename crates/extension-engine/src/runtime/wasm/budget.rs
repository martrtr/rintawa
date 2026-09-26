//! Host-owned resource budgets for sandboxed WASM component execution.

use wasmtime::{StoreLimits, StoreLimitsBuilder};

/// Host-owned resource limits for one WASM component instance.
///
/// Lifecycle, event, and task callbacks receive a fresh `fuel_per_callback`
/// allowance. Service callbacks and interactive Portable UI actions receive
/// dedicated larger allowances because they may legitimately process bounded
/// payloads while still remaining finite.
///
/// Memory and table limits are enforced by Wasmtime for the lifetime of the
/// component store. These defaults are a conservative local-host baseline, not a
/// public package ABI: a product supervisor may choose a stricter policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmExecutionBudget {
    /// Largest accepted compiled component artifact in bytes.
    pub max_component_bytes: usize,
    /// Maximum size of each guest linear memory in bytes.
    pub max_memory_bytes: usize,
    /// Maximum number of elements in each guest table.
    pub max_table_elements: usize,
    /// Maximum core instances allocated by one component store.
    pub max_instances: usize,
    /// Maximum tables allocated by one component store.
    pub max_tables: usize,
    /// Maximum linear memories allocated by one component store.
    pub max_memories: usize,
    /// Fuel made available before lifecycle, event, and task callbacks.
    pub fuel_per_callback: u64,
    /// Fuel made available before a bounded service request callback.
    pub fuel_per_service_request: u64,
    /// Fuel made available before an interactive Portable UI action.
    pub fuel_per_ui_action: u64,
    /// Maximum byte length of an inbound event topic or payload.
    pub max_host_message_bytes: usize,
    /// Maximum bytes returned by one bounded target-artifact resource read.
    pub max_artifact_read_bytes: usize,
    /// Maximum RTW bytes accepted by one generic artifact import call.
    pub max_artifact_import_bytes: usize,
    /// Maximum raw asset bytes accepted by one generic asset import call.
    pub max_asset_import_bytes: usize,
    /// Maximum deferred user-content writes accepted during one guest callback.
    pub max_user_content_writes_per_execution: usize,
    /// Maximum world-session mutations accepted during one guest callback.
    pub max_world_session_mutations_per_execution: usize,
    /// Maximum authoritative world commands accepted during one guest callback.
    pub max_world_command_submissions_per_execution: usize,
    /// Maximum policy-filtered World Projection reads accepted during one guest callback.
    pub max_world_projection_reads_per_execution: usize,
    /// Maximum cooperative background tasks owned by one WASM component.
    pub max_background_tasks: usize,
    /// Smallest periodic task interval accepted from a guest.
    pub min_background_task_interval_ms: u32,
    /// Maximum live loopback listener/stream handles owned by one component.
    pub max_network_handles: usize,
    /// Maximum payload accepted by one loopback read or write.
    pub max_network_io_bytes: usize,
    /// Maximum time a loopback connect host call may block.
    pub loopback_connect_timeout_ms: u64,
    /// Maximum payload returned by one bounded outbound HTTPS request.
    pub max_http_fetch_bytes: usize,
    /// Maximum total duration of one outbound HTTPS request.
    pub http_fetch_timeout_ms: u64,
    /// Maximum number of manually validated HTTPS redirects.
    pub max_http_redirects: usize,
}

impl Default for WasmExecutionBudget {
    fn default() -> Self {
        Self {
            max_component_bytes: 32 * 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            max_table_elements: 100_000,
            max_instances: 32,
            max_tables: 16,
            max_memories: 8,
            fuel_per_callback: 10_000_000,
            fuel_per_service_request: 50_000_000,
            fuel_per_ui_action: 50_000_000,
            max_host_message_bytes: 1024 * 1024,
            max_artifact_read_bytes: 8 * 1024 * 1024,
            max_artifact_import_bytes: 32 * 1024 * 1024,
            max_asset_import_bytes: 16 * 1024 * 1024,
            max_user_content_writes_per_execution: 4,
            max_world_session_mutations_per_execution: 8,
            max_world_command_submissions_per_execution: 8,
            max_world_projection_reads_per_execution: 8,
            max_background_tasks: 8,
            min_background_task_interval_ms: 10,
            max_network_handles: 64,
            max_network_io_bytes: 64 * 1024,
            loopback_connect_timeout_ms: 250,
            max_http_fetch_bytes: 32 * 1024 * 1024,
            http_fetch_timeout_ms: 15_000,
            max_http_redirects: 5,
        }
    }
}

impl WasmExecutionBudget {
    pub(super) fn store_limits(&self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.max_memory_bytes)
            .table_elements(self.max_table_elements)
            .instances(self.max_instances)
            .tables(self.max_tables)
            .memories(self.max_memories)
            .build()
    }
}
