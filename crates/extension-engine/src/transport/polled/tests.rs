//! D8 semantics through the real local Zenoh session and native Host owners.
use super::*;
use std::time::Duration;
fn context(transport: &Transport, id: &str, scope: &str) -> ExecutionContext {
    let owner = ComponentRef::new(id, "runtime");
    transport
        .contexts
        .begin(owner.clone(), scope.into())
        .unwrap();
    transport.contexts.activate_instance(&owner.instance_id);
    transport.contexts.context(&owner, false).unwrap()
}
fn retry<T>(mut action: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match action() {
            Err(Error::Unavailable) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => return result,
        }
    }
}
fn next(transport: &Transport, owner: &ExecutionContext, queryable: u64) -> InboundQuery {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(query) = retry(|| transport.next_query(owner, queryable)).unwrap() {
            return query;
        }
        assert!(Instant::now() < until, "missing incoming query");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn result(
    transport: &Transport,
    owner: &ExecutionContext,
    handle: u64,
) -> Result<Vec<Vec<u8>>, Error> {
    wait_result(|| transport.poll_query(owner, handle))
}
fn wait_result<T>(mut poll: impl FnMut() -> Result<Option<T>, Error>) -> Result<T, Error> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = poll()? {
            return Ok(result);
        }
        assert!(Instant::now() < until, "query never completed");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn test_declared_then_begun_query_is_ordered_and_replies_keep_attested_caller() -> Result<(), Error>
{
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let declaration = transport.declare_polled(&provider, "fixture/polled")?;
    let outgoing = retry(|| transport.begin_query(&caller, "fixture/polled", b"payload"))?;
    let request = next(&transport, &provider, declaration);
    assert_eq!(request.caller, caller.principal);
    assert_eq!(request.payload, b"payload");
    assert!(transport.poll_query(&caller, outgoing)?.is_none());
    retry(|| transport.reply_query(&provider, request.handle, Some(b"reply".to_vec())))?;
    assert_eq!(
        result(&transport, &caller, outgoing)?,
        vec![b"reply".to_vec()]
    );
    assert!(matches!(
        transport.poll_query(&caller, outgoing),
        Err(Error::InvalidHandle)
    ));
    assert!(matches!(
        transport.reply_query(&provider, request.handle, None),
        Err(Error::InvalidHandle)
    ));
    retry(|| transport.close_queryable(&provider, declaration))?;
    Ok(())
}
#[test]
fn test_partial_success_survives_other_provider_failure_and_close_releases_receivers()
-> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let a = context(&transport, "a", "world");
    let b = context(&transport, "b", "world");
    let da = transport.declare_polled(&a, "fixture/fanout")?;
    let db = retry(|| transport.declare_polled(&b, "fixture/fanout"))?;
    let handle = retry(|| transport.begin_query(&caller, "fixture/fanout", b"go"))?;
    let qa = next(&transport, &a, da);
    let qb = next(&transport, &b, db);
    retry(|| transport.reply_query(&b, qb.handle, None))?;
    retry(|| transport.reply_query(&a, qa.handle, Some(b"success".to_vec())))?;
    assert_eq!(
        result(&transport, &caller, handle)?,
        vec![b"success".to_vec()]
    );
    let handle = retry(|| transport.begin_query(&caller, "fixture/fanout", b"close"))?;
    let qa = next(&transport, &a, da);
    retry(|| transport.close_query(&caller, handle))?;
    retry(|| transport.reply_query(&a, qa.handle, Some(b"admitted effect".to_vec())))?;
    assert!(matches!(
        transport.poll_query(&caller, handle),
        Err(Error::InvalidHandle)
    ));
    retry(|| transport.close_queryable(&a, da))?;
    retry(|| transport.close_queryable(&b, db))?;
    Ok(())
}
#[test]
fn test_foreign_stale_closed_and_expired_handles_do_not_consume_live_work() -> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let foreign = context(&transport, "foreign", "other");
    let declaration = transport.declare_polled(&provider, "fixture/owned")?;
    let handle = retry(|| transport.begin_query(&caller, "fixture/owned", b"go"))?;
    let request = next(&transport, &provider, declaration);
    assert!(matches!(
        transport.reply_query(&foreign, request.handle, None),
        Err(Error::InvalidHandle)
    ));
    assert!(matches!(
        transport.close_queryable(&foreign, declaration),
        Err(Error::InvalidHandle)
    ));
    assert!(matches!(
        transport.close_query(&foreign, handle),
        Err(Error::InvalidHandle)
    ));
    assert!(matches!(
        transport.reply_query(
            &provider,
            request.handle,
            Some(vec![0; MAX_PAYLOAD_BYTES + 1])
        ),
        Err(Error::LimitExceeded)
    ));
    retry(|| transport.reply_query(&provider, request.handle, Some(b"own".to_vec())))?;
    assert_eq!(result(&transport, &caller, handle)?, vec![b"own".to_vec()]);
    let handle = retry(|| transport.begin_query(&caller, "fixture/owned", b"expire"))?;
    let request = next(&transport, &provider, declaration);
    std::thread::sleep(QUERY_TIMEOUT + Duration::from_millis(15));
    assert!(matches!(
        transport.reply_query(&provider, request.handle, None),
        Err(Error::InvalidHandle)
    ));
    let _ = result(&transport, &caller, handle);
    transport.revoke_instance(&provider.principal.instance_id);
    let replacement = context(&transport, "provider", "world");
    assert!(matches!(
        transport.next_query(&replacement, declaration),
        Err(Error::InvalidHandle)
    ));
    let new = retry(|| transport.declare_polled(&replacement, "fixture/owned"))?;
    assert_ne!(new, declaration);
    transport.revoke_activation(&provider);
    assert_eq!(
        transport
            .contexts
            .context(&replacement.principal.component(), false)?,
        replacement
    );
    retry(|| transport.close_queryable(&replacement, new))?;
    Ok(())
}

