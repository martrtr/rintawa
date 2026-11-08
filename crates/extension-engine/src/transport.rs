//! Host-owned Zenoh declarations for ephemeral extension contributions.
//!
//! There is no contract registry or second resolver: Zenoh performs the fan-out.
//! The engine authenticates the component before reaching this module.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use rintawa_sdk::{
    contracts::ComponentRef,
    types::{ExtensionInstanceId, RuntimeScopeId},
};
use thiserror::Error;
use zenoh::{
    Config, Session, Wait,
    handlers::{RingChannel, RingChannelHandler},
    pubsub::Subscriber,
    query::{ConsolidationMode, Query, QueryTarget, Queryable},
    sample::Sample,
};

pub(crate) const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_REPLIES: usize = 32;
const MAX_TOTAL_REPLY_BYTES: usize = 512 * 1024;
const MAX_DECLARATIONS_PER_OWNER: usize = 16;
const MAX_CONCURRENT_PROVIDER_CALLBACKS: usize = 64;
const MAX_SUBSCRIPTIONS_PER_OWNER: usize = 16;
const MAX_AUTHORITATIVE_POINTS_PER_SCOPE: usize = 64;
const MAX_EVENTS_PER_SUBSCRIPTION: usize = 16;
// One receive cannot consume more bytes than the Host can safely return.
const MAX_EVENTS_PER_POLL: usize = MAX_TOTAL_REPLY_BYTES / MAX_PAYLOAD_BYTES;
const QUERY_TIMEOUT: Duration = Duration::from_millis(500);

/// A rejected or failed ephemeral transport operation.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The caller is not an active registered component.
    #[error("component is not active")]
    Inactive,
    /// The name contains disallowed characters or wildcard expressions.
    #[error("invalid extension point name")]
    InvalidName,
    /// This component already registered the named point.
    #[error("extension point is already registered by this component")]
    Duplicate,
    /// Bounded registration or response limits were exceeded.
    #[error("transport resource limit exceeded")]
    LimitExceeded,
    /// The Zenoh session or a provider could not complete the request.
    #[error("transport unavailable")]
    Unavailable,
    /// The subscription handle is not owned by this component.
    #[error("unknown transport subscription")]
    UnknownSubscription,
    /// Host selected a different component for this authoritative point.
    #[error("component is not selected for this authoritative point")]
    NotSelected,
    /// This point already has an incompatible live registration.
    #[error("active registrations conflict with the selected authoritative provider")]
    ActiveConflict,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    queryables: Vec<OwnedQueryable>,
    subscriptions: Vec<OwnedSubscription>,
    next_subscription: u64,
    authoritative: HashMap<(RuntimeScopeId, String), ComponentRef>,
}

struct OwnedQueryable {
    owner: ComponentRef,
    key: String,
    declaration: Queryable<()>,
}

struct OwnedSubscription {
    owner: ComponentRef,
    handle: u64,
    subscriber: Subscriber<RingChannelHandler<Sample>>,
}

/// One lazily opened local Zenoh session shared by an Extension Engine.
///
/// Session identity and scope are supplied by the engine, never by guest payloads.
/// Declarations live only while their concrete component instance is active.
#[derive(Clone, Default)]
pub(crate) struct Transport {
    state: Arc<Mutex<State>>,
    active_callbacks: Arc<AtomicUsize>,
}

/// One bounded callback slot, released even when trusted native code panics.
struct CallbackPermit(Arc<AtomicUsize>);

impl Drop for CallbackPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn session(state: &mut State) -> Result<Session, TransportError> {
    if state.session.is_none() {
        let config = Config::from_json5(
            r#"{
                mode: "peer",
                listen: { endpoints: [] },
                connect: { endpoints: [] },
                scouting: {
                    multicast: { enabled: false },
                    gossip: { enabled: false }
                }
            }"#,
        )
        .map_err(|_| TransportError::Unavailable)?;
        state.session = Some(
            zenoh::open(config)
                .wait()
                .map_err(|_| TransportError::Unavailable)?,
        );
    }
    state.session.clone().ok_or(TransportError::Unavailable)
}

