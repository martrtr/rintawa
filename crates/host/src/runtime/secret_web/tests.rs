//! Worker admission, write-only results, prompt ownership and revocation.
use super::*;
use crate::{HOST_SCOPE, HostRuntime, SecretBackend};
use rintawa_extension_engine::{LivePolicyChange, LivePolicyDelta};
use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, types::RuntimeScopeId,
};
const WASM: &str =
    include_str!("../../../../extension-engine/tests/fixtures/stateful_component.wat");
struct Fixture {
    _root: tempfile::TempDir,
    home: crate::HostHome,
    runtime: HostRuntime,
    requester: LivePolicyTarget,
}

fn fixture() -> anyhow::Result<Fixture> {
    let root = tempfile::tempdir()?;
    let home = crate::HostHome::open(root.path().join("home"))?;
    home.select_secret_backend(SecretBackend::LocalFile)?;
    let source = root.path().join("source");
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    std::fs::write(source.join("component.wasm"), WASM)?;
    std::fs::write(
        source.join("manifest.toml"),
        r#"id = "fixture.native-secrets"
name = "Native secret fixture"
version = "0.0.1"
sdk = "^0.0"
[[components]]
id = "settings"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "component.wasm"
[components.permissions]
runtime = ["secret-prompt", "secret-manage"]
[[components]]
id = "backend"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "component.wasm"
[components.permissions]
secret-read = ["mail.*"]
"#,
    )?;
    let package = root.path().join("fixture.rtw");
    rintawa_artifacts::pack_directory(&source, &package, rintawa_artifacts::RtwLimits::default())?;
    let installed = home.install_local_rtw(&package, None)?.activation;
    let caller = ComponentRef::new(installed.instance_id, "settings");
    home.grant_runtime_permission(
        RuntimeScopeId::new(HOST_SCOPE),
        caller.clone(),
        RuntimePermission::SecretPrompt,
    )?;
    home.grant_runtime_permission(
        RuntimeScopeId::new(HOST_SCOPE),
        caller.clone(),
        RuntimePermission::SecretManage,
    )?;
    let runtime = HostRuntime::start(&home)?;
    let requester = runtime
        .secret_consumer_target(&RuntimeScopeId::new(HOST_SCOPE), &caller)
        .unwrap();
    Ok(Fixture {
        _root: root,
        home,
        runtime,
        requester,
    })
}
fn access(f: &Fixture) -> Arc<LocalHostAccess> {
    f.runtime.host_access.clone().unwrap()
}
fn wait(f: &Fixture, handle: u64) -> Value {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let value: Value =
            serde_json::from_str(&access(f).status(&f.requester, handle).unwrap()).unwrap();
        if value["status"] != "pending" {
            return value;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(3));
    }
}
fn begin(f: &Fixture, value: Value) -> u64 {
    access(f)
        .begin(
            &f.requester,
            &value.to_string(),
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
}
fn prompt(f: &Fixture) -> u64 {
    let authority = access(f)
        .policy_control()
        .unwrap()
        .admit_secret_prompt(
            &f.requester,
            &"backend".into(),
            SecretPath::parse("mail.password").unwrap(),
        )
        .unwrap();
    access(f)
        .begin(
            &f.requester,
            "{}",
            Some(authority),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
}
#[test]
fn create_is_unbound_metadata_only_then_prompt_binds_once_and_rotation_retains_identity()
-> anyhow::Result<()> {
    let f = fixture()?;
    let sentinel = "synthetic web sentinel\r\n多行\n";
    let h = begin(
        &f,
        json!({"operation":"create","label":"shared","value":sentinel}),
    );
    let result = wait(&f, h);
    assert_eq!(result["status"], "completed");
    assert!(!result.to_string().contains(sentinel));
    assert!(f.home.list_secret_consumers()?.is_empty());
    let id = result["result"]["id"].as_str().unwrap();
    let p = prompt(&f);
    let h = begin(
        &f,
        json!({"operation":"inspect-prompt","prompt":p.to_string()}),
    );
    let inspected = wait(&f, h);
    assert_eq!(inspected["result"]["target"]["componentId"], "backend");
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":id}),
    );
    assert_eq!(wait(&f, h)["status"], "completed");
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":id}),
    );
    assert_eq!(wait(&f, h)["status"], "failed");
    assert_eq!(f.home.list_secret_consumers()?.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&access(&f).status(&f.requester, p).unwrap())?["result"]["outcome"],
        "bound"
    );
    let h = begin(
        &f,
        json!({"operation":"replace","id":id,"revision":1,"value":"rotated"}),
    );
    assert_eq!(wait(&f, h)["status"], "completed");
    let h = begin(&f, json!({"operation":"delete","id":id,"revision":1}));
    assert_eq!(wait(&f, h)["code"], "revision-conflict");
    Ok(())
}
#[test]
fn prompt_cancel_and_permission_revoke_prevent_binding_and_foreign_cleanup_is_denied()
-> anyhow::Result<()> {
    let f = fixture()?;
    let entry = f
        .home
        .create_shared_secret("shared", &SecretValue::new("fake"))?;
    let p = prompt(&f);
    let mut other = f.requester.clone();
    other.activation = "999999".into();
    assert_eq!(
        access(&f).close(&other, p, true),
        Err(HostAccessError::NotFound)
    );
    access(&f).close(&f.requester, p, false).unwrap();
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":entry.id}),
    );
    assert_eq!(wait(&f, h)["status"], "failed");
    access(&f).close(&f.requester, p, true).unwrap();
    let p = prompt(&f);
    access(&f)
        .apply_policy(&LivePolicyChange {
            target: f.requester.clone(),
            delta: LivePolicyDelta {
                revoke_runtime: vec![RuntimePermission::SecretPrompt],
                ..Default::default()
            },
        })
        .unwrap();
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":entry.id}),
    );
    assert_eq!(wait(&f, h)["status"], "failed");
    assert!(f.home.list_secret_consumers()?.is_empty());
    access(&f).close(&f.requester, p, true).unwrap();
    Ok(())
}
#[test]
fn admin_permission_is_revalidated_and_cleanup_does_not_need_permission() -> anyhow::Result<()> {
    let f = fixture()?;
    access(&f)
        .apply_policy(&LivePolicyChange {
            target: f.requester.clone(),
            delta: LivePolicyDelta {
                revoke_runtime: vec![RuntimePermission::SecretManage],
                ..Default::default()
            },
        })
        .unwrap();
    let h = begin(
        &f,
        json!({"operation":"create","label":"forbidden","value":"fake"}),
    );
    assert_eq!(wait(&f, h)["code"], "stale");
    assert!(f.home.list_shared_secrets()?.is_empty());
    access(&f).close(&f.requester, h, true).unwrap();
    Ok(())
}
#[test]
fn retained_limits_and_strict_envelopes_are_checked_before_side_effects() -> anyhow::Result<()> {
    let f = fixture()?;
    for _ in 0..8 {
        let h = begin(&f, json!({"operation":"list","offset":0}));
        wait(&f, h);
    }
    assert_eq!(
        access(&f).begin(
            &f.requester,
            "{\"operation\":\"list\",\"offset\":0}",
            None,
            Arc::new(AtomicBool::new(false))
        ),
        Err(HostAccessError::QueueFull)
    );
    assert_eq!(
        access(&f).begin(
            &f.requester,
            "{\"operation\":\"list\",\"offset\":0,\"owner\":\"shell\"}",
            None,
            Arc::new(AtomicBool::new(false))
        ),
        Err(HostAccessError::InvalidRequest)
    );
    assert!(f.home.list_shared_secrets()?.is_empty());
    Ok(())
}

