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
    time::{Duration, Instant},
};

use rintawa_sdk::{
    contracts::{ComponentPrincipal, ComponentRef},
    types::{ExtensionInstanceId, RuntimeScopeId},
};
use thiserror::Error;

mod polled;

use crate::execution_context::{ExecutionContext, ExecutionContexts};
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
    /// Unknown, foreign, expired or already-consumed nonblocking handle.
    #[error("invalid transport handle")]
    InvalidHandle,
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
    polled: polled::PolledState,
}

struct OwnedQueryable {
    owner: ComponentRef,
    key: String,
    declaration: Queryable<()>,
}

struct OwnedSubscription {
    owner: ComponentPrincipal,
    handle: u64,
    subscriber: Subscriber<RingChannelHandler<Sample>>,
}

#[cfg(test)]
type CallbackHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

/// One lazily opened local Zenoh session shared by an Extension Engine.
///
/// Session identity and scope are supplied by the engine, never by guest payloads.
/// Declarations live only while their concrete component instance is active.
#[derive(Clone, Default)]
pub(crate) struct Transport {
    state: Arc<Mutex<State>>,
    active_callbacks: Arc<AtomicUsize>,
    pub(crate) contexts: ExecutionContexts,
    #[cfg(test)]
    pub(crate) before_callback: CallbackHook,
    #[cfg(test)]
    pub(crate) after_callback: CallbackHook,
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
            || state.polled.conflicting(&key, owner)
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
        F: Fn(Query, ComponentPrincipal) + Send + Sync + 'static,
    {
        let key = scoped_key(scope, point)?;
        let executing = self.contexts.context(owner, true)?;
        if executing.scope != *scope {
            return Err(TransportError::Inactive);
        }
        let contexts = self.contexts.clone();
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
            || state.polled.duplicate(&key, owner)
        {
            return Err(TransportError::Duplicate);
        }
        if state
            .queryables
            .iter()
            .filter(|registered| registered.owner == *owner)
            .count()
            + state.polled.declarations_for(owner)
            >= MAX_DECLARATIONS_PER_OWNER
        {
            return Err(TransportError::LimitExceeded);
        }
        let zenoh = session(&mut state)?;
        let active_callbacks = Arc::clone(&self.active_callbacks);
        let callback = Arc::new(callback);
        #[cfg(test)]
        let before_callback = self.before_callback.clone();
        #[cfg(test)]
        let after_callback = self.after_callback.clone();
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
                let caller = polled::request_caller(&query).ok();
                let Some(caller) = caller else {
                    let _ = query.reply_err(b"missing caller".to_vec()).wait();
                    active_callbacks.fetch_sub(1, Ordering::AcqRel);
                    return;
                };
                let permit = CallbackPermit(Arc::clone(&active_callbacks));
                let contexts = contexts.clone();
                let executing = executing.clone();
                let callback = Arc::clone(&callback);
                #[cfg(test)]
                let before_callback = before_callback.clone();
                #[cfg(test)]
                let after_callback = after_callback.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name(String::from("rintawa-zenoh-callback"))
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        let _permit = permit;
                        let valid =
                            contexts
                                .context(&caller.component(), false)
                                .is_ok_and(|context| {
                                    context.principal == caller && context.scope == executing.scope
                                })
                                && contexts.validate(&executing).is_ok();
                        if valid {
                            #[cfg(test)]
                            {
                                let hook = before_callback.lock().unwrap().take();
                                if let Some(hook) = hook {
                                    hook();
                                }
                            }
                            let live_caller = contexts
                                .context(&caller.component(), false)
                                .is_ok_and(|bound| {
                                    bound.principal == caller && bound.scope == executing.scope
                                });
                            if live_caller
                                && contexts.validate(&executing).is_ok()
                                && !polled::expired_metadata(&query)
                            {
                                callback(query, caller);
                            } else {
                                let _ = query
                                    .reply_err(b"expired caller or provider".to_vec())
                                    .wait();
                            }
                            #[cfg(test)]
                            {
                                let hook = after_callback.lock().unwrap().take();
                                if let Some(hook) = hook {
                                    hook();
                                }
                            }
                        } else {
                            let _ = query
                                .reply_err(b"inactive caller or provider".to_vec())
                                .wait();
                        }
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
        executing: &ExecutionContext,
        point: &str,
        payload: &[u8],
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        self.query_with_timeout(executing, point, payload, QUERY_TIMEOUT)
    }
    pub(crate) fn query_with_timeout(
        &self,
        executing: &ExecutionContext,
        point: &str,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let timeout = timeout.min(QUERY_TIMEOUT);
        if timeout.is_zero() {
            return Err(TransportError::Unavailable);
        }
        self.contexts.validate(executing)?;
        let scope = &executing.scope;
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
        let metadata = polled::metadata(&executing.principal, Instant::now() + timeout)?;
        let replies = zenoh
            .get(key)
            .payload(payload.to_vec())
            .attachment(metadata)
            .target(QueryTarget::All)
            .consolidation(ConsolidationMode::None)
            .timeout(timeout)
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
        owner: &ComponentPrincipal,
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
        owner: &ComponentPrincipal,
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
        owner: &ComponentPrincipal,
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
        self.revoke_instance_if(instance, None);
    }

