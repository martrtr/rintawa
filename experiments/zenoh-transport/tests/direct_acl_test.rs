//! Direct native Zenoh ACL comparison: isolated from production Host and SDK.
//! The two-field contract proposal is intentionally not implemented here.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use zenoh::{Config, Wait};

const WORLD_A: &str = "rintawa/probe/direct/world-a/public/answer";
const WORLD_B: &str = "rintawa/probe/direct/world-b/public/answer";

struct AuthFixture {
    path: PathBuf,
    world_a: String,
    world_b: String,
}

impl AuthFixture {
    fn create() -> Result<Self> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rintawa-zenoh-direct-{}-{now}", std::process::id()));
        let mut secrets = [0_u8; 32];
        fs::File::open("/dev/urandom")?.read_exact(&mut secrets)?;
        let world_a = secrets[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let world_b = secrets[16..]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        writeln!(file, "world_a:{world_a}")?;
        writeln!(file, "world_b:{world_b}")?;
        Ok(Self {
            path,
            world_a,
            world_b,
        })
    }
}

impl Drop for AuthFixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn router_with_acl(auth: &AuthFixture, port: u16) -> Result<zenoh::Session> {
    let mut config = Config::from_json5(
        r#"{
        mode: "router",
        listen: {endpoints: []},
        connect: {endpoints: []},
        scouting: {multicast: {enabled: false}, gossip: {enabled: false}}
    }"#,
    )
    .map_err(|error| anyhow!("{error}"))?;
    config
        .insert_json5("listen/endpoints", &format!(r#"["tcp/127.0.0.1:{port}"]"#))
        .map_err(|error| anyhow!("{error}"))?;
    config
        .insert_json5(
            "transport/auth/usrpwd/dictionary_file",
            &format!(r#""{}""#, auth.path.display()),
        )
        .map_err(|error| anyhow!("{error}"))?;
    config
        .insert_json5(
            "access_control",
            r#"{
                enabled: true,
                default_permission: "deny",
                rules: [
                    {
                        id: "world-a-only",
                        permission: "allow",
                        flows: ["ingress", "egress"],
                        messages: ["query", "reply", "declare_queryable", "put", "declare_subscriber"],
                        key_exprs: ["rintawa/probe/direct/world-a/**"]
                    },
                    {
                        id: "world-b-only",
                        permission: "allow",
                        flows: ["ingress", "egress"],
                        messages: ["query", "reply", "declare_queryable", "put", "declare_subscriber"],
                        key_exprs: ["rintawa/probe/direct/world-b/**"]
                    }
                ],
                subjects: [
                    {id: "world-a", usernames: ["world_a"]},
                    {id: "world-b", usernames: ["world_b"]}
                ],
                policies: [
                    {rules: ["world-a-only"], subjects: ["world-a"]},
                    {rules: ["world-b-only"], subjects: ["world-b"]}
                ]
            }"#,
        )
        .map_err(|error| anyhow!("{error}"))?;
    zenoh::open(config)
        .wait()
        .map_err(|error| anyhow!("{error}"))
}

fn managed_direct_peer(
    endpoint: &str,
    username: &str,
    password: &str,
    own_key: &str,
    foreign_key: &str,
    router: &zenoh::Session,
    injection_count: &AtomicUsize,
) -> Result<()> {
    let binary_path = std::env::var("CARGO_BIN_EXE_direct_peer")
        .or_else(|_| std::env::var("RINTAWA_ZENOH_PROBE_DIRECT_PEER"))
        .context("direct_peer binary path missing")?;

    // This direct session deliberately shares the Host network namespace.
    // Filesystem isolation is tested; network reachability is NOT sandboxed.
    let mut child = Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-user",
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
            &binary_path,
            "/probe/direct-peer",
            "--clearenv",
            "/probe/direct-peer",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("launch direct Zenoh client in restricted filesystem")?;
    let mut input = child
        .stdin
        .take()
        .context("direct client input unavailable")?;
    let output = child
        .stdout
        .take()
        .context("direct client output unavailable")?;

    // Credential input never appears in the process arguments/environment.
    writeln!(input, "{endpoint}")?;
    writeln!(input, "{username}")?;
    writeln!(input, "{password}")?;
    writeln!(input, "{own_key}")?;
    writeln!(input, "{foreign_key}")?;
    input.flush()?;

    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || -> Result<()> {
        let mut output = BufReader::new(output);
        let mut ready = String::new();
        let length = (&mut output).take(128).read_line(&mut ready)?;
        if length == 0 || !ready.ends_with('\n') {
            bail!("direct peer exited without valid READY line");
        }
        sender
            .send(ready)
            .context("direct peer result reader detached")?;
        Ok(())
    });
    let outcome = (|| -> Result<()> {
        let ready = receiver.recv_timeout(Duration::from_secs(4))?;
        if ready.trim_end() != format!("READY {username}") {
            bail!("direct peer failed its ACL assertions: {ready}");
        }
        let foreign_replies = router
            .get(foreign_key)
            .timeout(Duration::from_millis(350))
            .wait()
            .map_err(|error| anyhow!("{error}"))?;
        let mut values = Vec::new();
        while let Ok(reply) = foreign_replies.recv() {
            if let Ok(sample) = reply.result() {
                values.push(sample.payload().to_bytes().into_owned());
            }
        }
        if values
            != vec![if username == "world_a" {
                b"world_b".to_vec()
            } else {
                b"world_a".to_vec()
            }]
        {
            bail!("foreign registration escaped ACL: {values:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
        if injection_count.load(Ordering::SeqCst) != 0 {
            bail!("unauthorized publication reached foreign subscriber");
        }
        writeln!(input, "STOP")?;
        input.flush()?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = child.kill();
    }
    drop(input);
    let status = child.wait()?;
    reader
        .join()
        .map_err(|_| anyhow!("direct peer stdout thread panicked"))??;
    outcome?;
    if !status.success() {
        bail!("direct peer exited with {status}");
    }
    Ok(())
}

fn reject_spoofed_credentials(endpoint: &str, username: &str, password: &str) -> Result<()> {
    let config = Config::from_json5(&format!(
        r#"{{
            mode: "client",
            listen: {{endpoints: []}},
            connect: {{
                endpoints: ["{endpoint}"],
                timeout_ms: 700,
                exit_on_failure: true
            }},
            scouting: {{multicast: {{enabled: false}}, gossip: {{enabled: false}}}},
            transport: {{auth: {{usrpwd: {{user: "{username}", password: "{password}"}}}}}}
        }}"#
    ))
    .map_err(|error| anyhow!("{error}"))?;
    match zenoh::open(config).wait() {
        Ok(session) => {
            session.close().wait().map_err(|error| anyhow!("{error}"))?;
            bail!("router accepted a mismatched authenticated username/password");
        }
        Err(_) => Ok(()),
    }
}

#[test]
fn test_should_enforce_default_deny_acl_for_authenticated_direct_peers() -> Result<()> {
    let auth = AuthFixture::create()?;
    let reserve = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reserve.local_addr()?.port();
    drop(reserve);
    let router = router_with_acl(&auth, port)?;
    let endpoint = format!("tcp/127.0.0.1:{port}");
    reject_spoofed_credentials(&endpoint, "world_b", &auth.world_a)?;

    let provider_a = router
        .declare_queryable(WORLD_A)
        .callback(move |query| {
            let _ = query.reply(WORLD_A, b"world_a".to_vec()).wait();
        })
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    let provider_b = router
        .declare_queryable(WORLD_B)
        .callback(move |query| {
            let _ = query.reply(WORLD_B, b"world_b".to_vec()).wait();
        })
        .wait()
        .map_err(|error| anyhow!("{error}"))?;

    let injections_a = Arc::new(AtomicUsize::new(0));
    let count_a = injections_a.clone();
    let subscriber_a = router
        .declare_subscriber(WORLD_A)
        .callback(move |sample| {
            if sample.payload().to_bytes().as_ref() == b"FORGED" {
                count_a.fetch_add(1, Ordering::SeqCst);
            }
        })
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    let injections_b = Arc::new(AtomicUsize::new(0));
    let count_b = injections_b.clone();
    let subscriber_b = router
        .declare_subscriber(WORLD_B)
        .callback(move |sample| {
            if sample.payload().to_bytes().as_ref() == b"FORGED" {
                count_b.fetch_add(1, Ordering::SeqCst);
            }
        })
        .wait()
        .map_err(|error| anyhow!("{error}"))?;

    managed_direct_peer(
        &endpoint,
        "world_a",
        &auth.world_a,
        WORLD_A,
        WORLD_B,
        &router,
        &injections_b,
    )?;
    managed_direct_peer(
        &endpoint,
        "world_b",
        &auth.world_b,
        WORLD_B,
        WORLD_A,
        &router,
        &injections_a,
    )?;

    subscriber_b
        .undeclare()
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    subscriber_a
        .undeclare()
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    provider_b
        .undeclare()
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    provider_a
        .undeclare()
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    router.close().wait().map_err(|error| anyhow!("{error}"))?;
    Ok(())
}
