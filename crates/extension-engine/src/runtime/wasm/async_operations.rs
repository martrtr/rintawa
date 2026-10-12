//! Bounded owner-scoped ephemeral operation transport.
//!
//! Guest callbacks use this store to exchange provisional progress and bytes.
//! Neither a stream chunk nor a completion automatically mutates World state.
//! Service handoff and durable retry identities are deliberately separate.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use rintawa_sdk::contracts::ComponentRef;
use uuid::Uuid;

use crate::runtime::wasm::{
    AsyncOperationData, AsyncOperationError, AsyncOperationEvent, AsyncOperationHost,
    AsyncOperationProgress, WasmHostState,
};

const MAX_OPERATIONS_PER_OWNER: usize = 8;
const MAX_OPERATIONS_PER_COMPONENT_STORE: usize = 64;
const MAX_PENDING_EVENTS: usize = 16;
const MAX_PENDING_BYTES: usize = 128 * 1024;
const MAX_CHUNK_BYTES: usize = 8 * 1024;
const MAX_RESULT_BYTES: usize = 8 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;
const MAX_IDLE_DURATION: Duration = Duration::from_secs(10 * 60);
const TERMINAL_RETENTION: Duration = Duration::from_secs(90);

enum Terminal {
    Completed(Vec<u8>),
    Failed(String),
    Cancelled,
}

pub(super) struct OwnedAsyncOperation {
    pub(super) owner: ComponentRef,
    queue: VecDeque<AsyncOperationEvent>,
    queued_bytes: usize,
    next_sequence: u64,
    last_progress: u8,
    last_activity: Instant,
    terminal: Option<Terminal>,
    terminal_since: Option<Instant>,
}

impl OwnedAsyncOperation {
    fn new(owner: ComponentRef) -> Self {
        Self {
            owner,
            queue: VecDeque::new(),
            queued_bytes: 0,
            next_sequence: 1,
            last_progress: 0,
            last_activity: Instant::now(),
            terminal: None,
            terminal_since: None,
        }
    }

    fn ensure_pending(&self) -> Result<(), AsyncOperationError> {
        if self.terminal.is_some() {
            return Err(AsyncOperationError::InvalidState);
        }
        Ok(())
    }

    fn reserve_event(&self, bytes: usize) -> Result<u64, AsyncOperationError> {
        self.ensure_pending()?;
        if self.queue.len() >= MAX_PENDING_EVENTS
            || self.queued_bytes.saturating_add(bytes) > MAX_PENDING_BYTES
        {
            return Err(AsyncOperationError::QueueFull);
        }
        if self.next_sequence == u64::MAX {
            return Err(AsyncOperationError::LimitExceeded);
        }
        Ok(self.next_sequence)
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    fn terminate(&mut self, terminal: Terminal) {
        self.terminal = Some(terminal);
        self.terminal_since = Some(Instant::now());
        self.touch();
    }

    fn expired(&mut self, now: Instant) -> bool {
        if let Some(since) = self.terminal_since {
            return now.duration_since(since) >= TERMINAL_RETENTION;
        }
        if now.duration_since(self.last_activity) >= MAX_IDLE_DURATION {
            self.terminate(Terminal::Failed(String::from("operation timed out")));
        }
        false
    }
}

impl WasmHostState {
    fn current_operation_owner(&self) -> Result<ComponentRef, AsyncOperationError> {
        if !self.operation_access_active {
            return Err(AsyncOperationError::AccessNotActive);
        }
        self.current_execution_owner()
            .cloned()
            .ok_or(AsyncOperationError::AccessNotActive)
    }

    fn owned_operation_mut(
        &mut self,
        handle: &str,
        owner: &ComponentRef,
    ) -> Result<&mut OwnedAsyncOperation, AsyncOperationError> {
        let operation = self
            .async_operations
            .get_mut(handle)
            .ok_or(AsyncOperationError::UnknownHandle)?;
        if operation.owner != *owner {
            return Err(AsyncOperationError::UnknownHandle);
        }
        Ok(operation)
    }