    pub(crate) fn revoke_activation(&self, expected: &ExecutionContext) {
        self.revoke_instance_if(&expected.principal.instance_id, Some(expected));
    }

    fn revoke_instance_if(
        &self,
        instance: &ExtensionInstanceId,
        expected: Option<&ExecutionContext>,
    ) {
        let removed = {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(poisoned) => {
                    tracing::warn!("Zenoh declaration lock was poisoned during cleanup");
                    poisoned.into_inner()
                }
            };
            // Serialize declaration cleanup with publication of replacement resources.
            let retired = self.contexts.revoke_instance_if(instance, expected);
            let revoked = &retired.owners;
            if expected.is_some() && revoked.is_empty() {
                return;
            }
            let polled = state.polled.revoke(instance, revoked);
            let mut kept = Vec::new();
            let mut removed = Vec::new();
            for declaration in std::mem::take(&mut state.queryables) {
                if &declaration.owner.instance_id == instance
                    || revoked.contains(&declaration.owner)
                {
                    removed.push(declaration.declaration);
                } else {
                    kept.push(declaration);
                }
            }
            state.queryables = kept;
            let mut kept_subscriptions = Vec::new();
            let mut removed_subscriptions = Vec::new();
            for subscription in std::mem::take(&mut state.subscriptions) {
                if &subscription.owner.instance_id == instance
                    || revoked.contains(&subscription.owner.component())
                {
                    removed_subscriptions.push(subscription.subscriber);
                } else {
                    kept_subscriptions.push(subscription);
                }
            }
            state.subscriptions = kept_subscriptions;
            (removed, removed_subscriptions, polled, retired)
        };
        removed.3.dispose();
        removed.2.dispose();
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

    fn admit(
        transport: &Transport,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
    ) -> Result<ExecutionContext, TransportError> {
        if let Ok(context) = transport.contexts.context(owner, false) {
            return Ok(context);
        }
        transport.contexts.begin(owner.clone(), scope.clone())?;
        transport.contexts.activate_instance(&owner.instance_id);
        transport.contexts.context(owner, false)
    }
    fn principal(transport: &Transport, owner: &ComponentRef) -> ComponentPrincipal {
        transport
            .contexts
            .context(owner, false)
            .map(|context| context.principal)
            .unwrap_or_else(|_| ComponentPrincipal {
                instance_id: owner.instance_id.clone(),
                component_id: owner.component_id.clone(),
                activation: 0,
            })
    }
    fn declare<F>(
        transport: &Transport,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
        callback: F,
    ) -> Result<(), TransportError>
    where
        F: Fn(Query, ComponentPrincipal) + Send + Sync + 'static,
    {
        transport.check_declaration(owner, scope, point)?;
        admit(transport, owner, scope)?;
        transport.declare(owner, scope, point, callback)
    }
    fn query(
        transport: &Transport,
        scope: &RuntimeScopeId,
        point: &str,
        payload: &[u8],
    ) -> Result<Vec<Vec<u8>>, TransportError> {
        let context = admit(transport, &owner(&format!("test-caller:{}", scope)), scope)?;
        transport.query(&context, point, payload)
    }
    fn subscribe(
        transport: &Transport,
        owner: &ComponentRef,
        scope: &RuntimeScopeId,
        point: &str,
    ) -> Result<u64, TransportError> {
        let context = admit(transport, owner, scope)?;
        transport.subscribe(&context.principal, scope, point)
    }

