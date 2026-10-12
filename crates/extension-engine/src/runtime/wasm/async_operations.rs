//! Bounded, ephemeral and owner-scoped operation transport.
//!
//! A producer owns mutation and cancellation. The authenticated caller of one
//! routed service request may receive one bounded, read-only consumption lease.
//! No stream event is a durable WorldEvent or a committed WorldCommand.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
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
const MAX_OBSERVER_LEASES: usize = 256;
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
    delegated_reader: Option<ComponentRef>,
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
            delegated_reader: None,
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

    fn read_next(&mut self) -> AsyncOperationEvent {
        if matches!(self.terminal, Some(Terminal::Cancelled)) {
            return AsyncOperationEvent::Cancelled;
        }
        if let Some(event) = self.queue.pop_front() {
            if let AsyncOperationEvent::Data(chunk) = &event {
                self.queued_bytes -= chunk.bytes.len();
            }
            return event;
        }
        match &self.terminal {
            Some(Terminal::Completed(bytes)) => AsyncOperationEvent::Completed(bytes.clone()),
            Some(Terminal::Failed(reason)) => AsyncOperationEvent::Failed(reason.clone()),
            Some(Terminal::Cancelled) => AsyncOperationEvent::Cancelled,
            None => AsyncOperationEvent::Pending,
        }
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

/// A lease never retains the producer's operation after the producer stops.
struct ObserverLease {
    reader: ComponentRef,
    owner: ComponentRef,
    operation: Weak<Mutex<OwnedAsyncOperation>>,
}

#[derive(Default)]
pub(crate) struct AsyncObserverRegistry {
    leases: Mutex<HashMap<String, ObserverLease>>,
}

impl AsyncObserverRegistry {
    /// Revokes both writer-owned streams and read-only leases of a stopped reader.
    /// Release the registry lock before touching individual streams.
    pub(super) fn revoke_principal(&self, principal: &ComponentRef) {
        let mut readers_to_detach = Vec::new();
        if let Ok(mut leases) = self.leases.lock() {
            leases.retain(|_, lease| {
                if &lease.owner == principal {
                    return false;
                }
                if &lease.reader == principal {
                    readers_to_detach.push((lease.reader.clone(), lease.operation.clone()));
                    return false;
                }
                true
            });
        }
        for (reader, weak) in readers_to_detach {
            if let Some(operation) = weak.upgrade()
                && let Ok(mut locked) = operation.lock()
                && locked.delegated_reader.as_ref() == Some(&reader)
            {
                locked.delegated_reader = None;
            }
        }
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

    fn owned_operation(
        &self,
        handle: &str,
        owner: &ComponentRef,
    ) -> Result<Arc<Mutex<OwnedAsyncOperation>>, AsyncOperationError> {
        let operation = self
            .async_operations
            .get(handle)
            .ok_or(AsyncOperationError::UnknownHandle)?
            .clone();
        {
            let locked = operation
                .lock()
                .map_err(|_| AsyncOperationError::Unavailable)?;
            if locked.owner != *owner {
                return Err(AsyncOperationError::UnknownHandle);
            }
        }
        Ok(operation)
    }

    fn prune_async_operations(&mut self) {
        let now = Instant::now();
        self.async_operations.retain(|_, operation| {
            operation
                .lock()
                .is_ok_and(|mut locked| !locked.expired(now))
        });
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
                .filter(|operation| operation.lock().is_ok_and(|locked| locked.owner == owner))
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
        self.async_operations.insert(
            handle.clone(),
            Arc::new(Mutex::new(OwnedAsyncOperation::new(owner))),
        );
        Ok(handle)
    }

    fn push_progress(&mut self, handle: String, percent: u8) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
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
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
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

    fn complete(&mut self, handle: String, output: Vec<u8>) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if output.len() > MAX_RESULT_BYTES {
            return Err(AsyncOperationError::MessageTooLarge);
        }
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
        operation.ensure_pending()?;
        operation.terminate(Terminal::Completed(output));
        Ok(())
    }

    fn fail(&mut self, handle: String, reason: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        if reason.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(AsyncOperationError::MessageTooLarge);
        }
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
        operation.ensure_pending()?;
        operation.terminate(Terminal::Failed(reason));
        Ok(())
    }

    fn cancel(&mut self, handle: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
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
        let operation = self.owned_operation(&handle, &owner)?;
        let mut operation = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
        if operation.delegated_reader.is_some() {
            return Err(AsyncOperationError::InvalidState);
        }
        Ok(operation.read_next())
    }

    fn share_with_caller(&mut self, handle: String) -> Result<String, AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        let reader = self
            .service_caller
            .clone()
            .ok_or(AsyncOperationError::InvalidState)?;
        if reader == owner {
            return Err(AsyncOperationError::InvalidState);
        }
        // A guest-supplied instance ID cannot bypass the service runtime's scope.
        let producer_scope = self
            .services
            .component_scope_id(&owner)
            .ok_or(AsyncOperationError::Unavailable)?;
        if self.services.component_scope_id(&reader) != Some(producer_scope) {
            return Err(AsyncOperationError::InvalidState);
        }
        self.prune_async_operations();
        let operation = self.owned_operation(&handle, &owner)?;
        let mut locked = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
        if locked.delegated_reader.is_some() {
            return Err(AsyncOperationError::InvalidState);
        }
        let token = {
            let mut leases = self
                .async_observers
                .leases
                .lock()
                .map_err(|_| AsyncOperationError::Unavailable)?;
            leases.retain(|_, lease| lease.operation.strong_count() > 0);
            if leases.len() >= MAX_OBSERVER_LEASES {
                return Err(AsyncOperationError::LimitExceeded);
            }
            let token = loop {
                let token = Uuid::now_v7().to_string();
                if !leases.contains_key(&token) {
                    break token;
                }
            };
            leases.insert(
                token.clone(),
                ObserverLease {
                    reader: reader.clone(),
                    owner,
                    operation: Arc::downgrade(&operation),
                },
            );
            token
        };
        locked.delegated_reader = Some(reader);
        Ok(token)
    }

    fn poll_observer(&mut self, token: String) -> Result<AsyncOperationEvent, AsyncOperationError> {
        let reader = self.current_operation_owner()?;
        let source = {
            let leases = self
                .async_observers
                .leases
                .lock()
                .map_err(|_| AsyncOperationError::Unavailable)?;
            let lease = leases
                .get(&token)
                .ok_or(AsyncOperationError::UnknownHandle)?;
            if lease.reader != reader {
                return Err(AsyncOperationError::UnknownHandle);
            }
            lease.operation.clone()
        };
        let operation = source.upgrade().ok_or(AsyncOperationError::UnknownHandle)?;
        let mut locked = operation
            .lock()
            .map_err(|_| AsyncOperationError::Unavailable)?;
        if locked.delegated_reader.as_ref() != Some(&reader) || locked.expired(Instant::now()) {
            return Err(AsyncOperationError::UnknownHandle);
        }
        Ok(locked.read_next())
    }

    fn close_observer(&mut self, token: String) -> Result<(), AsyncOperationError> {
        let reader = self.current_operation_owner()?;
        let source = {
            let mut leases = self
                .async_observers
                .leases
                .lock()
                .map_err(|_| AsyncOperationError::Unavailable)?;
            let lease = leases
                .get(&token)
                .ok_or(AsyncOperationError::UnknownHandle)?;
            if lease.reader != reader {
                return Err(AsyncOperationError::UnknownHandle);
            }
            let source = lease.operation.clone();
            leases.remove(&token);
            source
        };
        if let Some(operation) = source.upgrade() {
            let mut locked = operation
                .lock()
                .map_err(|_| AsyncOperationError::Unavailable)?;
            if locked.delegated_reader.as_ref() == Some(&reader) {
                locked.delegated_reader = None;
            }
        }
        Ok(())
    }

    fn close(&mut self, handle: String) -> Result<(), AsyncOperationError> {
        let owner = self.current_operation_owner()?;
        self.prune_async_operations();
        let operation = self.owned_operation(&handle, &owner)?;
        drop(operation);
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
        assert_eq!(
            state
                .async_operations
                .get(&handle)
                .unwrap()
                .lock()
                .unwrap()
                .queued_bytes,
            0
        );
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
            .get(&expired)
            .unwrap()
            .lock()
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
            .get(&handle)
            .unwrap()
            .lock()
            .unwrap()
            .last_activity = Instant::now() - MAX_IDLE_DURATION - Duration::from_secs(1);
        assert!(matches!(
            AsyncOperationHost::poll(&mut state, handle),
            Ok(AsyncOperationEvent::Failed(reason)) if reason == "operation timed out"
        ));
    }

    fn shared_states() -> (WasmHostState, WasmHostState, WasmHostState) {
        use crate::runtime::wasm::{WasmExecutionBudget, WasmHostServices};
        use crate::secrets::SecretManager;
        use crate::services::ServiceInstanceRegistration;
        use rintawa_sdk::types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId};

        let host = WasmHostServices::standalone(SecretManager::system());
        for name in ["provider", "client", "intruder"] {
            host.services
                .register_instance(ServiceInstanceRegistration {
                    instance_id: ExtensionInstanceId::new(name),
                    extension_id: ExtensionId::new("example.stream"),
                    scope_id: RuntimeScopeId::new("world:test"),
                    definitions: Vec::new(),
                    providers: Vec::new(),
                    consumers: Vec::new(),
                    components: std::collections::HashMap::new(),
                })
                .unwrap();
        }
        let make = |name: &str| {
            let mut state = WasmHostState::with_host_services_and_budget(
                ComponentId::new("runtime"),
                host.clone(),
                false,
                &WasmExecutionBudget::default(),
            );
            state.execution_owner = Some(ComponentRef::new(name, "runtime"));
            state.operation_access_active = true;
            state
        };
        (make("provider"), make("client"), make("intruder"))
    }

    #[test]
    fn observer_lease_is_bound_to_authenticated_service_caller() {
        let (mut provider, mut client, mut intruder) = shared_states();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        assert!(matches!(
            AsyncOperationHost::share_with_caller(&mut provider, handle.clone()),
            Err(AsyncOperationError::InvalidState)
        ));
        provider.service_caller = Some(ComponentRef::new("client", "runtime"));
        let token = AsyncOperationHost::share_with_caller(&mut provider, handle.clone()).unwrap();
        assert_ne!(token, handle);
        assert!(matches!(
            AsyncOperationHost::share_with_caller(&mut provider, handle.clone()),
            Err(AsyncOperationError::InvalidState)
        ));
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut intruder, token.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::close_observer(&mut intruder, token.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut provider, handle.clone()),
            Err(AsyncOperationError::InvalidState)
        ));
        AsyncOperationHost::push_progress(&mut provider, handle.clone(), 25).unwrap();
        AsyncOperationHost::push_data(&mut provider, handle.clone(), b"stream".to_vec()).unwrap();
        AsyncOperationHost::complete(&mut provider, handle.clone(), b"done".to_vec()).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token.clone()),
            Ok(AsyncOperationEvent::Progress(AsyncOperationProgress {
                sequence: 1,
                percent: 25
            }))
        ));
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token.clone()),
            Ok(AsyncOperationEvent::Data(AsyncOperationData { sequence: 2, bytes }))
                if bytes == b"stream"
        ));
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token.clone()),
            Ok(AsyncOperationEvent::Completed(bytes)) if bytes == b"done"
        ));
        assert!(matches!(
            AsyncOperationHost::cancel(&mut client, handle.clone()),
            Err(AsyncOperationError::UnknownHandle)
        ));
        AsyncOperationHost::close_observer(&mut client, token.clone()).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut provider, handle),
            Ok(AsyncOperationEvent::Completed(bytes)) if bytes == b"done"
        ));
    }

    #[test]
    fn producer_cancel_revokes_all_provisional_observer_events() {
        let (mut provider, mut client, _) = shared_states();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        provider.service_caller = Some(ComponentRef::new("client", "runtime"));
        let token = AsyncOperationHost::share_with_caller(&mut provider, handle.clone()).unwrap();
        AsyncOperationHost::push_data(&mut provider, handle.clone(), b"partial".to_vec()).unwrap();
        AsyncOperationHost::cancel(&mut provider, handle).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token),
            Ok(AsyncOperationEvent::Cancelled)
        ));
    }

    #[test]
    fn producer_shutdown_invalidates_reader_without_retaining_operation() {
        let (mut provider, mut client, _) = shared_states();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        provider.service_caller = Some(ComponentRef::new("client", "runtime"));
        let token = AsyncOperationHost::share_with_caller(&mut provider, handle).unwrap();
        provider.revoke_runtime_resources_for_owner(&ComponentRef::new("provider", "runtime"));
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert_eq!(provider.async_observers.leases.lock().unwrap().len(), 0);
    }

    #[test]
    fn consumer_stop_revokes_lease_and_allows_new_reader() {
        let (mut provider, mut client, _) = shared_states();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        provider.service_caller = Some(ComponentRef::new("client", "runtime"));
        let old_token =
            AsyncOperationHost::share_with_caller(&mut provider, handle.clone()).unwrap();
        client.revoke_runtime_resources_for_owner(&ComponentRef::new("client", "runtime"));
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, old_token),
            Err(AsyncOperationError::UnknownHandle)
        ));
        assert!(matches!(
            AsyncOperationHost::poll(&mut provider, handle.clone()),
            Ok(AsyncOperationEvent::Pending)
        ));
        let new_token = AsyncOperationHost::share_with_caller(&mut provider, handle).unwrap();
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, new_token),
            Ok(AsyncOperationEvent::Pending)
        ));
    }

    #[test]
    fn idle_observer_poll_exposes_producer_timeout_without_producer_callbacks() {
        let (mut provider, mut client, _) = shared_states();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        provider.service_caller = Some(ComponentRef::new("client", "runtime"));
        let token = AsyncOperationHost::share_with_caller(&mut provider, handle.clone()).unwrap();
        {
            let operation = provider.async_operations.get(&handle).unwrap();
            operation.lock().unwrap().last_activity =
                Instant::now() - MAX_IDLE_DURATION - Duration::from_secs(1);
        }
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token.clone()),
            Ok(AsyncOperationEvent::Failed(reason)) if reason == "operation timed out"
        ));
        {
            let operation = provider.async_operations.get(&handle).unwrap();
            operation.lock().unwrap().terminal_since =
                Some(Instant::now() - TERMINAL_RETENTION - Duration::from_secs(1));
        }
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, token),
            Err(AsyncOperationError::UnknownHandle)
        ));
    }

    #[test]
    fn cross_scope_caller_cannot_receive_operation_lease() {
        use crate::services::ServiceInstanceRegistration;
        use rintawa_sdk::types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId};

        let (mut provider, mut client, _) = shared_states();
        provider
            .services
            .register_instance(ServiceInstanceRegistration {
                instance_id: ExtensionInstanceId::new("other-world"),
                extension_id: ExtensionId::new("example.stream"),
                scope_id: RuntimeScopeId::new("world:another"),
                definitions: Vec::new(),
                providers: Vec::new(),
                consumers: Vec::new(),
                components: std::collections::HashMap::new(),
            })
            .unwrap();
        let handle = AsyncOperationHost::create(&mut provider).unwrap();
        provider.service_caller = Some(ComponentRef::new("other-world", "runtime"));
        assert!(matches!(
            AsyncOperationHost::share_with_caller(&mut provider, handle.clone()),
            Err(AsyncOperationError::InvalidState)
        ));
        assert!(provider.async_observers.leases.lock().unwrap().is_empty());
        assert!(matches!(
            AsyncOperationHost::poll_observer(&mut client, handle),
            Err(AsyncOperationError::UnknownHandle)
        ));
    }
}