struct PausedWork {
    release: Option<mpsc::Sender<()>>,
    entered: mpsc::Receiver<()>,
}
impl PausedWork {
    fn install(transport: &Transport) -> Self {
        let (entered_tx, entered) = mpsc::channel();
        let (release, receiver) = mpsc::channel();
        transport.state.lock().unwrap().polled.before_work = Some(Box::new(move || {
            let _ = entered_tx.send(());
            let _ = receiver.recv();
        }));
        Self {
            release: Some(release),
            entered,
        }
    }
    fn wait(&self) {
        self.entered.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}
impl Drop for PausedWork {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
fn wait_for(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(Instant::now() < until, "condition did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn test_deadline_starts_at_admission_before_dispatch_and_survives_metadata_delivery()
-> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let declaration = transport.declare_polled(&provider, "fixture/deadline")?;
    let handle = retry(|| transport.begin_query(&caller, "fixture/deadline", b"original"))?;
    let request = next(&transport, &provider, declaration);
    {
        let state = transport.state.lock().unwrap();
        assert_eq!(
            state.polled.outgoing[&handle].deadline,
            state.polled.incoming[&request.handle].deadline
        );
    }
    retry(|| transport.reply_query(&provider, request.handle, Some(b"ok".to_vec())))?;
    result(&transport, &caller, handle)?;
    let pause = PausedWork::install(&transport);
    let expired = retry(|| transport.begin_query(&caller, "fixture/deadline", b"never dispatch"))?;
    pause.wait();
    std::thread::sleep(QUERY_TIMEOUT + Duration::from_millis(10));
    assert_eq!(result(&transport, &caller, expired)?, Vec::<Vec<u8>>::new());
    drop(pause);
    wait_for(|| {
        transport
            .state
            .lock()
            .unwrap()
            .polled
            .running
            .load(Ordering::Acquire)
            == 0
    });
    assert!(retry(|| transport.next_query(&provider, declaration))?.is_none());
    retry(|| transport.close_queryable(&provider, declaration))?;
    Ok(())
}
#[test]
fn test_global_running_slots_survive_close_and_release_only_after_actual_work_finishes()
-> Result<(), Error> {
    let transport = Transport::default();
    let owners: Vec<_> = (0..5)
        .map(|n| context(&transport, &format!("caller-{n}"), "world"))
        .collect();
    let pause = PausedWork::install(&transport);
    let first = transport.begin_query(&owners[0], "fixture/absent", b"first")?;
    pause.wait();
    let mut handles = vec![(0, first)];
    for n in 1..64 {
        let owner = n / 16;
        handles.push((
            owner,
            transport.begin_query(&owners[owner], "fixture/absent", b"queued")?,
        ));
    }
    assert!(matches!(
        transport.begin_query(&owners[4], "fixture/absent", b"overflow"),
        Err(Error::LimitExceeded)
    ));
    for (owner, handle) in handles {
        transport.close_query(&owners[owner], handle)?;
    }
    assert_eq!(transport.state.lock().unwrap().polled.outgoing.len(), 0);
    assert_eq!(
        transport
            .state
            .lock()
            .unwrap()
            .polled
            .running
            .load(Ordering::Acquire),
        64
    );
    assert!(matches!(
        transport.begin_query(&owners[0], "fixture/absent", b"closed still running"),
        Err(Error::LimitExceeded)
    ));
    drop(pause);
    wait_for(|| {
        transport
            .state
            .lock()
            .unwrap()
            .polled
            .running
            .load(Ordering::Acquire)
            == 0
    });
    let fresh = retry(|| transport.begin_query(&owners[0], "fixture/absent", b"recovered"))?;
    assert!(result(&transport, &owners[0], fresh)?.is_empty());
    Ok(())
}
#[test]
fn test_completed_uncollected_queries_and_checked_out_incoming_requests_keep_owner_quota()
-> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let other = context(&transport, "other", "world");
    let declaration = transport.declare_polled(&provider, "fixture/quota")?;
    let mut outgoing = Vec::new();
    for _ in 0..16 {
        outgoing.push(retry(|| {
            transport.begin_query(&caller, "fixture/quota", b"go")
        })?);
    }
    let mut checked = Vec::new();
    for _ in 0..16 {
        checked.push(next(&transport, &provider, declaration));
    }
    assert!(matches!(
        transport.begin_query(&caller, "fixture/quota", b"17"),
        Err(Error::LimitExceeded)
    ));
    let excess = retry(|| transport.begin_query(&other, "fixture/quota", b"incoming 17"))?;
    assert!(matches!(
        result(&transport, &other, excess),
        Err(Error::Unavailable)
    ));
    for request in checked {
        retry(|| transport.reply_query(&provider, request.handle, Some(b"reply".to_vec())))?;
    }
    wait_for(|| {
        let state = transport.state.lock().unwrap();
        state
            .polled
            .outgoing
            .values()
            .filter(|q| q.owner == caller)
            .all(|q| q.done && !q.dispatch_pending)
    });
    assert!(matches!(
        transport.begin_query(&caller, "fixture/quota", b"uncollected 17"),
        Err(Error::LimitExceeded)
    ));
    assert_eq!(
        result(&transport, &caller, outgoing.remove(0))?,
        vec![b"reply".to_vec()]
    );
    let fresh = retry(|| transport.begin_query(&caller, "fixture/quota", b"now fits"))?;
    retry(|| transport.close_query(&caller, fresh))?;
    for handle in outgoing {
        result(&transport, &caller, handle)?;
    }
    retry(|| transport.close_queryable(&provider, declaration))?;
    Ok(())
}
#[test]
fn test_callback_and_polled_declarations_share_quota_and_authoritative_selection()
-> Result<(), Error> {
    let transport = Transport::default();
    let a = context(&transport, "a", "world");
    let b = context(&transport, "b", "world");
    transport.select_authoritative(&a.scope, "fixture/authority", &a.principal.component())?;
    assert!(matches!(
        transport.declare_polled(&b, "fixture/authority"),
        Err(Error::NotSelected)
    ));
    let declaration = transport.declare_polled(&a, "fixture/authority")?;
    assert!(matches!(
        transport.select_authoritative(&a.scope, "fixture/authority", &b.principal.component()),
        Err(Error::ActiveConflict)
    ));
    for n in 0..15 {
        transport.declare(
            &a.principal.component(),
            &a.scope,
            &format!("fixture/callback-{n}"),
            |_, _| {},
        )?;
    }
    // Publication runs concurrently; an unavailable metadata lock is a permitted
    // transient result, not evidence that the shared declaration quota changed.
    let over_quota = retry(|| transport.declare_polled(&a, "fixture/17"));
    assert!(
        matches!(over_quota, Err(Error::LimitExceeded)),
        "{over_quota:?}"
    );
    assert!(matches!(
        transport.declare(
            &a.principal.component(),
            &a.scope,
            "fixture/also-17",
            |_, _| {}
        ),
        Err(Error::LimitExceeded)
    ));
    retry(|| transport.close_queryable(&a, declaration))?;
    transport.declare(
        &a.principal.component(),
        &a.scope,
        "fixture/freed",
        |_, _| {},
    )?;
    transport.revoke_instance(&a.principal.instance_id);
    Ok(())
}
#[test]
fn test_sourced_poll_attests_two_responders_and_shares_raw_handle_ownership() -> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let a = context(&transport, "a", "world");
    let b = context(&transport, "b", "world");
    let foreign = context(&transport, "foreign", "other");
    let point = "fixture/sourced";
    let da = transport.declare_polled(&a, point)?;
    let db = retry(|| transport.declare_polled(&b, point))?;
    let handle = retry(|| transport.begin_query(&caller, point, b"go"))?;
    let qa = next(&transport, &a, da);
    let qb = next(&transport, &b, db);
    for other in [&a, &foreign] {
        assert!(matches!(
            retry(|| transport.poll_query_sourced(other, handle)),
            Err(Error::InvalidHandle)
        ));
    }
    assert!(transport.poll_query(&caller, handle)?.is_none());
    let claimed_provider = serde_json::to_vec(&b.principal).unwrap();
    retry(|| transport.reply_query(&a, qa.handle, Some(claimed_provider.clone())))?;
    retry(|| transport.reply_query(&b, qb.handle, Some(b"legitimate".to_vec())))?;
    let replies = wait_result(|| transport.poll_query_sourced(&caller, handle))?;
    assert_eq!(replies.len(), 2);
    assert!(
        replies
            .iter()
            .any(|reply| { reply.provider == a.principal && reply.payload == claimed_provider })
    );
    assert!(
        replies
            .iter()
            .any(|reply| { reply.provider == b.principal && reply.payload == b"legitimate" })
    );
    assert!(matches!(
        transport.poll_query(&caller, handle),
        Err(Error::InvalidHandle)
    ));
    Ok(())
}

#[test]
fn test_sourced_poll_rejects_missing_malformed_oversized_and_foreign_sources() -> Result<(), Error>
{
    use crate::transport::{MAX_REPLY_SOURCE_BYTES, REPLY_SOURCE_PREFIX};

    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let native = context(&transport, "native-provider", "world");
    let foreign = context(&transport, "foreign-provider", "other");
    let point = "fixture/invalid-source";
    let payload = serde_json::to_vec(&native.principal).unwrap();
    let callback_payload = payload.clone();
    transport.declare(
        &native.principal.component(),
        &native.scope,
        point,
        move |query, _| {
            let reply = query.reply(query.key_expr().as_str(), callback_payload.clone());
            match query.payload().unwrap().to_bytes().as_ref() {
                b"missing" => reply.wait(),
                b"malformed" => reply
                    .attachment([REPLY_SOURCE_PREFIX, b"{}"].concat())
                    .wait(),
                b"oversized" => {
                    let mut source = [REPLY_SOURCE_PREFIX, &callback_payload].concat();
                    source.resize(MAX_REPLY_SOURCE_BYTES + 1, b' ');
                    reply.attachment(source).wait()
                }
                _ => reply_with_source(&query, &foreign.principal, callback_payload.clone()),
            }
            .unwrap();
        },
    )?;
    for request in [
        b"missing".as_slice(),
        b"malformed",
        b"oversized",
        b"foreign",
    ] {
        let handle = retry(|| transport.begin_query(&caller, point, request))?;
        assert!(matches!(
            wait_result(|| transport.poll_query_sourced(&caller, handle)),
            Err(Error::Unavailable)
        ));
        assert!(matches!(
            transport.poll_query(&caller, handle),
            Err(Error::InvalidHandle)
        ));
        let raw = retry(|| transport.begin_query(&caller, point, request))?;
        assert_eq!(result(&transport, &caller, raw)?, vec![payload.clone()]);
        assert!(matches!(
            transport.poll_query_sourced(&caller, raw),
            Err(Error::InvalidHandle)
        ));
    }
    Ok(())
}

#[test]
fn test_sourced_poll_checks_activation_after_reply_collection() -> Result<(), Error> {
    for replace in [false, true] {
        let transport = Transport::default();
        let caller = context(&transport, "caller", "world");
        let provider = context(&transport, "provider", "world");
        let point = "fixture/retired-source";
        let declaration = transport.declare_polled(&provider, point)?;
        let handle = retry(|| transport.begin_query(&caller, point, b"go"))?;
        let request = next(&transport, &provider, declaration);
        retry(|| transport.reply_query(&provider, request.handle, Some(b"old".to_vec())))?;
        wait_for(|| {
            let state = transport.state.lock().unwrap();
            let query = &state.polled.outgoing[&handle];
            query.done && query.values.len() == 1
        });
        transport.revoke_activation(&provider);
        if replace {
            let replacement = context(&transport, "provider", "world");
            assert_ne!(
                provider.principal.activation,
                replacement.principal.activation
            );
        }
        assert!(matches!(
            wait_result(|| transport.poll_query_sourced(&caller, handle)),
            Err(Error::Unavailable)
        ));
        assert!(matches!(
            transport.poll_query(&caller, handle),
            Err(Error::InvalidHandle)
        ));
    }
    Ok(())
}

#[test]
fn test_all_eight_imports_return_promptly_while_transport_work_is_paused() -> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let declaration = transport.declare_polled(&provider, "fixture/prompt")?;
    let existing = retry(|| transport.begin_query(&caller, "fixture/prompt", b"go"))?;
    let request = next(&transport, &provider, declaration);
    let pause = PausedWork::install(&transport);
    let trigger = retry(|| transport.begin_query(&caller, "fixture/other", b"pause"))?;
    pause.wait();
    let before = Instant::now();
    let handle = transport.begin_query(&caller, "fixture/prompt", b"begin")?;
    assert!(transport.poll_query(&caller, handle)?.is_none());
    assert!(transport.poll_query_sourced(&caller, handle)?.is_none());
    transport.close_query(&caller, handle)?;
    let pending = transport.declare_polled(&provider, "fixture/pending")?;
    assert!(transport.next_query(&provider, pending)?.is_none());
    transport.reply_query(&provider, request.handle, Some(b"ok".to_vec()))?;
    transport.close_queryable(&provider, declaration)?;
    assert!(
        before.elapsed() < Duration::from_millis(100),
        "guest imports waited for transport work"
    );
    drop(pause);
    assert_eq!(result(&transport, &caller, existing)?, vec![b"ok".to_vec()]);
    let _ = result(&transport, &caller, trigger)?;
    retry(|| transport.close_queryable(&provider, pending))?;
    Ok(())
}
#[test]
fn test_result_limits_reject_oversized_frames_and_aggregate_without_truncation() -> Result<(), Error>
{
    for (count, length) in [(1, MAX_PAYLOAD_BYTES + 1), (9, MAX_PAYLOAD_BYTES), (33, 1)] {
        let transport = Transport::default();
        let caller = context(&transport, "caller", "world");
        let provider = context(&transport, "provider", "world");
        transport.declare(
            &provider.principal.component(),
            &provider.scope,
            "fixture/limits",
            move |query, _| {
                for _ in 0..count {
                    let _ = query
                        .reply(query.key_expr().as_str(), vec![1; length])
                        .wait();
                }
            },
        )?;
        let handle = transport.begin_query(&caller, "fixture/limits", b"go")?;
        let actual = result(&transport, &caller, handle);
        assert!(
            matches!(actual, Err(Error::LimitExceeded)),
            "count={count} length={length} actual={actual:?}"
        );
        assert!(matches!(
            transport.poll_query(&caller, handle),
            Err(Error::InvalidHandle)
        ));
    }
    Ok(())
}