    #[test]
    fn test_should_reject_missing_foreign_and_stale_caller_metadata_before_dispatch()
    -> Result<(), TransportError> {
        let transport = Transport::default();
        let scope = RuntimeScopeId::new("world-a");
        let point = "fixture/attestation";
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        declare(
            &transport,
            &owner("provider"),
            &scope,
            point,
            move |query, _caller| {
                observed.fetch_add(1, Ordering::Relaxed);
                let _ = query
                    .reply(query.key_expr().as_str(), b"ok".to_vec())
                    .wait();
            },
        )?;
        let caller = admit(&transport, &owner("caller"), &scope)?;
        let foreign = admit(
            &transport,
            &owner("foreign"),
            &RuntimeScopeId::new("world-b"),
        )?;
        let mut stale = caller.principal.clone();
        stale.activation += 1;
        let zenoh = {
            let mut state = transport
                .state
                .lock()
                .map_err(|_| TransportError::Unavailable)?;
            session(&mut state)?
        };
        for metadata in [None, Some(polled::metadata(&foreign.principal,Instant::now()+QUERY_TIMEOUT).expect("foreign metadata")), Some(polled::metadata(&stale,Instant::now()+QUERY_TIMEOUT).expect("stale metadata")), Some(br#"{"instance_id":"caller","component_id":"component","activation":0,"scope":"world-a"}"#.to_vec())] {
            let request = zenoh.get(scoped_key(&scope, point)?).payload(b"claim-caller".to_vec()).timeout(QUERY_TIMEOUT);
            let replies = match metadata { Some(bytes) => request.attachment(bytes).wait(), None => request.wait() }.map_err(|_| TransportError::Unavailable)?;
            let replies: Vec<_> = replies.into_iter().collect();
            assert_eq!(replies.len(), 1);
            assert!(replies[0].result().is_err());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            transport.query(&caller, point, b"valid")?,
            vec![b"ok".to_vec()]
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn test_should_isolate_scopes_and_drop_instance_declarations() -> Result<(), TransportError> {
        let transport = Transport::default();
        let scope_a = RuntimeScopeId::new("a/../other");
        let scope_b = RuntimeScopeId::new("b");
        let instance_a = owner("one");
        let instance_b = owner("two");
        let point = "chat/send-controls/v1";
        declare(
            &transport,
            &instance_a,
            &scope_a,
            point,
            |query, _caller| {
                let _ = query
                    .reply(query.key_expr().as_str(), b"dice".to_vec())
                    .wait();
            },
        )?;
        declare(
            &transport,
            &instance_b,
            &scope_a,
            point,
            |query, _caller| {
                let _ = query
                    .reply(query.key_expr().as_str(), b"emoji".to_vec())
                    .wait();
            },
        )?;
        let mut replies = query(&transport, &scope_a, point, b"render")?;
        replies.sort();
        assert_eq!(replies, vec![b"dice".to_vec(), b"emoji".to_vec()]);
        assert!(query(&transport, &scope_b, point, b"render")?.is_empty());
        assert!(matches!(
            declare(&transport, &instance_a, &scope_a, point, |_, _| {}),
            Err(TransportError::Duplicate)
        ));
        transport.revoke_instance(&instance_a.instance_id);
        assert_eq!(
            query(&transport, &scope_a, point, b"render")?,
            vec![b"emoji".to_vec()]
        );
        transport.revoke_instance(&instance_b.instance_id);
        assert!(query(&transport, &scope_a, point, b"render")?.is_empty());
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

        declare(&transport, &slow, &scope, point, |query, _caller| {
            std::thread::sleep(QUERY_TIMEOUT + Duration::from_millis(250));
            let _ = query
                .reply(query.key_expr().as_str(), b"late".to_vec())
                .wait();
        })?;
        declare(&transport, &fast, &scope, point, |query, _caller| {
            let _ = query
                .reply(query.key_expr().as_str(), b"ready".to_vec())
                .wait();
        })?;
        let started = std::time::Instant::now();
        let replies = query(&transport, &scope, point, b"render")?;
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
                query(&transport, &scope, point, b"render")?,
                vec![b"ready".to_vec()]
            );
        }
        let avg = start.elapsed() / samples;
        println!("local Zenoh single-provider reply: {avg:?} mean over {samples} queries");
        transport.revoke_instance(&fast.instance_id);
        assert!(query(&transport, &scope, point, b"render")?.is_empty());
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
            declare(&transport, &other, &host, point, |_, _| {}),
            Err(TransportError::NotSelected)
        ));
        assert!(matches!(
            transport.publish(&other, &host, point, b"fake"),
            Err(TransportError::NotSelected)
        ));
        assert!(matches!(
            query(&transport, &host, point, b"hello"),
            Err(TransportError::Unavailable)
        ));