    fn prune_async_operations(&mut self) {
        let now = Instant::now();
        self.async_operations
            .retain(|_, operation| !operation.expired(now));
    }
}

impl AsyncOperationHost for WasmHostState {
    fn create(&mut self) -> Result<String, AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if self.async_operations.len() >= MAX_OPERATIONS_PER_COMPONENT_STORE
            || self
                .async_operations
                .values()
                .filter(|operation| operation.owner == owner)
                .count()
                >= MAX_OPERATIONS_PER_OWNER
        {
            return Err(AsyncOperationError::LimitExceeded);
        }
        let handle = loop {
            let candidate = Uuid::now_v7().to_string();
            if !self.async_operations.contains_key(&candidate) {
                break candidate;
            }
        };
        self.async_operations
            .insert(handle.clone(), OwnedAsyncOperation::new(owner));
        Ok(handle)
    }

    fn push_progress(&mut self, handle: String, percent: u8) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation_mut(&handle, &owner)?;
        if percent > 100 || percent < operation.last_progress {
            return Err(AsyncOperationError::InvalidProgress);
        }
        let sequence = operation.reserve_event(0)?;
        operation.next_sequence += 1;
        operation.last_progress = percent;
        operation
            .queue
            .push_back(AsyncOperationEvent::Progress(AsyncOperationProgress {
                sequence,
                percent,
            }));
        operation.touch();
        Ok(())
    }

    fn push_data(&mut self, handle: String, data: Vec<u8>) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if data.len() > MAX_CHUNK_BYTES {
            return Err(AsyncOperationError::MessageTooLarge);
        }
        if data.is_empty() {
            return Err(AsyncOperationError::InvalidState);
        }
        let operation = self.owned_operation_mut(&handle, &owner)?;
        let sequence = operation.reserve_event(data.len())?;
        operation.queued_bytes += data.len();
        operation.next_sequence += 1;
        operation
            .queue
            .push_back(AsyncOperationEvent::Data(AsyncOperationData {
                sequence,
                bytes: data,
            }));
        operation.touch();
        Ok(())
    }

    fn complete(&mut self, handle: String, result: Vec<u8>) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if result.len() > MAX_RESULT_BYTES {
            return Err(AsyncOperationError::MessageTooLarge);
        }
        let operation = self.owned_operation_mut(&handle, &owner)?;
        operation.ensure_pending()?;
        operation.terminate(Terminal::Completed(result));
        Ok(())
    }

    fn fail(&mut self, handle: String, reason: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if reason.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(AsyncOperationError::MessageTooLarge);
        }
        let operation = self.owned_operation_mut(&handle, &owner)?;
        operation.ensure_pending()?;
        operation.terminate(Terminal::Failed(reason));
        Ok(())
    }

    fn cancel(&mut self, handle: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation_mut(&handle, &owner)?;
        if matches!(operation.terminal, Some(Terminal::Cancelled)) {
            return Ok(());
        }
        operation.ensure_pending()?;
        operation.queue.clear();
        operation.queued_bytes = 0;
        operation.terminate(Terminal::Cancelled);
        Ok(())
    }

    fn poll(&mut self, handle: String) -> Result<AsyncOperationEvent, AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation_mut(&handle, &owner)?;
        if matches!(operation.terminal, Some(Terminal::Cancelled)) {
            return Ok(AsyncOperationEvent::Cancelled);
        }
        if let Some(event) = operation.queue.pop_front() {
            if let AsyncOperationEvent::Data(chunk) = &event {
                operation.queued_bytes -= chunk.bytes.len();
            }
            return Ok(event);
        }
        Ok(match &operation.terminal {
            Some(Terminal::Completed(bytes)) => AsyncOperationEvent::Completed(bytes.clone()),
            Some(Terminal::Failed(reason)) => AsyncOperationEvent::Failed(reason.clone()),
            Some(Terminal::Cancelled) => AsyncOperationEvent::Cancelled,
            None => AsyncOperationEvent::Pending,
        })
    }

    fn close(&mut self, handle: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self
            .async_operations
            .get(&handle)
            .ok_or(AsyncOperationError::UnknownHandle)?;
        if operation.owner != owner {
            return Err(AsyncOperationError::UnknownHandle);
        }
        self.async_operations.remove(&handle);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rintawa_sdk::{contracts::ComponentRef, types::ComponentId};

    use super::*;

    fn state() -> WasmHostState {
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        state.operation_access_active = true;
        state.execution_owner = Some(ComponentRef::new("first", "runtime"));
        state
    }

    #[test]
    fn operation_requires_active_authenticated_execution() {
        let mut state = state();
        state.operation_access_active = false;
        assert!(matches!(
            AsyncOperationHost::create(&mut state),
            Err(AsyncOperationError::AccessNotActive)
        ));
        state.operation_access_active = true;
        state.execution_owner = None;
        assert!(matches!(
            AsyncOperationHost::create(&mut state),
            Err(AsyncOperationError::AccessNotActive)
        ));
    }

    #[test]
    fn ordered_progress_chunks_and_explicit_completion_are_stable() {
        let mut state = state();
        let handle = AsyncOperationHost::create(&mut state).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Pending)
        ));
        AsyncOperationHost::push_progress(&mut state, handle.clone(), 10).unwrap();
        AsyncOperationHost::push_data(&mut state, handle.clone(), b"hello".to_vec()).unwrap();
        AsyncOperationHost::push_progress(&mut state, handle.clone(), 100).unwrap();
        AsyncOperationHost::complete(&mut state, handle.clone(), b"final".to_vec()).unwrap();

        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Progress(AsyncOperationProgress {
                sequence: 1,
                percent: 10
            }))
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Data(AsyncOperationData { sequence: 2, bytes }))
                if bytes == b"hello"
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Progress(AsyncOperationProgress {
                sequence: 3,
                percent: 100
            }))
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Completed(bytes)) if bytes == b"final"
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Completed(bytes)) if bytes == b"final"
        ));
        assert!(matches!(
            AsyncOperationHost::push_data(&mut state, handle.clone(), b"late".to_vec()),
            Err(AsyncOperationError::InvalidState)
        ));
        AsyncOperationHost::close(&mut state, handle.clone()).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle),
            Err(AsyncOperationError::UnknownHandle)
        ));
    }

    #[test]
    fn operations_are_isolated_between_exact_component_principals() {
        let mut state = state();
        let first = AsyncOperationHost::create(&mut state).unwrap();
        state.execution_owner = Some(ComponentRef::new("other", "runtime"));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, first.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::cancel(&mut state, first.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::push_progress(&mut state, first.clone(), 10),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::close(&mut state, first.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        let second = AsyncOperationHost::create(&mut state).unwrap();
        assert_ne!(first, second);
        state.execution_owner = Some(ComponentRef::new("first", "runtime"));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, first),
            Ok(AsyncOperationEvent::Pending)
        ));
    }

    #[test]
    fn queue_backpressure_is_explicit_and_preserves_sequence() {
        let mut state = state();
        let handle = AsyncOperationHost::create(&mut state).unwrap();
        for _ in 0..MAX_PENDING_EVENTS {
            AsyncOperationHost::push_data(&mut state, handle.clone(), vec![42; MAX_CHUNK_BYTES])
                .unwrap();
        }
        assert!(matches!(
            AsyncOperationHost::push_data(&mut state, handle.clone(), vec![1]),
            Err(AsyncOperationError::QueueFull)
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Data(AsyncOperationData {
                sequence: 1,
                ..
            }))
        ));
        AsyncOperationHost::push_data(&mut state, handle.clone(), vec![1]).unwrap();
        for sequence in 2..=17 {
            assert!(matches!(
                AsyncOperationHost::poll(&mut state, handle.clone()),
                Ok(AsyncOperationEvent::Data(AsyncOperationData { sequence: observed, .. }))
                    if observed == sequence
            ));
        }
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle),
            Ok(AsyncOperationEvent::Pending)
        ));
    }

    #[test]
    fn cancellation_discards_provisional_data_and_refuses_late_completion() {
        let mut state = state();
        let handle = AsyncOperationHost::create(&mut state).unwrap();
        AsyncOperationHost::push_data(&mut state, handle.clone(), b"secret".to_vec()).unwrap();
        AsyncOperationHost::cancel(&mut state, handle.clone()).unwrap();
        AsyncOperationHost::cancel(&mut state, handle.clone()).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Cancelled)
        ));
        assert!(matches!(
            AsyncOperationHost::complete(&mut state, handle.clone(), vec![]),
            Err(AsyncOperationError::InvalidState)
        ));
        assert_eq!(state.async_operations.get(&handle).unwrap().queued_bytes, 0);
    }

    #[test]
    fn terminal_errors_and_validation_do_not_fake_success() {
        let mut state = state();
        let handle = AsyncOperationHost::create(&mut state).unwrap();
        assert!(matches!(
            AsyncOperationHost::push_data(&mut state, handle.clone(), vec![0; MAX_CHUNK_BYTES + 1]),
            Err(AsyncOperationError::MessageTooLarge)
        ));
        assert!(matches!(
            AsyncOperationHost::push_progress(&mut state, handle.clone(), 101),
            Err(AsyncOperationError::InvalidProgress)
        ));
        AsyncOperationHost::push_progress(&mut state, handle.clone(), 70).unwrap();
        assert!(matches!(
            AsyncOperationHost::push_progress(&mut state, handle.clone(), 20),
            Err(AsyncOperationError::InvalidProgress)
        ));
        assert!(matches!(
            AsyncOperationHost::complete(&mut state, handle.clone(), vec![0; MAX_RESULT_BYTES + 1]),
            Err(AsyncOperationError::MessageTooLarge)
        ));
        AsyncOperationHost::fail(&mut state, handle.clone(), String::from("network failed"))
            .unwrap();
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle.clone()),
            Ok(AsyncOperationEvent::Progress(_))
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle),
            Ok(AsyncOperationEvent::Failed(reason)) if reason == "network failed"
        ));
    }

    #[test]
    fn operation_capacity_is_per_owner_and_released_after_close() {
        let mut state = state();
        let handles: Vec<_> = (0..MAX_OPERATIONS_PER_OWNER)
            .map(|_| AsyncOperationHost::create(&mut state).unwrap())
            .collect();
        assert!(matches!(
            AsyncOperationHost::create(&mut state),
            Err(AsyncOperationError::LimitExceeded)
        ));
        state.execution_owner = Some(ComponentRef::new("other", "runtime"));
        assert!(AsyncOperationHost::create(&mut state).is_ok());
        state.execution_owner = Some(ComponentRef::new("first", "runtime"));
        AsyncOperationHost::close(&mut state, handles[0].clone()).unwrap();
        assert!(AsyncOperationHost::create(&mut state).is_ok());
    }

    #[test]
    fn expired_operations_and_stopped_owners_are_revoked() {
        let mut state = state();
        let expired = AsyncOperationHost::create(&mut state).unwrap();
        AsyncOperationHost::complete(&mut state, expired.clone(), vec![]).unwrap();
        state
            .async_operations
            .get_mut(&expired)
            .unwrap()
            .terminal_since = Some(Instant::now() - TERMINAL_RETENTION - Duration::from_secs(1));
        let other = AsyncOperationHost::create(&mut state).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, expired),
            Err(AsyncOperationError::UnknownHandle)
        ));
        state.revoke_runtime_resources_for_owner(&ComponentRef::new("first", "runtime"));
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, other),
            Err(AsyncOperationError::UnknownHandle)
        ));
    }

    #[test]
    fn stalled_operation_becomes_terminal_failure() {
        let mut state = state();
        let handle = AsyncOperationHost::create(&mut state).unwrap();
        state
            .async_operations
            .get_mut(&handle)
            .unwrap()
            .last_activity = Instant::now() - MAX_IDLE_DURATION - Duration::from_secs(1);
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle),
            Ok(AsyncOperationEvent::Failed(reason)) if reason == "operation timed out"
        ));
    }
}
