//! Host-owned resource budgets for sandboxed WASM component execution.

use wasmtime::{StoreLimits, StoreLimitsBuilder};

/// Host-owned resource limits for one WASM component instance.
///
/// Lifecycle and event callbacks receive a fresh `fuel_per_callback` allowance.
/// Cooperative runtime tasks and service callbacks receive dedicated larger
/// allowances while still remaining finite.
///
/// Memory and table limits are enforced by Wasmtime for the lifetime of the
/// component store. These defaults are a conservative local-host baseline, not a
/// public package ABI: a product supervisor may choose a stricter policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmExecutionBudget {
    /// Largest accepted compiled component artifact in bytes.
    pub max_component_bytes: usize,
    /// Combined maximum size of all memories in a component lifecycle/System pair.
    pub max_memory_bytes: usize,
    /// Maximum combined linear memory across this runtime engine and its clones.
    pub max_total_memory_bytes: usize,
    /// Maximum number of elements in each guest table.
    pub max_table_elements: usize,
    /// Maximum core instances allocated by one component store.
    pub max_instances: usize,
    /// Maximum tables allocated by one component store.
    pub max_tables: usize,
    /// Maximum linear memories allocated by one component store.
    pub max_memories: usize,
    /// Fuel made available before lifecycle and event callbacks.
    pub fuel_per_callback: u64,
    /// Fuel made available before a cooperative runtime task callback.
    pub fuel_per_runtime_task: u64,
    /// Fuel made available before a bounded service request callback.
    pub fuel_per_service_request: u64,
    /// Maximum byte length of an inbound event topic or payload.
    pub max_host_message_bytes: usize,
    /// Maximum byte length of one bounded service request or response.
    pub max_service_message_bytes: usize,
    /// Maximum bytes returned by one bounded target-artifact resource read.
    pub max_artifact_read_bytes: usize,
    /// Maximum RTW bytes accepted by one generic artifact import call.
    pub max_artifact_import_bytes: usize,
    /// Maximum raw asset bytes accepted by one generic asset import call.
    pub max_asset_import_bytes: usize,
    /// Maximum world-session mutations accepted during one guest callback.
    pub max_world_session_mutations_per_execution: usize,
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
    /// Maximum concurrent HTTP request handles for one component owner.
    pub max_http_request_handles: usize,
    /// Largest outbound body accepted by one request.
    pub max_http_request_body_bytes: usize,
    /// Maximum header count on one request or streamed response.
    pub max_http_request_headers: usize,
    /// Maximum aggregate header bytes on one request or streamed response.
    pub max_http_request_header_bytes: usize,
    /// Largest chunk returned by one nonblocking HTTP poll.
    pub max_http_request_chunk_bytes: usize,
}

