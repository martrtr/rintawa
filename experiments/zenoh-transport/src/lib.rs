//! Isolated Zenoh feasibility probe, not part of the installed Rintawa runtime.
//! A trusted host owns the one Zenoh session. The sandboxed guest only sees WIT
//! imports and never receives a raw session, namespace prefix or caller identity.

#![cfg(test)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use wasmtime::{
    Engine, Store, StoreLimits, StoreLimitsBuilder,
    component::{Component, Linker},
};
use zenoh::{
    Config, Session, Wait,
    query::{ConsolidationMode, Query, QueryTarget},
};

wasmtime::component::bindgen!({
    path: "wit",
    world: "transport-probe",
});

use crate::rintawa::transport_probe::broker;

const MAX_MESSAGE_BYTES: usize = 4096;
const MAX_REPLIES: usize = 16;
const MAX_TOTAL_REPLY_BYTES: usize = 32 * 1024;
const MAX_REGISTRATIONS: usize = 8;
const MAX_IO_CALLS_PER_CALLBACK: usize = 8;
const CALLBACK_FUEL: u64 = 8_000_000;
const QUERY_TIMEOUT: Duration = Duration::from_millis(350);

trait ZenohResultExt<T> {
    fn checked(self) -> Result<T>;
}

impl<T> ZenohResultExt<T> for zenoh::Result<T> {
    fn checked(self) -> Result<T> {
        self.map_err(|error| anyhow!("{error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Principal {
    scope: String,
    extension: String,
    component: String,
}

impl Principal {
    fn trusted_fixture(scope: &str, extension: &str, component: &str) -> Self {
        Self {
            scope: scope.into(),
            extension: extension.into(),
            component: component.into(),
        }
    }
}

// Never trust a guest-supplied key expression: no wildcards, path traversal,
// absolute names or arbitrary host-/other-world prefixes.
fn is_safe_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !name.contains("..")
}

fn private_key(principal: &Principal, name: &str) -> Result<String, broker::Error> {
    if !is_safe_segment(name)
        || !is_safe_segment(&principal.scope)
        || !is_safe_segment(&principal.extension)
        || !is_safe_segment(&principal.component)
    {
        return Err(broker::Error::InvalidName);
    }
    Ok(format!(
        "rintawa/probe/world/{}/ext/{}/{}/{}",
        principal.scope, principal.extension, principal.component, name
    ))
}

fn public_key(principal: &Principal, name: &str) -> Result<String, broker::Error> {
    if !is_safe_segment(name) || !is_safe_segment(&principal.scope) {
        return Err(broker::Error::InvalidName);
    }
    Ok(format!(
        "rintawa/probe/world/{}/public/{}",
        principal.scope, name
    ))
}

fn selected_public_key(
    caller: &Principal,
    selected: &Principal,
    name: &str,
) -> Result<String, broker::Error> {
    if caller != selected {
        return Err(broker::Error::Denied);
    }
    public_key(caller, name)
}

struct GuestState {
    principal: Principal,
    session: Session,
    queryables: Vec<String>,
    subscriptions: Vec<String>,
    limits: StoreLimits,
    outbound_calls: usize,
    active_query: Option<(String, Query)>,
    reply_count: usize,
    reply_bytes: usize,
    active: bool,
}

impl broker::Host for GuestState {
    fn declare_queryable(&mut self, name: String) -> Result<(), broker::Error> {
        if !self.active {
            return Err(broker::Error::Denied);
        }
        let key = private_key(&self.principal, &name)?;
        if self.queryables.len() >= MAX_REGISTRATIONS || self.queryables.contains(&key) {
            return Err(broker::Error::Denied);
        }
        self.queryables.push(key);
        Ok(())
    }

    fn subscribe(&mut self, name: String) -> Result<(), broker::Error> {
        if !self.active {
            return Err(broker::Error::Denied);
        }
        let key = private_key(&self.principal, &name)?;
        if self.subscriptions.len() >= MAX_REGISTRATIONS || self.subscriptions.contains(&key) {
            return Err(broker::Error::Denied);
        }
        self.subscriptions.push(key);
        Ok(())
    }

    fn publish(&mut self, name: String, payload: Vec<u8>) -> Result<(), broker::Error> {
        if !self.active || self.outbound_calls >= MAX_IO_CALLS_PER_CALLBACK {
            return Err(broker::Error::Denied);
        }
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(broker::Error::TooLarge);
        }
        let key = private_key(&self.principal, &name)?;
        self.outbound_calls += 1;
        self.session
            .put(key, payload)
            .wait()
            .map_err(|_| broker::Error::Unavailable)
    }

    fn query_public(
        &mut self,
        name: String,
        payload: Vec<u8>,
    ) -> Result<Vec<Vec<u8>>, broker::Error> {
        if !self.active || self.outbound_calls >= MAX_IO_CALLS_PER_CALLBACK {
            return Err(broker::Error::Denied);
        }
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(broker::Error::TooLarge);
        }
        let key = public_key(&self.principal, &name)?;
        self.outbound_calls += 1;
        query_all(&self.session, &key, &payload).map_err(|_| broker::Error::Unavailable)
    }