        declare(&transport, &selected, &host, point, |query, _caller| {
            let _ = query
                .reply(query.key_expr().as_str(), b"host".to_vec())
                .wait();
        })?;
        declare(&transport, &other, &world, point, |query, _caller| {
            let _ = query
                .reply(query.key_expr().as_str(), b"world".to_vec())
                .wait();
        })?;
        assert_eq!(
            query(&transport, &host, point, b"hello")?,
            vec![b"host".to_vec()]
        );
        assert_eq!(
            query(&transport, &world, point, b"hello")?,
            vec![b"world".to_vec()]
        );

        assert!(matches!(
            transport.select_authoritative(&host, point, &other),
            Err(TransportError::ActiveConflict)
        ));
        transport.revoke_instance(&selected.instance_id);
        assert!(matches!(
            query(&transport, &host, point, b"hello"),
            Err(TransportError::Unavailable)
        ));
        assert!(matches!(
            declare(&transport, &other, &host, point, |_, _| {}),
            Err(TransportError::NotSelected)
        ));
        assert_eq!(
            query(&transport, &world, point, b"hello")?,
            vec![b"world".to_vec()]
        );

        transport.clear_authoritative_in_scope(&host);
        let fallback = owner("fallback");
        declare(&transport, &fallback, &host, point, |query, _caller| {
            let _ = query
                .reply(query.key_expr().as_str(), b"fallback".to_vec())
                .wait();
        })?;
        assert_eq!(
            query(&transport, &host, point, b"hello")?,
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

        let handle = subscribe(&transport, &owner_a, &scope_a, point)?;
        let foreign = subscribe(&transport, &owner_b, &scope_b, point)?;
        assert!(matches!(
            transport.poll(&principal(&transport, &owner_b), handle),
            Err(TransportError::UnknownSubscription)
        ));
        assert!(matches!(
            transport.unsubscribe(&principal(&transport, &owner_b), handle),
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
        let mut received = transport.poll(&principal(&transport, &owner_a), handle)?;
        assert_eq!(received.len(), MAX_EVENTS_PER_POLL);
        received.extend(transport.poll(&principal(&transport, &owner_a), handle)?);
        assert_eq!(received.len(), MAX_EVENTS_PER_SUBSCRIPTION);
        assert_eq!(received.first(), Some(&b"04".to_vec()));
        assert_eq!(received.last(), Some(&b"19".to_vec()));
        assert!(
            transport
                .poll(&principal(&transport, &owner_b), foreign)?
                .is_empty()
        );

        transport.revoke_instance(&owner_a.instance_id);
        assert!(matches!(
            transport.poll(&principal(&transport, &owner_a), handle),
            Err(TransportError::UnknownSubscription)
        ));
        let replacement = subscribe(&transport, &owner_a, &scope_a, point)?;
        assert_ne!(replacement, handle);
        transport.unsubscribe(&principal(&transport, &owner_a), replacement)?;
        assert!(matches!(
            transport.unsubscribe(&principal(&transport, &owner_a), replacement),
            Err(TransportError::UnknownSubscription)
        ));
        transport.revoke_instance(&owner_b.instance_id);
        assert!(matches!(
            transport.poll(&principal(&transport, &owner_b), foreign),
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
                query(&transport, &scope, invalid, b"render"),
                Err(TransportError::InvalidName)
            ));
        }
        assert!(matches!(
            query(
                &transport,
                &scope,
                "chat/input",
                &vec![0; MAX_PAYLOAD_BYTES + 1]
            ),
            Err(TransportError::LimitExceeded)
        ));
        Ok(())
    }
}