/// Checks the syntax of an untrusted extension point name.
///
/// # Errors
/// Rejects empty, wildcard and incorrectly delimited names.
pub fn validate_transport_point(point: &str) -> Result<(), TransportError> {
    if point.is_empty()
        || point.len() > 256
        || point.split('/').count() > 5
        || !point.split('/').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 64
                && segment.as_bytes()[0].is_ascii_alphanumeric()
                && !segment.contains("..")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err(TransportError::InvalidName);
    }
    Ok(())
}

// Scope IDs are opaque and may contain arbitrary separators. Encoding exact
// bytes prevents a crafted scope from escaping its Zenoh key prefix.
fn scoped_key(scope: &RuntimeScopeId, point: &str) -> Result<String, TransportError> {
    validate_transport_point(point)?;
    let mut encoded = String::with_capacity(scope.as_str().len() * 2);
    for byte in scope.as_str().bytes() {
        use std::fmt::Write;
        write!(encoded, "{byte:02x}").map_err(|_| TransportError::InvalidName)?;
    }
    Ok(format!("rintawa/world/{encoded}/contributions/{point}"))
}

impl Transport {
    // These policies are Host-owned and are never writable through guest WIT.
    // Zenoh remains responsible for routing and fan-out.
    pub(crate) fn select_authoritative(
        &self,
        scope: &RuntimeScopeId,
        point: &str,
        owner: &ComponentRef,
    ) -> Result<(), TransportError> {
        let key = scoped_key(scope, point)?;
        let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
        if state
            .queryables
            .iter()
            .any(|entry| entry.key == key && entry.owner != *owner)
        {
            return Err(TransportError::ActiveConflict);
        }
        let policy_key = (scope.clone(), point.to_string());
        if !state.authoritative.contains_key(&policy_key)
            && state
                .authoritative
                .keys()
                .filter(|(existing_scope, _)| existing_scope == scope)
                .count()
                >= MAX_AUTHORITATIVE_POINTS_PER_SCOPE
        {
            return Err(TransportError::LimitExceeded);
        }
        state.authoritative.insert(policy_key, owner.clone());
        Ok(())
    }

