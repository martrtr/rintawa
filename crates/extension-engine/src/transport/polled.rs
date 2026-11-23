//! Nonblocking query admission and polled Zenoh reply routes, owned by activations.
use crate::{
    execution_context::{ExecutionContext, ExecutionContexts},
    transport::{
        MAX_DECLARATIONS_PER_OWNER, MAX_PAYLOAD_BYTES, MAX_REPLIES, MAX_TOTAL_REPLY_BYTES,
        QUERY_TIMEOUT, State, Transport, TransportError as Error, reply_source, reply_with_source,
        scoped_key, session,
    },
};
use rintawa_sdk::{
    contracts::{ComponentPrincipal, ComponentRef},
    types::ExtensionInstanceId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, MutexGuard, OnceLock, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Instant,
};
use zenoh::{
    Wait,
    handlers::CallbackDrop,
    query::{ConsolidationMode, Query, QueryTarget, Queryable, Reply},
};

const MAX_OWNED_QUERIES: usize = 16;
const MAX_RUNNING_QUERIES: usize = 64;
const MAX_CONTROL_WORK: usize = 256;
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static CLOCK: OnceLock<Instant> = OnceLock::new();
fn clock() -> Instant {
    *CLOCK.get_or_init(Instant::now)
}
fn handle() -> Result<u64, Error> {
    NEXT_HANDLE
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map_err(|_| Error::Unavailable)
}
#[derive(Serialize, Deserialize)]
struct Metadata {
    #[serde(flatten)]
    caller: ComponentPrincipal,
    deadline_tick: u64,
}
pub(super) fn metadata(caller: &ComponentPrincipal, deadline: Instant) -> Result<Vec<u8>, Error> {
    let nanos = deadline.saturating_duration_since(clock()).as_nanos();
    let deadline_tick = u64::try_from(nanos).map_err(|_| Error::Unavailable)?;
    serde_json::to_vec(&Metadata {
        caller: caller.clone(),
        deadline_tick,
    })
    .map_err(|_| Error::Unavailable)
}
pub(super) fn request_caller(query: &Query) -> Result<ComponentPrincipal, Error> {
    read_metadata(query).map(|(caller, _)| caller)
}

pub(super) fn expired_metadata(query: &Query) -> bool {
    let Some(attachment) = query.attachment() else {
        return false;
    };
    let bytes = attachment.to_bytes();
    serde_json::from_slice::<Metadata>(&bytes).is_ok_and(|metadata| {
        clock()
            .checked_add(std::time::Duration::from_nanos(metadata.deadline_tick))
            .is_none_or(|deadline| deadline <= Instant::now())
    })
}