#[test]
fn release_during_blocked_admitted_io_is_nonblocking_and_stays_charged() -> anyhow::Result<()> {
    use rintawa_extension_engine::SecretVault;
    use rintawa_sdk::secrets::SecretAccessError;
    use std::sync::mpsc;
    struct Held {
        entered: mpsc::SyncSender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
    }
    impl SecretVault for Held {
        fn store(&self, _: &SecretPath, _: &SecretValue) -> Result<(), SecretAccessError> {
            self.entered.send(()).unwrap();
            self.resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            Ok(())
        }
        fn load(&self, _: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
            Ok(None)
        }
        fn delete(&self, _: &SecretPath) -> Result<bool, SecretAccessError> {
            Ok(true)
        }
    }
    let f = fixture()?;
    let current = access(&f);
    let (entered, arrived) = mpsc::sync_channel(1);
    let (resume, blocked) = mpsc::channel();
    let native = Arc::new(LocalHostAccess {
        secrets: Some(crate::shared_secrets::ManagedSecrets::test_system(
            f.home.root().to_owned(),
            Arc::new(Held {
                entered,
                resume: Mutex::new(blocked),
            }),
        )),
        live_policy: Mutex::new(Some(current.policy_control().unwrap())),
        ..LocalHostAccess::new(
            f.home.root(),
            current.user_resources.clone(),
            current.world_sessions.clone(),
            current.world_data.clone(),
        )
    });
    let h = native
        .clone()
        .begin(
            &f.requester,
            &json!({"operation":"create","label":"admitted","value":"fake blocked sentinel"})
                .to_string(),
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    arrived.recv_timeout(Duration::from_secs(5))?;
    let started = Instant::now();
    native.close(&f.requester, h, true).unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    for _ in 0..7 {
        native
            .clone()
            .begin(
                &f.requester,
                "{\"operation\":\"list\",\"offset\":0}",
                None,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
    }
    assert_eq!(
        native.clone().begin(
            &f.requester,
            "{\"operation\":\"list\",\"offset\":0}",
            None,
            Arc::new(AtomicBool::new(false))
        ),
        Err(HostAccessError::QueueFull)
    );
    resume.send(())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if native.secret_operations.0.lock().unwrap().entries[&h]
            .result
            .lock()
            .unwrap()
            .is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        native.secrets.as_ref().unwrap().list().unwrap().len(),
        1,
        "cancel does not undo admitted IO"
    );
    assert!(f.home.list_secret_consumers()?.is_empty());
    native
        .clone()
        .begin(
            &f.requester,
            "{\"operation\":\"list\",\"offset\":0}",
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    Ok(())
}

#[test]
fn metadata_contention_does_not_consume_prompt_as_stale() -> anyhow::Result<()> {
    use std::sync::mpsc;
    let f = fixture()?;
    let p = prompt(&f);
    let authority = access(&f)
        .retained_prompt(&p.to_string())
        .unwrap()
        .authority
        .clone();
    let (entered, arrived) = mpsc::channel();
    let (resume, blocked) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        authority.bind(SecretPath::parse("managed.test").unwrap(), || {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
    });
    arrived.recv_timeout(Duration::from_secs(5))?;
    assert_eq!(
        access(&f).status(&f.requester, p),
        Err(HostAccessError::Unavailable)
    );
    resume.send(())?;
    thread.join().unwrap().unwrap();
    let status: Value = serde_json::from_str(&access(&f).status(&f.requester, p).unwrap())?;
    assert_eq!(status["status"], "pending");
    Ok(())
}
#[test]
fn exact_disconnect_cannot_revoke_replacement_and_can_revoke_disabled_world_consumer()
-> anyhow::Result<()> {
    let mut f = fixture()?;
    let one = f
        .home
        .create_shared_secret("one", &SecretValue::new("fake one"))?;
    let two = f
        .home
        .create_shared_secret("two", &SecretValue::new("fake two"))?;
    let target = access(&f)
        .policy_control()
        .unwrap()
        .snapshot("host", &f.requester.instance_id, "backend")
        .unwrap()
        .target;
    f.runtime
        .bind_shared_secret(&target, SecretPath::parse("mail.password")?, one.id)
        .unwrap();
    let old = f.home.list_secret_consumers()?[0].clone();
    f.runtime
        .bind_shared_secret(&target, SecretPath::parse("mail.password")?, two.id)
        .unwrap();
    let h = begin(&f, json!({"operation":"disconnect","grant":old}));
    assert_eq!(wait(&f, h)["code"], "stale");
    let new = f.home.list_secret_consumers()?[0].clone();
    let h = begin(&f, json!({"operation":"disconnect","grant":new}));
    assert_eq!(wait(&f, h)["status"], "completed");
    let world = f.home.create_world()?;
    let artifact = f.home.load_profile()?.activations[0].artifact.clone();
    let activation = f
        .home
        .select_world_stored_rtw(world.id, &artifact, Some(false))?;
    f.home.grant_secret_read(
        crate::world_runtime_scope_id(world.id),
        ComponentRef::new(activation.instance_id, "backend"),
        SecretPath::parse("mail.password")?,
    )?;
    f.runtime.activate_world(&f.home, world.id)?;
    let grant = f.home.list_secret_consumers()?[0].clone();
    let h = begin(&f, json!({"operation":"disconnect","grant":grant}));
    assert_eq!(wait(&f, h)["status"], "completed");
    assert!(f.home.list_secret_consumers()?.is_empty());
    assert_eq!(f.home.list_shared_secrets()?.len(), 2);
    Ok(())
}
#[test]
fn expired_prompt_and_changed_persisted_artifact_cannot_bind() -> anyhow::Result<()> {
    let f = fixture()?;
    let entry = f
        .home
        .create_shared_secret("entry", &SecretValue::new("fake"))?;
    let p = prompt(&f);
    {
        let a = access(&f);
        let mut book = a.secret_operations.0.lock().unwrap();
        Arc::get_mut(book.entries.get_mut(&p).unwrap().prompt.as_mut().unwrap())
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
    }
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":entry.id}),
    );
    assert_eq!(wait(&f, h)["status"], "failed");
    access(&f).close(&f.requester, p, true).unwrap();
    let p = prompt(&f);
    let mut profile = f.home.load_profile()?;
    profile.activations[0].artifact =
        rintawa_artifacts::ArtifactDigest::sha256(b"changed selection");
    profile.save(&f.home.profile_path)?;
    let h = begin(
        &f,
        json!({"operation":"bind-prompt","prompt":p.to_string(),"id":entry.id}),
    );
    assert_eq!(wait(&f, h)["code"], "stale");
    assert!(f.home.list_secret_consumers()?.is_empty());
    Ok(())
}