    pub(crate) fn clear_authoritative_in_scope(&self, scope: &RuntimeScopeId) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state
            .authoritative
            .retain(|(entry_scope, _), _| entry_scope != scope);
    }

    pub(crate) fn check_declaration(
        &self,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
    ) -> Result<(), TransportError> {
        validate_transport_point(point)?;
        let state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
        if let Some(selected) = state.authoritative.get(&(scope.clone(), point.to_string()))
            && selected != owner
        {
            return Err(TransportError::NotSelected);
        }
        Ok(())
    }

    pub(crate) fn declare<F>(
        &self,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
        callback: F,
    ) -> Result<(), TransportError>
    where
        F: Fn(Query) + Send + Sync + 'static,
    {
        let key = scoped_key(scope, point)?;
        let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
        if let Some(selected) = state.authoritative.get(&(scope.clone(), point.to_string()))
            && selected != owner
        {
            return Err(TransportError::NotSelected);
        }
        if state
            .queryables
            .iter()
            .any(|registered| registered.owner == *owner && registered.key == key)
        {
            return Err(TransportError::Duplicate);
        }
        if state
            .queryables
            .iter()
            .filter(|registered| registered.owner == *owner)
            .count()
            >= MAX_DECLARATIONS_PER_OWNER
        {
            return Err(TransportError::LimitExceeded);
        }
        let zenoh = session(&mut state)?;
        let active_callbacks = Arc::clone(&self.active_callbacks);
        let callback = Arc::new(callback);
        let declaration = zenoh
            .declare_queryable(key.as_str())
            .callback(move |query: Query| {
                // Zenoh may invoke multiple queryables on one runtime worker.
                // Running a slow guest directly here can starve fast responders.
                // Bound the number of active callbacks before leaving that worker.
                if active_callbacks
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |running| {
                        (running < MAX_CONCURRENT_PROVIDER_CALLBACKS).then_some(running + 1)
                    })
                    .is_err()
                {
                    let _ = query.reply_err(b"provider busy".to_vec()).wait();
                    return;
                }
                let permit = CallbackPermit(Arc::clone(&active_callbacks));
                let callback = Arc::clone(&callback);
                if let Err(error) = std::thread::Builder::new()
                    .name(String::from("rintawa-zenoh-callback"))
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        let _permit = permit;
                        callback(query);
                    })
                {
                    // Thread creation failure must not leak a callback slot.
                    tracing::warn!(%error, "failed to start a bounded Zenoh callback");
                }
            })
            .wait()
            .map_err(|_| TransportError::Unavailable)?;
        state.queryables.push(OwnedQueryable {
            owner: owner.clone(),
            key,
            declaration,
        });
        Ok(())
    }

    pub(crate) fn query(
        &self,
        scope: &RuntimeScopeId,
        point: &str,
        payload: &[u8],
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let key = scoped_key(scope, point)?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(TransportError::LimitExceeded);
        }
        let zenoh = {
            let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
            if state
                .authoritative
                .contains_key(&(scope.clone(), point.to_string()))
                && !state.queryables.iter().any(|entry| entry.key == key)
            {
                return Err(TransportError::Unavailable);
            }
            session(&mut state)?
        };
        let replies = zenoh
            .get(key)
            .payload(payload.to_vec())
            .target(QueryTarget::All)
            .consolidation(ConsolidationMode::None)
            .timeout(QUERY_TIMEOUT)
            .wait()
            .map_err(|_| TransportError::Unavailable)?;
        let mut values = Vec::new();
        let mut total_bytes = 0_usize;
        let mut failures = 0_usize;
        for (count, reply) in replies.into_iter().enumerate() {
            if count >= MAX_REPLIES {
                return Err(TransportError::LimitExceeded);
            }
            match reply.result() {
                Ok(sample) => {
                    let data = sample.payload().to_bytes();
                    total_bytes = total_bytes.saturating_add(data.len());
                    if data.len() > MAX_PAYLOAD_BYTES || total_bytes > MAX_TOTAL_REPLY_BYTES {
                        return Err(TransportError::LimitExceeded);
                    }
                    values.push(data.into_owned());
                }
                Err(error) => {
                    total_bytes = total_bytes.saturating_add(error.payload().len());
                    failures += 1;
                    if total_bytes > MAX_TOTAL_REPLY_BYTES {
                        return Err(TransportError::LimitExceeded);
                    }
                }
            }
        }
        if values.is_empty() && failures > 0 {
            return Err(TransportError::Unavailable);
        }
        Ok(values)
    }

    pub(crate) fn publish(
        &self,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        let key = scoped_key(scope, point)?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(TransportError::LimitExceeded);
        }
        let zenoh = {
            let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
            if let Some(selected) = state.authoritative.get(&(scope.clone(), point.to_string()))
                && selected != owner
            {
                return Err(TransportError::NotSelected);
            }
            session(&mut state)?
        };
        zenoh
            .put(key, payload.to_vec())
            .wait()
            .map_err(|_| TransportError::Unavailable)
    }

    pub(crate) fn subscribe(
        &self,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
    ) -> Result<u64, TransportError> {
        let key = scoped_key(scope, point)?;
        let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
        if state
            .subscriptions
            .iter()
            .filter(|subscription| subscription.owner == *owner)
            .count()
            >= MAX_SUBSCRIPTIONS_PER_OWNER
        {
            return Err(TransportError::LimitExceeded);
        }
        let handle = state
            .next_subscription
            .checked_add(1)
            .ok_or(TransportError::LimitExceeded)?;
        let zenoh = session(&mut state)?;
        let subscriber = zenoh
            .declare_subscriber(key)
            .with(RingChannel::new(MAX_EVENTS_PER_SUBSCRIPTION))
            .wait()
            .map_err(|_| TransportError::Unavailable)?;
        state.next_subscription = handle;
        state.subscriptions.push(OwnedSubscription {
            owner: owner.clone(),
            handle,
            subscriber,
        });
        Ok(handle)
    }

    pub(crate) fn poll(
        &self,
        owner: &ComponentRef,
        handle: u64,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
        let subscription = state
            .subscriptions
            .iter()
            .find(|subscription| subscription.owner == *owner && subscription.handle == handle)
            .ok_or(TransportError::UnknownSubscription)?;
        let mut payloads = Vec::new();
        let mut total_bytes = 0_usize;
        for _ in 0..MAX_EVENTS_PER_POLL {
            let Some(sample) = subscription
                .subscriber
                .try_recv()
                .map_err(|_| TransportError::Unavailable)?
            else {
                break;
            };
            let payload = sample.payload().to_bytes();
            total_bytes = total_bytes.saturating_add(payload.len());
            if payload.len() > MAX_PAYLOAD_BYTES || total_bytes > MAX_TOTAL_REPLY_BYTES {
                return Err(TransportError::LimitExceeded);
            }
            payloads.push(payload.into_owned());
        }
        Ok(payloads)
    }

    pub(crate) fn unsubscribe(
        &self,
        owner: &ComponentRef,
        handle: u64,
    ) -> Result<(), TransportError> {
        let subscription = {
            let mut state = self.state.lock().map_err(|_| TransportError::Unavailable)?;
            let index = state
                .subscriptions
                .iter()
                .position(|subscription| {
                    subscription.owner == *owner && subscription.handle == handle
                })
                .ok_or(TransportError::UnknownSubscription)?;
            state.subscriptions.remove(index)
        };
        subscription
            .subscriber
            .undeclare()
            .wait()
            .map_err(|_| TransportError::Unavailable)
    }

    pub(crate) fn revoke_instance(&self, instance: &ExtensionInstanceId) {
        let removed = {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(poisoned) => {
                    tracing::warn!("Zenoh declaration lock was poisoned during cleanup");
                    poisoned.into_inner()
                }
            };
            let mut kept = Vec::new();
            let mut removed = Vec::new();
            for declaration in std::mem::take(&mut state.queryables) {
                if &declaration.owner.instance_id == instance {
                    removed.push(declaration.declaration);
                } else {
                    kept.push(declaration);
                }
            }
            state.queryables = kept;
            let mut kept_subscriptions = Vec::new();
            let mut removed_subscriptions = Vec::new();
            for subscription in std::mem::take(&mut state.subscriptions) {
                if &subscription.owner.instance_id == instance {
                    removed_subscriptions.push(subscription.subscriber);
                } else {
                    kept_subscriptions.push(subscription);
                }
            }
            state.subscriptions = kept_subscriptions;
            (removed, removed_subscriptions)
        };
        for declaration in removed.0 {
            if let Err(error) = declaration.undeclare().wait() {
                tracing::warn!(%error, "failed to undeclare stopped component's Zenoh key");
            }
        }
        for subscription in removed.1 {
            if let Err(error) = subscription.undeclare().wait() {
                tracing::warn!(%error, "failed to undeclare stopped component's Zenoh subscription");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(instance: &str) -> ComponentRef {
        ComponentRef::new(instance, "component")
    }

    #[test]
    fn test_should_isolate_scopes_and_drop_instance_declarations() -> Result<(), TransportError> {
        let transport = Transport::default();
        let scope_a = RuntimeScopeId::new("a/../other");
        let scope_b = RuntimeScopeId::new("b");
        let instance_a = owner("one");
        let instance_b = owner("two");
        let point = "chat/send-controls/v1";
        transport.declare(&instance_a, &scope_a, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"dice".to_vec())
                .wait();
        })?;
        transport.declare(&instance_b, &scope_a, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"emoji".to_vec())
                .wait();
        })?;
        let mut replies = transport.query(&scope_a, point, b"render")?;
        replies.sort();
        assert_eq!(replies, vec![b"dice".to_vec(), b"emoji".to_vec()]);
        assert!(transport.query(&scope_b, point, b"render")?.is_empty());
        assert!(matches!(
            transport.declare(&instance_a, &scope_a, point, |_| {}),
            Err(TransportError::Duplicate)
        ));
        transport.revoke_instance(&instance_a.instance_id);
        assert_eq!(
            transport.query(&scope_a, point, b"render")?,
            vec![b"emoji".to_vec()]
        );
        transport.revoke_instance(&instance_b.instance_id);
        assert!(transport.query(&scope_a, point, b"render")?.is_empty());
        Ok(())
    }

    #[test]
    fn test_should_bound_slow_provider_without_stalling_fast_fanout_or_stop()
    -> Result<(), TransportError> {
        let transport = Transport::default();
        let scope = RuntimeScopeId::new("world-latency");
        let slow = owner("slow");
        let fast = owner("fast");
        let point = "ui/panels/v1";

        transport.declare(&slow, &scope, point, |query| {
            std::thread::sleep(QUERY_TIMEOUT + Duration::from_millis(250));
            let _ = query
                .reply(query.key_expr().as_str(), b"late".to_vec())
                .wait();
        })?;
        transport.declare(&fast, &scope, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"ready".to_vec())
                .wait();
        })?;
        let started = std::time::Instant::now();
        let replies = transport.query(&scope, point, b"render")?;
        let elapsed = started.elapsed();
        assert_eq!(replies, vec![b"ready".to_vec()]);
        assert!(
            elapsed <= Duration::from_secs(2),
            "slow provider blocked the caller beyond query deadline: {elapsed:?}"
        );

        let stopped_at = std::time::Instant::now();
        transport.revoke_instance(&slow.instance_id);
        assert!(
            stopped_at.elapsed() <= Duration::from_secs(2),
            "stopping the slow provider stalled the engine"
        );

        let samples = 32;
        let start = std::time::Instant::now();
        for _ in 0..samples {
            assert_eq!(
                transport.query(&scope, point, b"render")?,
                vec![b"ready".to_vec()]
            );
        }
        let avg = start.elapsed() / samples;
        println!("local Zenoh single-provider reply: {avg:?} mean over {samples} queries");
        transport.revoke_instance(&fast.instance_id);
        assert!(transport.query(&scope, point, b"render")?.is_empty());
        Ok(())
    }

    #[test]
    fn test_should_enforce_selected_authority_per_scope_without_fallback()
    -> Result<(), TransportError> {
        let transport = Transport::default();
        let host = RuntimeScopeId::new("host");
        let world = RuntimeScopeId::new("world:a");
        let selected = owner("selected");
        let other = owner("other");
        let point = "rintawa/ai/generate/v1";

        transport.select_authoritative(&host, point, &selected)?;
        transport.select_authoritative(&world, point, &other)?;
        assert!(matches!(
            transport.check_declaration(&other, &host, point),
            Err(TransportError::NotSelected)
        ));
        assert!(matches!(
            transport.declare(&other, &host, point, |_| {}),
            Err(TransportError::NotSelected)
        ));
        assert!(matches!(
            transport.publish(&other, &host, point, b"fake"),
            Err(TransportError::NotSelected)
        ));
        assert!(matches!(
            transport.query(&host, point, b"hello"),
            Err(TransportError::Unavailable)
        ));

        transport.declare(&selected, &host, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"host".to_vec())
                .wait();
        })?;
        transport.declare(&other, &world, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"world".to_vec())
                .wait();
        })?;
        assert_eq!(
            transport.query(&host, point, b"hello")?,
            vec![b"host".to_vec()]
        );
        assert_eq!(
            transport.query(&world, point, b"hello")?,
            vec![b"world".to_vec()]
        );

        assert!(matches!(
            transport.select_authoritative(&host, point, &other),
            Err(TransportError::ActiveConflict)
        ));
        transport.revoke_instance(&selected.instance_id);
        assert!(matches!(
            transport.query(&host, point, b"hello"),
            Err(TransportError::Unavailable)
        ));
        assert!(matches!(
            transport.declare(&other, &host, point, |_| {}),
            Err(TransportError::NotSelected)
        ));
        assert_eq!(
            transport.query(&world, point, b"hello")?,
            vec![b"world".to_vec()]
        );

        transport.clear_authoritative_in_scope(&host);
        transport.declare(&other, &host, point, |query| {
            let _ = query
                .reply(query.key_expr().as_str(), b"fallback".to_vec())
                .wait();
        })?;
        assert_eq!(
            transport.query(&host, point, b"hello")?,
            vec![b"fallback".to_vec()]
        );
        Ok(())
    }

    #[test]
    fn test_should_bound_pubsub_and_revoke_owner_subscriptions() -> Result<(), TransportError> {
        let transport = Transport::default();
        let owner_a = owner("subscriber-a");
        let owner_b = owner("subscriber-b");
        let scope_a = RuntimeScopeId::new("world-a");
        let scope_b = RuntimeScopeId::new("world-b");
        let point = "world/state/changed";

        let handle = transport.subscribe(&owner_a, &scope_a, point)?;
        let foreign = transport.subscribe(&owner_b, &scope_b, point)?;
        assert!(matches!(
            transport.poll(&owner_b, handle),
            Err(TransportError::UnknownSubscription)
        ));
        assert!(matches!(
            transport.unsubscribe(&owner_b, handle),
            Err(TransportError::UnknownSubscription)
        ));
        assert!(matches!(
            transport.publish(&owner_a, &scope_a, "../escape", b"bad"),
            Err(TransportError::InvalidName)
        ));
        assert!(matches!(
            transport.publish(&owner_a, &scope_a, point, &vec![0; MAX_PAYLOAD_BYTES + 1]),
            Err(TransportError::LimitExceeded)
        ));
        for index in 0..(MAX_EVENTS_PER_SUBSCRIPTION + 4) {
            transport.publish(&owner_a, &scope_a, point, format!("{index:02}").as_bytes())?;
        }
        // The Zenoh ring itself discards the oldest events; Core has no queue.
        std::thread::sleep(Duration::from_millis(50));
        let mut received = transport.poll(&owner_a, handle)?;
        assert_eq!(received.len(), MAX_EVENTS_PER_POLL);
        received.extend(transport.poll(&owner_a, handle)?);
        assert_eq!(received.len(), MAX_EVENTS_PER_SUBSCRIPTION);
        assert_eq!(received.first(), Some(&b"04".to_vec()));
        assert_eq!(received.last(), Some(&b"19".to_vec()));
        assert!(transport.poll(&owner_b, foreign)?.is_empty());

        transport.revoke_instance(&owner_a.instance_id);
        assert!(matches!(
            transport.poll(&owner_a, handle),
            Err(TransportError::UnknownSubscription)
        ));
        let replacement = transport.subscribe(&owner_a, &scope_a, point)?;
        assert_ne!(replacement, handle);
        transport.unsubscribe(&owner_a, replacement)?;
        assert!(matches!(
            transport.unsubscribe(&owner_a, replacement),
            Err(TransportError::UnknownSubscription)
        ));
        transport.revoke_instance(&owner_b.instance_id);
        assert!(matches!(
            transport.poll(&owner_b, foreign),
            Err(TransportError::UnknownSubscription)
        ));
        Ok(())
    }

    #[test]
    fn test_should_reject_wildcards_and_oversized_requests() -> Result<(), TransportError> {
        let transport = Transport::default();
        let scope = RuntimeScopeId::new("world");
        for invalid in ["", "**", "../secret", "chat//controls", "chat/*", "/global"] {
            assert!(matches!(
                transport.query(&scope, invalid, b"render"),
                Err(TransportError::InvalidName)
            ));
        }
        assert!(matches!(
            transport.query(&scope, "chat/input", &vec![0; MAX_PAYLOAD_BYTES + 1]),
            Err(TransportError::LimitExceeded)
        ));
        Ok(())
    }
}