    fn reply(&mut self, payload: Vec<u8>) -> Result<(), broker::Error> {
        if !self.active || self.outbound_calls >= MAX_IO_CALLS_PER_CALLBACK {
            return Err(broker::Error::Denied);
        }
        if payload.len() > MAX_MESSAGE_BYTES
            || self.reply_count >= MAX_REPLIES
            || self.reply_bytes.saturating_add(payload.len()) > MAX_TOTAL_REPLY_BYTES
        {
            return Err(broker::Error::TooLarge);
        }
        let (key, query) = self.active_query.as_ref().ok_or(broker::Error::Denied)?;
        let payload_bytes = payload.len();
        query
            .reply(key.as_str(), payload)
            .wait()
            .map_err(|_| broker::Error::Unavailable)?;
        self.reply_count += 1;
        self.reply_bytes += payload_bytes;
        self.outbound_calls += 1;
        Ok(())
    }
}

struct Runner {
    store: Store<GuestState>,
    binding: TransportProbe,
}

struct HostedGuest {
    runner: Arc<Mutex<Runner>>,
    queryables: Vec<String>,
    subscriptions: Vec<String>,
}

fn isolated_session() -> Result<Session> {
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
    .checked()?;
    zenoh::open(config)
        .wait()
        .checked()
        .context("opening isolated in-process Zenoh session")
}

fn wasm_module_path() -> Result<String> {
    std::env::var("RINTAWA_ZENOH_PROBE_GUEST")
        .context("RINTAWA_ZENOH_PROBE_GUEST must name the locally compiled WASM guest")
}

fn start_wasm_guest(
    engine: &Engine,
    component: &Component,
    session: &Session,
    principal: Principal,
) -> Result<HostedGuest> {
    let mut linker = Linker::new(engine);
    TransportProbe::add_to_linker(&mut linker, |state: &mut GuestState| state)?;

    let mut store = Store::new(
        engine,
        GuestState {
            principal,
            session: session.clone(),
            queryables: Vec::new(),
            subscriptions: Vec::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(32 * 1024 * 1024)
                .build(),
            outbound_calls: 0,
            active_query: None,
            reply_count: 0,
            reply_bytes: 0,
            active: true,
        },
    );
    store.limiter(|state| &mut state.limits);
    store.set_fuel(CALLBACK_FUEL)?;
    let binding = TransportProbe::instantiate(&mut store, component, &linker)?;
    binding
        .rintawa_transport_probe_guest()
        .call_start(&mut store)?;
    let declared_queryables = store.data().queryables.clone();
    let declared_subscriptions = store.data().subscriptions.clone();
    let runner = Arc::new(Mutex::new(Runner { store, binding }));
    Ok(HostedGuest {
        runner,
        queryables: declared_queryables,
        subscriptions: declared_subscriptions,
    })
}