#[test]
fn test_caller_stop_and_queryable_close_revoke_queued_and_checked_out_routes() -> Result<(), Error>
{
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    let declaration = transport.declare_polled(&provider, "fixture/revoke")?;
    let first = retry(|| transport.begin_query(&caller, "fixture/revoke", b"checked out"))?;
    let request = next(&transport, &provider, declaration);
    transport.revoke_instance(&caller.principal.instance_id);
    assert!(matches!(
        transport.reply_query(&provider, request.handle, Some(b"stale".to_vec())),
        Err(Error::InvalidHandle)
    ));
    let replacement = context(&transport, "caller", "world");
    assert!(matches!(
        transport.poll_query(&replacement, first),
        Err(Error::InvalidHandle)
    ));
    let first = retry(|| transport.begin_query(&replacement, "fixture/revoke", b"checked out"))?;
    let request = next(&transport, &provider, declaration);
    let second = retry(|| transport.begin_query(&replacement, "fixture/revoke", b"queued"))?;
    wait_for(|| transport.state.lock().unwrap().polled.incoming.len() == 2);
    retry(|| transport.close_queryable(&provider, declaration))?;
    assert!(matches!(
        transport.reply_query(&provider, request.handle, Some(b"late".to_vec())),
        Err(Error::InvalidHandle)
    ));
    assert!(matches!(
        result(&transport, &replacement, first),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        result(&transport, &replacement, second),
        Err(Error::Unavailable)
    ));
    let fresh = retry(|| transport.declare_polled(&provider, "fixture/revoke"))?;
    assert_ne!(fresh, declaration);
    retry(|| transport.close_queryable(&provider, fresh))?;
    Ok(())
}
#[test]
fn test_reply_close_race_does_not_consume_replacement_or_leak_routes() -> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let provider = context(&transport, "provider", "world");
    for _ in 0..12 {
        let declaration = retry(|| transport.declare_polled(&provider, "fixture/race"))?;
        let outgoing = retry(|| transport.begin_query(&caller, "fixture/race", b"race"))?;
        let request = next(&transport, &provider, declaration);
        let responder = transport.clone();
        let executing = provider.clone();
        let reply = std::thread::spawn(move || {
            retry(|| responder.reply_query(&executing, request.handle, Some(b"won".to_vec())))
        });
        retry(|| transport.close_queryable(&provider, declaration))?;
        assert!(matches!(
            reply.join().unwrap(),
            Ok(()) | Err(Error::InvalidHandle)
        ));
        let outcome = result(&transport, &caller, outgoing);
        assert!(matches!(outcome, Ok(_) | Err(Error::Unavailable)));
        assert!(transport.state.lock().unwrap().polled.incoming.is_empty());
    }
    wait_for(|| {
        transport
            .state
            .lock()
            .unwrap()
            .polled
            .running
            .load(Ordering::Acquire)
            == 0
    });
    Ok(())
}
#[test]
fn test_publication_failure_is_observable_and_its_handle_can_be_closed() -> Result<(), Error> {
    let transport = Transport::default();
    let provider = context(&transport, "provider", "world");
    let session = {
        let mut state = transport.state.lock().unwrap();
        session(&mut state)?
    };
    session.close().wait().map_err(|_| Error::Unavailable)?;
    let declaration = transport.declare_polled(&provider, "fixture/failure")?;
    wait_for(|| transport.state.lock().unwrap().polled.declarations[&declaration].failed);
    wait_for(|| {
        matches!(
            transport.next_query(&provider, declaration),
            Err(Error::Unavailable)
        )
    });
    retry(|| transport.close_queryable(&provider, declaration))?;
    assert_eq!(
        transport
            .state
            .lock()
            .unwrap()
            .polled
            .declarations_for(&provider.principal.component()),
        0
    );
    Ok(())
}
#[test]
fn test_successful_replies_are_returned_at_deadline_despite_silent_provider() -> Result<(), Error> {
    let transport = Transport::default();
    let caller = context(&transport, "caller", "world");
    let fast = context(&transport, "fast", "world");
    let silent = context(&transport, "silent", "world");
    transport.declare(
        &fast.principal.component(),
        &fast.scope,
        "fixture/partial",
        |query, _| {
            let _ = query
                .reply(query.key_expr().as_str(), b"success".to_vec())
                .wait();
        },
    )?;
    let declaration = transport.declare_polled(&silent, "fixture/partial")?;
    let handle = retry(|| transport.begin_query(&caller, "fixture/partial", b"go"))?;
    let _held = next(&transport, &silent, declaration);
    assert_eq!(
        result(&transport, &caller, handle)?,
        vec![b"success".to_vec()]
    );
    retry(|| transport.close_queryable(&silent, declaration))?;
    Ok(())
}

#[test]
fn next_query_reports_pending_during_native_control_contention() {
    let transport = Transport::default();
    let provider = context(&transport, "busy-provider", "world");
    let declaration = retry(|| transport.declare_polled(&provider, "fixture/busy")).unwrap();
    let guard = transport.state.lock().unwrap();
    assert!(matches!(
        transport.next_query(&provider, declaration),
        Ok(None)
    ));
    drop(guard);
    assert!(matches!(
        retry(|| transport.next_query(&provider, declaration)),
        Ok(None)
    ));
    retry(|| transport.close_queryable(&provider, declaration)).unwrap();
}