fn read_metadata(query: &Query) -> Result<(ComponentPrincipal, Instant), Error> {
    let bytes = query.attachment().ok_or(Error::Unavailable)?.to_bytes();
    if bytes.len() > 1024 * 1024 {
        return Err(Error::LimitExceeded);
    }
    let metadata: Metadata = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
    let deadline = clock()
        .checked_add(std::time::Duration::from_nanos(metadata.deadline_tick))
        .ok_or(Error::Unavailable)?;
    if deadline <= Instant::now() {
        return Err(Error::InvalidHandle);
    }
    Ok((metadata.caller, deadline))
}
/// DTO handed to an owner; the Zenoh reply route never leaves Host.
pub(crate) struct InboundQuery {
    pub(crate) handle: u64,
    pub(crate) caller: ComponentPrincipal,
    pub(crate) payload: Vec<u8>,
}
pub(crate) struct SourcedReply {
    pub(crate) provider: ComponentPrincipal,
    pub(crate) payload: Vec<u8>,
}
struct CollectedReply {
    provider: Option<ComponentPrincipal>,
    payload: Vec<u8>,
}
struct Outgoing {
    owner: ExecutionContext,
    deadline: Instant,
    dispatch_pending: bool,
    done: bool,
    values: Vec<CollectedReply>,
    errors: usize,
    count: usize,
    bytes: usize,
    failure: Option<Error>,
}
struct Declaration {
    owner: ExecutionContext,
    key: String,
    declaration: Option<Queryable<()>>,
    failed: bool,
    queued: VecDeque<u64>,
}
struct Incoming {
    owner: ExecutionContext,
    caller: ExecutionContext,
    deadline: Instant,
    queryable: u64,
    query: Query,
    payload: Vec<u8>,
    checked_out: bool,
}
struct RunningPermit(Arc<AtomicUsize>);
impl Drop for RunningPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
enum Work {
    Declare {
        handle: u64,
        owner: ExecutionContext,
        key: String,
    },
    Begin {
        handle: u64,
        owner: ExecutionContext,
        key: String,
        point: String,
        payload: Vec<u8>,
        deadline: Instant,
        permit: Arc<RunningPermit>,
    },
    Reply {
        request: Incoming,
        payload: Option<Vec<u8>>,
    },
    Dispose(Disposal),
}
#[derive(Default)]
pub(super) struct Disposal {
    declarations: Vec<Queryable<()>>,
    queries: Vec<Query>,
}
impl Disposal {
    pub(super) fn dispose(self) {
        for query in self.queries {
            let _ = query.reply_err(b"query closed".to_vec()).wait();
        }
        for declaration in self.declarations {
            let _ = declaration.undeclare().wait();
        }
    }
    fn is_empty(&self) -> bool {
        self.declarations.is_empty() && self.queries.is_empty()
    }
}
#[derive(Default)]
pub(super) struct PolledState {
    outgoing: HashMap<u64, Outgoing>,
    declarations: HashMap<u64, Declaration>,
    incoming: HashMap<u64, Incoming>,
    work: VecDeque<Work>,
    wake: Option<SyncSender<()>>,
    running: Arc<AtomicUsize>,
    #[cfg(test)]
    before_work: Option<Box<dyn FnOnce() + Send>>,
}
impl PolledState {
    pub(super) fn declarations_for(&self, owner: &ComponentRef) -> usize {
        self.declarations
            .values()
            .filter(|d| d.owner.principal.component() == *owner)
            .count()
    }
    pub(super) fn duplicate(&self, key: &str, owner: &ComponentRef) -> bool {
        self.declarations
            .values()
            .any(|d| d.key == key && d.owner.principal.component() == *owner)
    }
    pub(super) fn conflicting(&self, key: &str, owner: &ComponentRef) -> bool {
        self.declarations
            .values()
            .any(|d| d.key == key && d.owner.principal.component() != *owner)
    }
    fn room(&self) -> Result<(), Error> {
        if self.work.len() >= MAX_CONTROL_WORK {
            Err(Error::LimitExceeded)
        } else {
            Ok(())
        }
    }
    fn push(&mut self, work: Work) {
        self.work.push_back(work);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }
    fn remove_request(&mut self, handle: u64) -> Option<Incoming> {
        let request = self.incoming.remove(&handle)?;
        if let Some(declaration) = self.declarations.get_mut(&request.queryable) {
            declaration.queued.retain(|id| *id != handle);
        }
        Some(request)
    }
    fn expire(
        &mut self,
        owner: &ExecutionContext,
        contexts: &ExecutionContexts,
    ) -> Result<(), Error> {
        let mut expired = Vec::new();
        for (handle, request) in &self.incoming {
            if request.owner != *owner {
                continue;
            }
            let stale = match contexts.try_validate(&request.caller) {
                Ok(()) => false,
                Err(Error::Inactive) => true,
                Err(error) => return Err(error),
            };
            if stale || request.deadline <= Instant::now() {
                expired.push(*handle);
            }
        }
        if expired.is_empty() {
            return Ok(());
        }
        self.room()?;
        let queries = expired
            .into_iter()
            .filter_map(|id| self.remove_request(id).map(|request| request.query))
            .collect();
        self.push(Work::Dispose(Disposal {
            queries,
            declarations: Vec::new(),
        }));
        Ok(())
    }
    pub(super) fn revoke(
        &mut self,
        instance: &ExtensionInstanceId,
        revoked: &[ComponentRef],
    ) -> Disposal {
        let revoked_owner = |owner: &ExecutionContext| {
            &owner.principal.instance_id == instance
                || revoked.contains(&owner.principal.component())
        };
        self.outgoing
            .retain(|_, query| !revoked_owner(&query.owner));
        let handles: Vec<_> = self
            .incoming
            .iter()
            .filter(|(_, q)| revoked_owner(&q.owner) || revoked_owner(&q.caller))
            .map(|(id, _)| *id)
            .collect();
        let queries = handles
            .into_iter()
            .filter_map(|id| self.remove_request(id).map(|request| request.query))
            .collect();
        let handles: Vec<_> = self
            .declarations
            .iter()
            .filter(|(_, d)| revoked_owner(&d.owner))
            .map(|(id, _)| *id)
            .collect();
        let declarations = handles
            .into_iter()
            .filter_map(|id| self.declarations.remove(&id).and_then(|d| d.declaration))
            .collect();
        Disposal {
            queries,
            declarations,
        }
    }
}
impl Transport {
    fn nonblocking_state(&self, owner: &ExecutionContext) -> Result<MutexGuard<'_, State>, Error> {
        let mut state = self.state.try_lock().map_err(|_| Error::Unavailable)?;
        self.contexts.try_validate(owner)?;
        state.polled.expire(owner, &self.contexts)?;
        Ok(state)
    }
    fn ensure_polled_worker(&self, state: &mut State) -> Result<(), Error> {
        if state.polled.wake.is_some() {
            return Ok(());
        }
        let weak = Arc::downgrade(&self.state);
        let contexts = self.contexts.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("rintawa-zenoh-control".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                while receiver.recv().is_ok() {
                    loop {
                        let Some(shared) = weak.upgrade() else {
                            return;
                        };
                        let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
                        let work = state.polled.work.pop_front();
                        #[cfg(test)]
                        let hook = if work.is_some() {
                            state.polled.before_work.take()
                        } else {
                            None
                        };
                        drop(state);
                        drop(shared);
                        let Some(work) = work else {
                            break;
                        };
                        #[cfg(test)]
                        if let Some(hook) = hook {
                            hook();
                        }
                        run_work(&weak, &contexts, work);
                    }
                }
            })
            .map_err(|_| Error::Unavailable)?;
        state.polled.wake = Some(sender);
        Ok(())
    }
    pub(crate) fn begin_query(
        &self,
        owner: &ExecutionContext,
        point: &str,
        payload: &[u8],
    ) -> Result<u64, Error> {
        clock();
        let deadline = Instant::now() + QUERY_TIMEOUT;
        let key = scoped_key(&owner.scope, point)?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::LimitExceeded);
        }
        let mut state = self.nonblocking_state(owner)?;
        // Sixteen results at 512 KiB each also enforce the 8 MiB owner byte cap.
        if state
            .polled
            .outgoing
            .values()
            .filter(|q| q.owner == *owner)
            .count()
            >= MAX_OWNED_QUERIES
        {
            return Err(Error::LimitExceeded);
        }
        state.polled.room()?;
        self.ensure_polled_worker(&mut state)?;
        state
            .polled
            .running
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_RUNNING_QUERIES).then_some(n + 1)
            })
            .map_err(|_| Error::LimitExceeded)?;
        let permit = Arc::new(RunningPermit(state.polled.running.clone()));
        let handle = handle()?;
        state.polled.outgoing.insert(
            handle,
            Outgoing {
                owner: owner.clone(),
                deadline,
                dispatch_pending: true,
                done: false,
                values: Vec::new(),
                errors: 0,
                count: 0,
                bytes: 0,
                failure: None,
            },
        );
        state.polled.push(Work::Begin {
            handle,
            owner: owner.clone(),
            key,
            point: point.into(),
            payload: payload.to_vec(),
            deadline,
            permit,
        });
        Ok(handle)
    }
    pub(crate) fn poll_query(
        &self,
        owner: &ExecutionContext,
        handle: u64,
    ) -> Result<Option<Vec<Vec<u8>>>, Error> {
        self.take_query_replies(owner, handle, false)
            .map(|replies| replies.map(|replies| replies.into_iter().map(|r| r.payload).collect()))
    }
    pub(crate) fn poll_query_sourced(
        &self,
        owner: &ExecutionContext,
        handle: u64,
    ) -> Result<Option<Vec<SourcedReply>>, Error> {
        self.take_query_replies(owner, handle, true)?
            .map(|replies| {
                replies
                    .into_iter()
                    .map(|reply| {
                        Ok(SourcedReply {
                            provider: reply.provider.ok_or(Error::Unavailable)?,
                            payload: reply.payload,
                        })
                    })
                    .collect()
            })
            .transpose()
    }
    fn take_query_replies(
        &self,
        owner: &ExecutionContext,
        handle: u64,
        verify_sources: bool,
    ) -> Result<Option<Vec<CollectedReply>>, Error> {
        let mut state = match self.nonblocking_state(owner) {
            Ok(state) => state,
            Err(Error::Unavailable | Error::LimitExceeded) => return Ok(None),
            Err(error) => return Err(error),
        };
        let query = state
            .polled
            .outgoing
            .get(&handle)
            .filter(|q| q.owner == *owner)
            .ok_or(Error::InvalidHandle)?;
        if query.failure.is_none()
            && !(query.done && !query.dispatch_pending)
            && Instant::now() < query.deadline
        {
            return Ok(None);
        }
        // Revocation takes the same transport lock before retiring an activation.
        let sources = if verify_sources && query.failure.is_none() {
            query.values.iter().try_for_each(|reply| {
                self.contexts.try_validate(&ExecutionContext {
                    principal: reply.provider.clone().ok_or(Error::Inactive)?,
                    scope: owner.scope.clone(),
                })
            })
        } else {
            Ok(())
        };
        if matches!(sources, Err(Error::Unavailable)) {
            return Ok(None);
        }
        let query = state.polled.outgoing.remove(&handle).unwrap();
        if let Some(error) = query.failure {
            return Err(error);
        }
        if query.values.is_empty() && query.errors > 0 {
            return Err(Error::Unavailable);
        }
        sources.map_err(|_| Error::Unavailable)?;
        Ok(Some(query.values))
    }
    pub(crate) fn close_query(&self, owner: &ExecutionContext, handle: u64) -> Result<(), Error> {
        let mut state = self.nonblocking_state(owner)?;
        if state
            .polled
            .outgoing
            .get(&handle)
            .is_none_or(|q| q.owner != *owner)
        {
            return Err(Error::InvalidHandle);
        }
        state.polled.outgoing.remove(&handle);
        Ok(())
    }
    pub(crate) fn declare_polled(
        &self,
        owner: &ExecutionContext,
        point: &str,
    ) -> Result<u64, Error> {
        let key = scoped_key(&owner.scope, point)?;
        let mut state = self.nonblocking_state(owner)?;
        let component = owner.principal.component();
        if state
            .authoritative
            .get(&(owner.scope.clone(), point.into()))
            .is_some_and(|selected| *selected != component)
        {
            return Err(Error::NotSelected);
        }
        if state
            .queryables
            .iter()
            .any(|d| d.owner == component && d.key == key)
            || state.polled.duplicate(&key, &component)
        {
            return Err(Error::Duplicate);
        }
        if state
            .queryables
            .iter()
            .filter(|d| d.owner == component)
            .count()
            + state.polled.declarations_for(&component)
            >= MAX_DECLARATIONS_PER_OWNER
        {
            return Err(Error::LimitExceeded);
        }
        state.polled.room()?;
        self.ensure_polled_worker(&mut state)?;
        let handle = handle()?;
        state.polled.declarations.insert(
            handle,
            Declaration {
                owner: owner.clone(),
                key: key.clone(),
                declaration: None,
                failed: false,
                queued: VecDeque::new(),
            },
        );
        state.polled.push(Work::Declare {
            handle,
            owner: owner.clone(),
            key,
        });
        Ok(handle)
    }
    pub(crate) fn next_query(
        &self,
        owner: &ExecutionContext,
        queryable: u64,
    ) -> Result<Option<InboundQuery>, Error> {
        let mut state = match self.nonblocking_state(owner) {
            Ok(state) => state,
            Err(Error::Unavailable | Error::LimitExceeded) => return Ok(None),
            Err(error) => return Err(error),
        };
        let declaration = state
            .polled
            .declarations
            .get_mut(&queryable)
            .filter(|d| d.owner == *owner)
            .ok_or(Error::InvalidHandle)?;
        if declaration.failed {
            return Err(Error::Unavailable);
        }
        let Some(id) = declaration.queued.pop_front() else {
            return Ok(None);
        };
        let request = state
            .polled
            .incoming
            .get_mut(&id)
            .ok_or(Error::InvalidHandle)?;
        request.checked_out = true;
        Ok(Some(InboundQuery {
            handle: id,
            caller: request.caller.principal.clone(),
            payload: request.payload.clone(),
        }))
    }
    pub(crate) fn reply_query(
        &self,
        owner: &ExecutionContext,
        handle: u64,
        payload: Option<Vec<u8>>,
    ) -> Result<(), Error> {
        let mut state = self.nonblocking_state(owner)?;
        let request = state
            .polled
            .incoming
            .get(&handle)
            .filter(|q| q.owner == *owner && q.checked_out)
            .ok_or(Error::InvalidHandle)?;
        if payload
            .as_ref()
            .is_some_and(|p| p.len() > MAX_PAYLOAD_BYTES)
        {
            return Err(Error::LimitExceeded);
        }
        self.contexts.try_validate(&request.caller)?;
        state.polled.room()?;
        let request = state.polled.remove_request(handle).unwrap();
        state.polled.push(Work::Reply { request, payload });
        Ok(())
    }
    pub(crate) fn close_queryable(
        &self,
        owner: &ExecutionContext,
        handle: u64,
    ) -> Result<(), Error> {
        let mut state = self.nonblocking_state(owner)?;
        let declaration = state
            .polled
            .declarations
            .get(&handle)
            .filter(|d| d.owner == *owner)
            .ok_or(Error::InvalidHandle)?;
        if declaration.declaration.is_some()
            || state
                .polled
                .incoming
                .values()
                .any(|request| request.queryable == handle)
        {
            state.polled.room()?;
        }
        let declaration = state.polled.declarations.remove(&handle).unwrap();
        let handles: Vec<_> = state
            .polled
            .incoming
            .iter()
            .filter(|(_, q)| q.queryable == handle)
            .map(|(id, _)| *id)
            .collect();
        let queries = handles
            .into_iter()
            .filter_map(|id| state.polled.remove_request(id).map(|q| q.query))
            .collect();
        let disposal = Disposal {
            queries,
            declarations: declaration.declaration.into_iter().collect(),
        };
        if !disposal.is_empty() {
            state.polled.push(Work::Dispose(disposal));
        }
        Ok(())
    }
}