fn run_wasm_query(
    runner: &Arc<Mutex<Runner>>,
    payload: &[u8],
    query: Option<(String, Query)>,
) -> Result<Option<Vec<u8>>> {
    if payload.len() > MAX_MESSAGE_BYTES {
        bail!("oversized inbound query");
    }
    let mut runner = runner
        .lock()
        .map_err(|_| anyhow!("WASM runner lock poisoned"))?;
    if !runner.store.data().active {
        bail!("WASM owner inactive");
    }
    let Runner { store, binding } = &mut *runner;
    store.set_fuel(CALLBACK_FUEL)?;
    {
        let state = store.data_mut();
        state.outbound_calls = 0;
        state.reply_count = 0;
        state.reply_bytes = 0;
        state.active_query = query;
    }
    let result = binding
        .rintawa_transport_probe_guest()
        .call_on_query(&mut *store, payload);
    let streamed = store.data().reply_count > 0;
    store.data_mut().active_query = None;
    let response = result?;
    if response.len() > MAX_MESSAGE_BYTES
        || store.data().reply_bytes.saturating_add(response.len()) > MAX_TOTAL_REPLY_BYTES
    {
        bail!("oversized WASM reply");
    }
    if streamed && response.is_empty() {
        Ok(None)
    } else {
        Ok(Some(response))
    }
}

fn deliver_wasm_event(runner: &Arc<Mutex<Runner>>, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_MESSAGE_BYTES {
        bail!("oversized inbound event");
    }
    let mut runner = runner
        .lock()
        .map_err(|_| anyhow!("WASM runner lock poisoned"))?;
    if !runner.store.data().active {
        bail!("WASM owner inactive");
    }
    let Runner { store, binding } = &mut *runner;
    store.data_mut().outbound_calls = 0;
    store.set_fuel(CALLBACK_FUEL)?;
    binding
        .rintawa_transport_probe_guest()
        .call_on_event(store, payload)?;
    Ok(())
}