impl Default for WasmExecutionBudget {
    fn default() -> Self {
        Self {
            max_component_bytes: 32 * 1024 * 1024,
            max_memory_bytes: 64 * 1024 * 1024,
            max_total_memory_bytes: 512 * 1024 * 1024,
            max_table_elements: 100_000,
            max_instances: 32,
            max_tables: 16,
            max_memories: 8,
            fuel_per_callback: 10_000_000,
            fuel_per_runtime_task: 250_000_000,
            fuel_per_service_request: 500_000_000,
            max_host_message_bytes: 1024 * 1024,
            max_service_message_bytes: 8 * 1024 * 1024,
            max_artifact_read_bytes: 8 * 1024 * 1024,
            max_artifact_import_bytes: 32 * 1024 * 1024,
            max_asset_import_bytes: 32 * 1024 * 1024,
            max_world_session_mutations_per_execution: 8,
            max_background_tasks: 8,
            min_background_task_interval_ms: 10,
            max_network_handles: 64,
            max_network_io_bytes: 64 * 1024,
            loopback_connect_timeout_ms: 250,
            max_http_fetch_bytes: 32 * 1024 * 1024,
            http_fetch_timeout_ms: 15_000,
            max_http_redirects: 5,
            max_http_request_handles: 8,
            max_http_request_body_bytes: 1024 * 1024,
            max_http_request_headers: 32,
            max_http_request_header_bytes: 8 * 1024,
            max_http_request_chunk_bytes: 16 * 1024,
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

/// Shared linear-memory accounting; independent runtimes never share a counter.
#[derive(Debug)]
pub(crate) struct MemoryBudget {
    used: std::sync::atomic::AtomicUsize,
    maximum: usize,
}
impl MemoryBudget {
    pub(crate) fn new(maximum: usize) -> Self {
        Self {
            used: std::sync::atomic::AtomicUsize::new(0),
            maximum,
        }
    }
    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .try_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |used| used.checked_add(bytes).filter(|next| *next <= self.maximum),
            )
            .is_ok()
    }
    fn release(&self, bytes: usize) {
        self.used
            .fetch_sub(bytes, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(super) struct LinearMemoryLimiter {
    limits: StoreLimits,
    pair: std::sync::Arc<MemoryBudget>,
    total: std::sync::Arc<MemoryBudget>,
    allocated: usize,
    pending_growth: usize,
}
impl LinearMemoryLimiter {
    pub(super) fn new(
        budget: &WasmExecutionBudget,
        pair: std::sync::Arc<MemoryBudget>,
        total: std::sync::Arc<MemoryBudget>,
    ) -> Self {
        Self {
            limits: budget.store_limits(),
            pair,
            total,
            allocated: 0,
            pending_growth: 0,
        }
    }
}
impl wasmtime::ResourceLimiter for LinearMemoryLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.pending_growth = 0;
        if !self.limits.memory_growing(current, desired, maximum)? {
            return Ok(false);
        }
        let bytes = desired - current;
        if !self.pair.reserve(bytes) {
            anyhow::bail!("component pair linear-memory budget exhausted");
        }
        if !self.total.reserve(bytes) {
            self.pair.release(bytes);
            anyhow::bail!("engine aggregate linear-memory budget exhausted");
        }
        self.allocated += bytes;
        self.pending_growth = bytes;
        Ok(true)
    }
    fn memory_grow_failed(&mut self, error: anyhow::Error) -> anyhow::Result<()> {
        self.pair.release(self.pending_growth);
        self.total.release(self.pending_growth);
        self.allocated -= self.pending_growth;
        self.pending_growth = 0;
        self.limits.memory_grow_failed(error)
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }
    fn table_grow_failed(&mut self, error: anyhow::Error) -> anyhow::Result<()> {
        self.limits.table_grow_failed(error)
    }
    fn instances(&self) -> usize {
        self.limits.instances()
    }
    fn tables(&self) -> usize {
        self.limits.tables()
    }
    fn memories(&self) -> usize {
        self.limits.memories()
    }
}
impl Drop for LinearMemoryLimiter {
    fn drop(&mut self) {
        self.pair.release(self.allocated);
        self.total.release(self.allocated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wasmtime::{Engine, Memory, MemoryType, Store};
    fn store(
        engine: &Engine,
        pair: Arc<MemoryBudget>,
        total: Arc<MemoryBudget>,
    ) -> Store<LinearMemoryLimiter> {
        let mut store = Store::new(
            engine,
            LinearMemoryLimiter::new(&WasmExecutionBudget::default(), pair, total),
        );
        store.limiter(|limiter| limiter);
        store
    }
    #[test]
    fn test_combined_memories_pair_and_aggregate_limits_release_on_drop() -> anyhow::Result<()> {
        let engine = Engine::default();
        let pair = Arc::new(MemoryBudget::new(4 * 65536));
        let total = Arc::new(MemoryBudget::new(6 * 65536));
        let mut lifecycle = store(&engine, pair.clone(), total.clone());
        Memory::new(&mut lifecycle, MemoryType::new(2, None))?;
        Memory::new(&mut lifecycle, MemoryType::new(1, None))?;
        let mut system = store(&engine, pair.clone(), total.clone());
        Memory::new(&mut system, MemoryType::new(1, None))?;
        assert!(Memory::new(&mut system, MemoryType::new(1, None)).is_err());
        let mut other = store(
            &engine,
            Arc::new(MemoryBudget::new(4 * 65536)),
            total.clone(),
        );
        Memory::new(&mut other, MemoryType::new(2, None))?;
        assert!(Memory::new(&mut other, MemoryType::new(1, None)).is_err());
        drop(system);
        Memory::new(&mut other, MemoryType::new(1, None))?;
        drop(lifecycle);
        drop(other);
        assert_eq!(total.used.load(std::sync::atomic::Ordering::Acquire), 0);
        assert_eq!(pair.used.load(std::sync::atomic::Ordering::Acquire), 0);
        Ok(())
    }
    #[test]
    fn test_failed_native_memory_growth_rolls_back_reservation() -> anyhow::Result<()> {
        let engine = Engine::default();
        let pair = Arc::new(MemoryBudget::new(4 * 65536));
        let total = Arc::new(MemoryBudget::new(4 * 65536));
        let mut store = store(&engine, pair.clone(), total.clone());
        let memory = Memory::new(&mut store, MemoryType::new(1, Some(1)))?;
        assert!(memory.grow(&mut store, 1).is_err());
        assert_eq!(pair.used.load(std::sync::atomic::Ordering::Acquire), 65536);
        drop(store);
        assert_eq!(total.used.load(std::sync::atomic::Ordering::Acquire), 0);
        Ok(())
    }
}