fn run_work(weak: &Weak<std::sync::Mutex<State>>, contexts: &ExecutionContexts, work: Work) {
    match work {
        Work::Dispose(disposal) => disposal.dispose(),
        Work::Reply { request, payload } => {
            if request.deadline > Instant::now()
                && contexts.validate(&request.owner).is_ok()
                && contexts.validate(&request.caller).is_ok()
            {
                match payload {
                    Some(bytes) => {
                        let _ = reply_with_source(&request.query, &request.owner.principal, bytes);
                    }
                    None => {
                        let _ = request
                            .query
                            .reply_err(b"provider rejected".to_vec())
                            .wait();
                    }
                }
            }
        }
        Work::Declare { handle, owner, key } => {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let session = {
                let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
                if !state.polled.declarations.contains_key(&handle)
                    || contexts.validate(&owner).is_err()
                {
                    return;
                }
                session(&mut state)
            };
            let result = session.and_then(|session| {
                let incoming_weak = weak.clone();
                let incoming_contexts = contexts.clone();
                let incoming_owner = owner.clone();
                session
                    .declare_queryable(key)
                    .callback(move |query| {
                        admit_incoming(
                            &incoming_weak,
                            &incoming_contexts,
                            &incoming_owner,
                            handle,
                            query,
                        )
                    })
                    .wait()
                    .map_err(|_| Error::Unavailable)
            });
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(declaration) = state.polled.declarations.get_mut(&handle)
                && contexts.validate(&owner).is_ok()
            {
                match result {
                    Ok(value) => declaration.declaration = Some(value),
                    Err(_) => declaration.failed = true,
                }
                return;
            }
            drop(state); // A retired declaration is dropped off the guest/pump thread.
            drop(result);
        }
        Work::Begin {
            handle,
            owner,
            key,
            point,
            payload,
            deadline,
            permit,
        } => {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let session = {
                let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
                if !state.polled.outgoing.contains_key(&handle) {
                    return;
                }
                if contexts.validate(&owner).is_err() || deadline <= Instant::now() {
                    return;
                }
                if state
                    .authoritative
                    .contains_key(&(owner.scope.clone(), point))
                    && !state.queryables.iter().any(|d| d.key == key)
                    && !state
                        .polled
                        .declarations
                        .values()
                        .any(|d| d.key == key && d.declaration.is_some())
                {
                    Err(Error::Unavailable)
                } else {
                    session(&mut state)
                }
            };
            let result = session.and_then(|session| {
                // Opening the lazy session consumes the original admission deadline too.
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(());
                }
                let replies_weak = weak.clone();
                let dropped_weak = weak.clone();
                let mut held_permit = Some(permit.clone());
                let metadata = metadata(&owner.principal, deadline)?;
                session
                    .get(key)
                    .payload(payload)
                    .attachment(metadata)
                    .target(QueryTarget::All)
                    .consolidation(ConsolidationMode::None)
                    .timeout(remaining)
                    .with(CallbackDrop {
                        callback: move |reply: Reply| collect_reply(&replies_weak, handle, reply),
                        drop: move || {
                            if let Some(shared) = dropped_weak.upgrade()
                                && let Some(query) = shared
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .polled
                                    .outgoing
                                    .get_mut(&handle)
                            {
                                query.done = true;
                            }
                            drop(held_permit.take());
                        },
                    })
                    .wait()
                    .map_err(|_| Error::Unavailable)
            });
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(query) = state.polled.outgoing.get_mut(&handle) {
                query.dispatch_pending = false;
                if let Err(error) = result {
                    query.failure = Some(error);
                    query.done = true;
                }
            }
        }
    }
}
fn collect_reply(weak: &Weak<std::sync::Mutex<State>>, handle: u64, reply: Reply) {
    let Some(shared) = weak.upgrade() else {
        return;
    };
    let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
    let Some(query) = state.polled.outgoing.get_mut(&handle) else {
        return;
    };
    if query.failure.is_some() || Instant::now() > query.deadline {
        return;
    }
    query.count += 1;
    let result = reply.result();
    let length = match result {
        Ok(sample) => sample.payload().len(),
        Err(error) => error.payload().len(),
    };
    query.bytes = query.bytes.saturating_add(length);
    if query.count > MAX_REPLIES
        || query.bytes > MAX_TOTAL_REPLY_BYTES
        || (result.is_ok() && length > MAX_PAYLOAD_BYTES)
    {
        query.failure = Some(Error::LimitExceeded);
        return;
    }
    match result {
        Ok(sample) => query.values.push(CollectedReply {
            provider: reply_source(sample),
            payload: sample.payload().to_bytes().into_owned(),
        }),
        Err(_) => query.errors += 1,
    }
}
fn admit_incoming(
    weak: &Weak<std::sync::Mutex<State>>,
    contexts: &ExecutionContexts,
    owner: &ExecutionContext,
    queryable: u64,
    query: Query,
) {
    let rejected = (|| -> Option<Query> {
        let Some(shared) = weak.upgrade() else {
            return Some(query);
        };
        let (caller, deadline) = match read_metadata(&query) {
            Ok(value) => value,
            Err(_) => return Some(query),
        };
        let caller = ExecutionContext {
            scope: owner.scope.clone(),
            principal: caller,
        };
        let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
        if deadline <= Instant::now()
            || contexts.validate(owner).is_err()
            || contexts.validate(&caller).is_err()
            || !state.polled.declarations.contains_key(&queryable)
        {
            return Some(query);
        }
        // Expiry can be retried by the next owner operation if metadata is busy.
        if state.polled.expire(owner, contexts).is_err() {
            return Some(query);
        }
        // Sixteen bounded incoming messages also enforce the 1 MiB owner cap.
        if query.payload().is_some_and(|p| p.len() > MAX_PAYLOAD_BYTES)
            || state
                .polled
                .incoming
                .values()
                .filter(|q| q.owner == *owner)
                .count()
                >= MAX_OWNED_QUERIES
        {
            return Some(query);
        }
        let handle = match handle() {
            Ok(handle) => handle,
            Err(_) => return Some(query),
        };
        let payload = query
            .payload()
            .map(|p| p.to_bytes().into_owned())
            .unwrap_or_default();
        state
            .polled
            .declarations
            .get_mut(&queryable)
            .unwrap()
            .queued
            .push_back(handle);
        state.polled.incoming.insert(
            handle,
            Incoming {
                owner: owner.clone(),
                caller,
                deadline,
                queryable,
                query,
                payload,
                checked_out: false,
            },
        );
        None
    })();
    if let Some(query) = rejected {
        let _ = query.reply_err(b"polled query unavailable".to_vec()).wait();
    }
}

#[cfg(test)]
mod tests;