fn query_all(session: &Session, name: &str, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    if payload.len() > MAX_MESSAGE_BYTES {
        bail!("oversized request");
    }
    let replies = session
        .get(name)
        .payload(payload.to_vec())
        .target(QueryTarget::All)
        .consolidation(ConsolidationMode::None)
        .timeout(QUERY_TIMEOUT)
        .wait()
        .checked()?;
    let mut values = Vec::new();
    let mut received = 0_usize;
    let mut failed = 0_usize;
    let mut total_bytes = 0_usize;
    while let Ok(reply) = replies.recv() {
        received += 1;
        if received > MAX_REPLIES {
            bail!("fan-out count limit exceeded");
        }
        let (payload, is_success) = match reply.result() {
            Ok(sample) => (sample.payload().to_bytes(), true),
            Err(error) => (error.payload().to_bytes(), false),
        };
        total_bytes += payload.len();
        if payload.len() > MAX_MESSAGE_BYTES || total_bytes > MAX_TOTAL_REPLY_BYTES {
            bail!("fan-out byte limit exceeded");
        }
        if is_success {
            values.push(payload.into_owned());
        } else {
            failed += 1;
        }
    }
    if values.is_empty() && failed > 0 {
        bail!("all queried providers returned errors");
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };

    fn call_from_managed_native(
        session: &Session,
        principal: Principal,
        binary_path: &str,
        peer_args: &[&str],
    ) -> Result<String> {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            process::{Command, Stdio},
            sync::mpsc,
            thread,
            time::Instant,
        };

        // Only one executable is mapped. The network namespace is private,
        // and the Host alone owns the session and caller's principal.
        let mut child = Command::new("bwrap")
            .args([
                "--die-with-parent",
                "--unshare-user",
                "--unshare-net",
                "--unshare-pid",
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/bin",
                "/bin",
                "--symlink",
                "usr/lib",
                "/lib",
                "--symlink",
                "usr/lib",
                "/lib64",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--dir",
                "/probe",
                "--ro-bind",
                binary_path,
                "/probe/native-peer",
                "--chdir",
                "/",
                "--clearenv",
                "/probe/native-peer",
            ])
            .args(peer_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("running native process in Bubblewrap network namespace")?;

        let child_stdout = child
            .stdout
            .take()
            .context("native process stdout unavailable")?;
        let mut child_stdin = child
            .stdin
            .take()
            .context("native process stdin unavailable")?;
        let (sender, receiver) = mpsc::channel();

        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(child_stdout);
            for _ in 0..5 {
                let mut line = String::new();
                // Bound every message before it reaches any protocol parser.
                let bytes = (&mut reader).take(256).read_line(&mut line)?;
                if bytes == 0 || !line.ends_with('\n') {
                    anyhow::bail!("native peer ended or sent oversized request");
                }
                sender
                    .send(line)
                    .context("managed native test channel closed")?;
            }
            Ok::<(), anyhow::Error>(())
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        let outcome = (|| -> Result<String> {
            let namespace =
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            let host_namespace = std::fs::read_link("/proc/self/ns/net")?;
            if !namespace.starts_with("NS net:[")
                || namespace.trim() == format!("NS {}", host_namespace.display())
            {
                bail!("native process has no independent network namespace");
            }

            let mut permitted_value = None;
            for _ in 0..4 {
                let request =
                    receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
                let name = request
                    .trim_end()
                    .strip_prefix("GET ")
                    .context("malformed native request")?;
                let answer = match public_key(&principal, name) {
                    Ok(key) => {
                        let replies = query_all(session, &key, b"native-request")?;
                        match replies.as_slice() {
                            [response] => {
                                let text = std::str::from_utf8(response)?;
                                permitted_value = Some(text.to_owned());
                                format!("VALUE {text}")
                            }
                            [] => String::from("EMPTY"),
                            _ => bail!("unexpected public provider count"),
                        }
                    }
                    Err(_) => String::from("DENIED"),
                };
                writeln!(child_stdin, "{answer}")?;
                child_stdin.flush()?;
            }
            permitted_value.context("native peer never reached its scoped provider")
        })();

        if outcome.is_err() {
            let _ = child.kill();
        }
        drop(child_stdin);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(anyhow!("managed native process exceeded deadline"));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(anyhow!("waiting for managed native process: {error}"));
                }
            }
        };
        let reader_result = reader
            .join()
            .map_err(|_| anyhow!("native reader panicked"))?;
        outcome.and_then(|value| {
            let status = status?;
            reader_result?;
            if !status.success() {
                bail!("managed native process exited unsuccessfully: {status}");
            }
            Ok(value)
        })
    }

    #[test]
    fn test_should_isolate_two_real_native_processes_at_host_boundary() -> Result<()> {
        let binary_path = std::env::var("RINTAWA_ZENOH_PROBE_NATIVE_PEER")
            .context("run through experiments/zenoh-transport/run.sh")?;
        let session = isolated_session()?;
        let world_a = Principal::trusted_fixture("world-a", "native", "consumer");
        let world_b = Principal::trusted_fixture("world-b", "native", "consumer");

        let key_a = public_key(&world_a, "controls").map_err(|error| anyhow!("{error:?}"))?;
        let key_b = public_key(&world_b, "controls").map_err(|error| anyhow!("{error:?}"))?;
        let provider_a = session
            .declare_queryable(key_a.as_str())
            .callback({
                let key = key_a.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"dice".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;
        let provider_b = session
            .declare_queryable(key_b.as_str())
            .callback({
                let key = key_b.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"emoji".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;

        assert_eq!(
            call_from_managed_native(&session, world_a, &binary_path, &[])?,
            "dice"
        );
        assert_eq!(
            call_from_managed_native(&session, world_b, &binary_path, &[])?,
            "emoji"
        );

        provider_a.undeclare().wait().checked()?;
        provider_b.undeclare().wait().checked()?;
        assert!(query_all(&session, &key_a, b"probe")?.is_empty());
        assert!(query_all(&session, &key_b, b"probe")?.is_empty());
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_reap_hung_native_peer_and_preserve_world_routing() -> Result<()> {
        let binary_path = std::env::var("RINTAWA_ZENOH_PROBE_NATIVE_PEER")
            .context("run through experiments/zenoh-transport/run.sh")?;
        let session = isolated_session()?;
        let world_a = Principal::trusted_fixture("world-a", "native", "consumer");
        let world_b = Principal::trusted_fixture("world-b", "native", "consumer");
        let key_a = public_key(&world_a, "controls").map_err(|error| anyhow!("{error:?}"))?;
        let key_b = public_key(&world_b, "controls").map_err(|error| anyhow!("{error:?}"))?;
        let provider_a = session
            .declare_queryable(key_a.as_str())
            .callback({
                let key = key_a.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"dice".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;
        let provider_b = session
            .declare_queryable(key_b.as_str())
            .callback({
                let key = key_b.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"emoji".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;

        for mode in ["--hang-before-first-request", "--hang-after-protocol"] {
            let started = Instant::now();
            let peer_session = session.clone();
            let peer_world = world_a.clone();
            let peer_binary = binary_path.clone();
            let native = std::thread::spawn(move || {
                call_from_managed_native(&peer_session, peer_world, &peer_binary, &[mode])
            });
            std::thread::sleep(Duration::from_millis(100));
            let independent_started = Instant::now();
            assert_eq!(
                query_all(&session, &key_b, b"while-native-hung")?,
                vec![b"emoji".to_vec()]
            );
            assert!(independent_started.elapsed() < Duration::from_secs(2));
            let failure = native
                .join()
                .map_err(|_| anyhow!("managed native test thread panicked"))?
                .expect_err("the Host must terminate an unresponsive native peer");
            let diagnostic = format!("{failure:#}");
            assert!(
                diagnostic.contains("timed out") || diagnostic.contains("deadline"),
                "unexpected native failure: {diagnostic}"
            );
            assert!(started.elapsed() < Duration::from_secs(6));
            assert_eq!(
                query_all(&session, &key_b, b"after-native-timeout")?,
                vec![b"emoji".to_vec()]
            );
        }

        assert_eq!(
            call_from_managed_native(&session, world_a, &binary_path, &[])?,
            "dice"
        );
        provider_a.undeclare().wait().checked()?;
        provider_b.undeclare().wait().checked()?;
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_reject_untrusted_names_and_foreign_shared_owner() {
        let world_a = Principal::trusted_fixture("world-a", "example.one", "runtime");
        let world_b = Principal::trusted_fixture("world-b", "example.one", "runtime");
        let impostor = Principal::trusted_fixture("world-a", "example.one", "other");

        assert!(private_key(&world_a, "../world-b/private").is_err());
        let forged = Principal::trusted_fixture("world-a/other", "example.one", "runtime");
        assert!(private_key(&forged, "answer").is_err());
        assert!(private_key(&world_a, "**").is_err());
        assert!(private_key(&world_a, "/host/secret").is_err());
        assert!(private_key(&world_a, "a//b").is_err());
        assert!(private_key(&world_a, "answer").is_ok());
        assert_ne!(
            private_key(&world_a, "answer").ok(),
            private_key(&world_b, "answer").ok()
        );

        assert!(selected_public_key(&impostor, &world_a, "single-role").is_err());
        assert!(selected_public_key(&world_b, &world_a, "single-role").is_err());
        assert!(selected_public_key(&world_a, &world_a, "single-role").is_ok());
    }

    #[test]
    fn test_should_restrict_selected_role_and_contain_bad_provider() -> Result<()> {
        let session = isolated_session()?;
        let selected = Principal::trusted_fixture("world-a", "selected", "runtime");
        let denied = Principal::trusted_fixture("world-a", "unselected", "runtime");
        let key = selected_public_key(&selected, &selected, "primary")
            .map_err(|error| anyhow!("{error:?}"))?;
        assert!(selected_public_key(&denied, &selected, "primary").is_err());
        let primary = session
            .declare_queryable(key.as_str())
            .callback({
                let key = key.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"authoritative".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;
        assert_eq!(
            query_all(&session, &key, b"prompt")?,
            vec![b"authoritative".to_vec()]
        );
        primary.undeclare().wait().checked()?;

        let shared = "rintawa/probe/world/world-a/public/contributions";
        let healthy = session
            .declare_queryable(shared)
            .callback(move |query| {
                let _ = query.reply(shared, b"healthy".to_vec()).wait();
            })
            .wait()
            .checked()?;
        let failed = session
            .declare_queryable(shared)
            .callback(move |query| {
                let _ = query.reply_err("unavailable").wait();
            })
            .wait()
            .checked()?;
        assert_eq!(
            query_all(&session, shared, b"prompt")?,
            vec![b"healthy".to_vec()]
        );
        assert!(query_all(&session, shared, &vec![0; MAX_MESSAGE_BYTES + 1]).is_err());

        failed.undeclare().wait().checked()?;
        healthy.undeclare().wait().checked()?;
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_count_error_replies_toward_fanout_limit() -> Result<()> {
        let session = isolated_session()?;
        let name = "rintawa/probe/world/world-a/public/errors";
        let mut queryables = Vec::new();
        for index in 0..=MAX_REPLIES {
            let response = format!("error-{index}");
            let queryable = session
                .declare_queryable(name)
                .callback(move |query| {
                    let _ = query.reply_err(response.as_str()).wait();
                })
                .wait()
                .checked()?;
            queryables.push(queryable);
        }
        assert!(query_all(&session, name, b"prompt").is_err());
        for queryable in queryables {
            queryable.undeclare().wait().checked()?;
        }
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_reject_aggregate_reply_bytes_before_count_limit() -> Result<()> {
        let session = isolated_session()?;
        let name = "rintawa/probe/world/world-a/public/large-replies";
        let mut queryables = Vec::new();
        for _ in 0..9 {
            let queryable = session
                .declare_queryable(name)
                .callback(move |query| {
                    let _ = query.reply(name, vec![42; MAX_MESSAGE_BYTES]).wait();
                })
                .wait()
                .checked()?;
            queryables.push(queryable);
        }
        assert!(query_all(&session, name, b"prompt").is_err());
        for queryable in queryables {
            queryable.undeclare().wait().checked()?;
        }
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_fan_out_and_release_native_queryables() -> Result<()> {
        let session = isolated_session()?;
        let key = "rintawa/probe/world/fanout/public/controls";
        let a = session
            .declare_queryable(key)
            .callback(move |query| {
                let _ = query.reply(key, b"dice".to_vec()).wait();
            })
            .wait()
            .checked()?;
        let b = session
            .declare_queryable(key)
            .callback(move |query| {
                let _ = query.reply(key, b"emoji".to_vec()).wait();
            })
            .wait()
            .checked()?;

        let start = Instant::now();
        let mut values = query_all(&session, key, b"request")?;
        values.sort();
        assert_eq!(values, vec![b"dice".to_vec(), b"emoji".to_vec()]);
        println!("two-queryable local fan-out latency: {:?}", start.elapsed());

        b.undeclare().wait().checked()?;
        assert_eq!(
            query_all(&session, key, b"request")?,
            vec![b"dice".to_vec()]
        );
        a.undeclare().wait().checked()?;
        assert!(query_all(&session, key, b"request")?.is_empty());
        session.close().wait().checked()?;
        Ok(())
    }

    #[test]
    fn test_should_bridge_native_and_real_wasm_without_cross_world_delivery() -> Result<()> {
        let wasm = std::fs::read(wasm_module_path()?)?;
        let encoded = wit_component::ComponentEncoder::default()
            .module(&wasm)?
            .validate(true)
            .encode()?;
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.wasm_component_model(true);
        wasm_config.consume_fuel(true);
        let engine = Engine::new(&wasm_config)?;
        let component = Component::new(&engine, encoded)?;
        let session = isolated_session()?;

        let principal_a = Principal::trusted_fixture("world-a", "probe", "runtime");
        let principal_b = Principal::trusted_fixture("world-b", "probe", "runtime");
        let HostedGuest {
            runner: guest_a,
            queryables: queries_a,
            subscriptions: subscriptions_a,
        } = start_wasm_guest(&engine, &component, &session, principal_a.clone())?;
        let HostedGuest {
            runner: guest_b,
            queryables: queries_b,
            subscriptions: subscriptions_b,
        } = start_wasm_guest(&engine, &component, &session, principal_b.clone())?;
        assert_eq!(
            queries_a,
            vec![private_key(&principal_a, "answer").unwrap()]
        );
        assert_eq!(
            queries_b,
            vec![private_key(&principal_b, "answer").unwrap()]
        );
        assert_eq!(
            subscriptions_a,
            vec![private_key(&principal_a, "incoming").unwrap()]
        );
        assert_eq!(
            subscriptions_b,
            vec![private_key(&principal_b, "incoming").unwrap()]
        );

        let queryable_a_key = queries_a[0].clone();
        let guest_a_query = guest_a.clone();
        let queryable_a = session
            .declare_queryable(queryable_a_key.clone())
            .callback(move |query| {
                let data = query
                    .payload()
                    .map(|p| p.to_bytes().into_owned())
                    .unwrap_or_default();
                match run_wasm_query(
                    &guest_a_query,
                    &data,
                    Some((queryable_a_key.clone(), query.clone())),
                ) {
                    Ok(Some(response)) => {
                        let _ = query.reply(queryable_a_key.as_str(), response).wait();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = query.reply_err(error.to_string()).wait();
                    }
                }
            })
            .wait()
            .checked()?;

        let queryable_b_key = queries_b[0].clone();
        let guest_b_query = guest_b.clone();
        let queryable_b = session
            .declare_queryable(queryable_b_key.clone())
            .callback(move |query| {
                let data = query
                    .payload()
                    .map(|p| p.to_bytes().into_owned())
                    .unwrap_or_default();
                match run_wasm_query(
                    &guest_b_query,
                    &data,
                    Some((queryable_b_key.clone(), query.clone())),
                ) {
                    Ok(Some(response)) => {
                        let _ = query.reply(queryable_b_key.as_str(), response).wait();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = query.reply_err(error.to_string()).wait();
                    }
                }
            })
            .wait()
            .checked()?;

        let events_a = Arc::new(AtomicUsize::new(0));
        let events_a_callback = events_a.clone();
        let guest_a_events = guest_a.clone();
        let subscriber_a = session
            .declare_subscriber(subscriptions_a[0].as_str())
            .callback(move |sample| {
                let data = sample.payload().to_bytes();
                if deliver_wasm_event(&guest_a_events, data.as_ref()).is_ok() {
                    events_a_callback.fetch_add(1, Ordering::SeqCst);
                }
            })
            .wait()
            .checked()?;
        let events_b = Arc::new(AtomicUsize::new(0));
        let events_b_callback = events_b.clone();
        let guest_b_events = guest_b.clone();
        let subscriber_b = session
            .declare_subscriber(subscriptions_b[0].as_str())
            .callback(move |sample| {
                let data = sample.payload().to_bytes();
                if deliver_wasm_event(&guest_b_events, data.as_ref()).is_ok() {
                    events_b_callback.fetch_add(1, Ordering::SeqCst);
                }
            })
            .wait()
            .checked()?;

        let (sent, received) = std::sync::mpsc::channel();
        let listener = session
            .declare_subscriber(private_key(&principal_a, "processed").unwrap())
            .callback(move |sample| {
                let _ = sent.send(sample.payload().to_bytes().into_owned());
            })
            .wait()
            .checked()?;

        session
            .put(subscriptions_a[0].as_str(), b"hello!".to_vec())
            .wait()
            .checked()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while events_a.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(events_a.load(Ordering::SeqCst), 1);
        assert_eq!(events_b.load(Ordering::SeqCst), 0);

        assert_eq!(
            query_all(&session, &queries_a[0], b"ping")?,
            vec![b"pong:ping:6".to_vec()]
        );
        assert_eq!(
            received.recv_timeout(Duration::from_secs(2))?,
            b"from-wasm".to_vec()
        );
        assert_eq!(
            query_all(&session, &queries_a[0], b"stream")?,
            vec![b"first".to_vec(), b"middle".to_vec(), b"last".to_vec()]
        );
        assert_eq!(
            query_all(&session, &queries_b[0], b"stream")?,
            vec![b"first".to_vec(), b"middle".to_vec(), b"last".to_vec()]
        );
        assert_eq!(
            query_all(&session, &queries_b[0], b"ping")?,
            vec![b"pong:ping:0".to_vec()]
        );

        // A WASM extension consumes two independent native contributions
        // through a single limited public query, without receiving a raw session.
        let controls =
            public_key(&principal_a, "controls").map_err(|error| anyhow!("{error:?}"))?;
        let dice = session
            .declare_queryable(controls.as_str())
            .callback({
                let key = controls.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"dice".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;
        let emoji = session
            .declare_queryable(controls.as_str())
            .callback({
                let key = controls.clone();
                move |query| {
                    let _ = query.reply(key.as_str(), b"emoji".to_vec()).wait();
                }
            })
            .wait()
            .checked()?;
        // A contribution may contain another extension point. The guest, not
        // the transport, interprets the payload and follows the nested name.
        let workspace =
            public_key(&principal_a, "workspace").map_err(|error| anyhow!("{error:?}"))?;
        let workspace_provider = session
            .declare_queryable(workspace.as_str())
            .callback({
                let key = workspace.clone();
                move |query| {
                    let _ = query
                        .reply(key.as_str(), b"chat-ui|controls".to_vec())
                        .wait();
                }
            })
            .wait()
            .checked()?;
        assert_eq!(
            query_all(&session, &queries_a[0], b"nested")?,
            vec![b"nested:2".to_vec()]
        );
        assert_eq!(
            query_all(&session, &queries_b[0], b"nested")?,
            vec![b"nested-empty".to_vec()]
        );
        workspace_provider.undeclare().wait().checked()?;
        assert_eq!(
            query_all(&session, &queries_a[0], b"compose")?,
            vec![b"composed:2".to_vec()]
        );
        assert_eq!(
            query_all(&session, &queries_b[0], b"compose-empty")?,
            vec![b"empty".to_vec()]
        );
        assert_eq!(
            query_all(&session, &queries_a[0], b"overquery")?,
            vec![b"limited".to_vec()]
        );
        emoji.undeclare().wait().checked()?;
        dice.undeclare().wait().checked()?;

        let broken = public_key(&principal_a, "broken").map_err(|error| anyhow!("{error:?}"))?;
        let broken_provider = session
            .declare_queryable(broken.as_str())
            .callback(move |query| {
                let _ = query.reply_err("provider failed").wait();
            })
            .wait()
            .checked()?;
        assert!(query_all(&session, &broken, b"request").is_err());
        assert_eq!(
            query_all(&session, &queries_a[0], b"compose-failed")?,
            vec![b"failure-forwarded".to_vec()]
        );
        broken_provider.undeclare().wait().checked()?;

        // Partial replies and a subsequent provider failure stay distinguishable.
        let stream = session
            .get(queries_a[0].as_str())
            .payload(b"stream-burn".to_vec())
            .target(QueryTarget::All)
            .consolidation(ConsolidationMode::None)
            .timeout(QUERY_TIMEOUT)
            .wait()
            .checked()?;
        let mut successful_chunks = 0;
        let mut failed_chunks = 0;
        for _ in 0..2 {
            let frame = stream.recv().checked()?;
            match frame.result() {
                Ok(sample) => {
                    assert_eq!(sample.payload().to_bytes().as_ref(), b"partial");
                    successful_chunks += 1;
                }
                Err(_) => failed_chunks += 1,
            }
        }
        assert_eq!(successful_chunks, 1);
        assert_eq!(failed_chunks, 1);
        assert_eq!(
            query_all(&session, &queries_b[0], b"ping")?,
            vec![b"pong:ping:0".to_vec()]
        );

        // An infinite guest loop exhausts its own fuel and emits an explicit error.
        assert!(run_wasm_query(&guest_a, &vec![0; MAX_MESSAGE_BYTES + 1], None).is_err());
        let start = Instant::now();
        let failed_replies = session
            .get(queries_a[0].as_str())
            .payload(b"burn".to_vec())
            .target(QueryTarget::All)
            .timeout(QUERY_TIMEOUT)
            .wait()
            .checked()?;
        let failure = failed_replies
            .recv()
            .checked()
            .context("expected WASM budget error reply")?;
        assert!(failure.result().is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(
            query_all(&session, &queries_b[0], b"ping")?,
            vec![b"pong:ping:0".to_vec()]
        );

        // Explicit stop: revoke guest and remove Zenoh declarations.
        guest_a
            .lock()
            .map_err(|_| anyhow!("lock poisoned"))?
            .store
            .data_mut()
            .active = false;
        subscriber_a.undeclare().wait().checked()?;
        queryable_a.undeclare().wait().checked()?;
        listener.undeclare().wait().checked()?;
        assert!(query_all(&session, &queries_a[0], b"ping")?.is_empty());
        assert_eq!(
            query_all(&session, &queries_b[0], b"ping")?,
            vec![b"pong:ping:0".to_vec()]
        );

        guest_b
            .lock()
            .map_err(|_| anyhow!("lock poisoned"))?
            .store
            .data_mut()
            .active = false;
        subscriber_b.undeclare().wait().checked()?;
        queryable_b.undeclare().wait().checked()?;
        assert!(query_all(&session, &queries_b[0], b"ping")?.is_empty());
        session.close().wait().checked()?;
        Ok(())
    }
}
