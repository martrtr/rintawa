//! Live consent, revocation and exact activation regression tests.
use super::*;
use crate::{
    execution_context::VerifiedComponent,
    secrets::{InMemorySecretVault, SecretVault},
};
use rintawa_sdk::{
    manifest::ComponentPermissions,
    secrets::{SecretAccessError, SecretValue},
    types::RuntimeScopeId,
};
use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};
fn bind(c: &ExecutionContexts, id: &str, bytes: &[u8]) -> LivePolicyTarget {
    let owner = ComponentRef::new(id, "backend");
    c.begin_verified(
        owner.clone(),
        RuntimeScopeId::new(format!("world:{id}")),
        Some(VerifiedComponent {
            digest: rintawa_artifacts::ArtifactDigest::sha256(bytes),
            published_key: None,
            permissions: ComponentPermissions {
                runtime: vec![
                    RuntimePermission::HttpRequest,
                    RuntimePermission::PublishedPreferenceRead,
                    RuntimePermission::BackgroundTask,
                    RuntimePermission::LoopbackListen,
                    RuntimePermission::LoopbackConnect,
                ],
                secret_read: vec![SecretPathPattern::parse("ai.*").unwrap()],
            },
        }),
    )
    .unwrap();
    c.activate_instance(&owner.instance_id);
    let (ctx, v) = c.policy_identity(&owner).unwrap();
    LivePolicyTarget {
        scope_id: ctx.scope.to_string(),
        instance_id: id.into(),
        component_id: "backend".into(),
        artifact: v.digest.to_string(),
        activation: ctx.principal.activation.to_string(),
    }
}
fn control() -> LivePolicyControl {
    LivePolicyControl::new(
        ExecutionContexts::default(),
        RuntimePermissionManager::default(),
        SecretManager::with_vault(Arc::new(InMemorySecretVault::default())),
    )
}
fn change(t: &LivePolicyTarget, delta: LivePolicyDelta) -> LivePolicyChange {
    LivePolicyChange {
        target: t.clone(),
        delta,
    }
}
#[test]
fn live_grant_is_effective_without_replacing_activation_and_denies_stale_identity() {
    let p = control();
    let t = bind(&p.contexts, "a", b"v1");
    let b = bind(&p.contexts, "b", b"v1");
    let d = LivePolicyDelta {
        grant_runtime: vec![
            RuntimePermission::HttpRequest,
            RuntimePermission::PublishedPreferenceRead,
        ],
        allow_http_origins: vec!["http://127.0.0.1:11434".into()],
        allow_secret_read: vec![SecretPath::parse("ai.one").unwrap()],
        ..Default::default()
    };
    let saved = AtomicBool::new(false);
    p.apply(&change(&t, d.clone()), || {
        saved.store(true, Ordering::Release);
        Ok(())
    })
    .unwrap();
    assert!(saved.load(Ordering::Acquire));
    let a = p
        .snapshot(&t.scope_id, &t.instance_id, &t.component_id)
        .unwrap();
    assert_eq!(a.target, t);
    assert_eq!(a.granted.len(), 2);
    assert_eq!(a.http_origins.len(), 1);
    assert_eq!(a.secret_read, vec!["ai.one"]);
    assert!(
        p.snapshot(&b.scope_id, &b.instance_id, &b.component_id)
            .unwrap()
            .granted
            .is_empty()
    );
    for (index, mut wrong) in [t.clone(), t.clone(), t.clone()].into_iter().enumerate() {
        match index {
            0 => wrong.activation = "0".into(),
            1 => wrong.artifact = rintawa_artifacts::ArtifactDigest::sha256(b"other").to_string(),
            _ => wrong.scope_id = b.scope_id.clone(),
        }
        assert!(
            p.apply(&change(&wrong, d.clone()), || panic!(
                "stale target persisted"
            ))
            .is_err()
        );
    }
    p.contexts.revoke_instance_if(&"a".into(), None).dispose();
    bind(&p.contexts, "a", b"v2");
    assert!(
        p.apply(&change(&t, d), || panic!("replacement persisted"))
            .is_err()
    );
}
#[test]
fn invalid_delta_and_failed_save_change_neither_policy_nor_cancellable_work() {
    let p = control();
    let t = bind(&p.contexts, "a", b"v1");
    for d in [
        LivePolicyDelta {
            grant_runtime: vec![RuntimePermission::RuntimePolicyWrite],
            ..Default::default()
        },
        LivePolicyDelta {
            allow_http_origins: vec!["http://127.0.0.1:11434".into()],
            ..Default::default()
        },
        LivePolicyDelta {
            allow_secret_read: vec![SecretPath::parse("other.key").unwrap()],
            ..Default::default()
        },
        LivePolicyDelta {
            grant_runtime: vec![RuntimePermission::HttpRequest],
            revoke_runtime: vec![RuntimePermission::HttpRequest],
            ..Default::default()
        },
    ] {
        assert!(
            p.apply(&change(&t, d), || panic!("invalid delta persisted"))
                .is_err()
        );
    }
    assert!(
        p.apply(
            &change(
                &t,
                LivePolicyDelta {
                    grant_runtime: vec![RuntimePermission::HttpRequest],
                    ..Default::default()
                }
            ),
            || Err(HostAccessError::Unavailable)
        )
        .is_err()
    );
    assert!(
        p.snapshot(&t.scope_id, &t.instance_id, &t.component_id)
            .unwrap()
            .granted
            .is_empty()
    );
}
#[test]
fn origin_revoke_cancels_without_poll_regrant_cannot_revive_and_other_world_origin_survive() {
    let p = control();
    let a = bind(&p.contexts, "a", b"v1");
    let b = bind(&p.contexts, "b", b"v1");
    for t in [&a, &b] {
        p.apply(
            &change(
                t,
                LivePolicyDelta {
                    grant_runtime: vec![RuntimePermission::HttpRequest],
                    allow_http_origins: vec![
                        "http://127.0.0.1:11434".into(),
                        "http://127.0.0.1:11435".into(),
                    ],
                    ..Default::default()
                },
            ),
            || Ok(()),
        )
        .unwrap();
    }
    let start = |t: &LivePolicyTarget, port| {
        p.runtime
            .register_http(
                &ComponentRef::new(t.instance_id.as_str(), "backend"),
                t.activation.parse().unwrap(),
                &url::Url::parse(&format!("http://127.0.0.1:{port}/v1")).unwrap(),
                Arc::new(AtomicBool::new(false)),
                None,
            )
            .unwrap()
    };
    let old = start(&a, 11434);
    let other = start(&a, 11435);
    let other_world = start(&b, 11434);
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                deny_http_origins: vec!["http://127.0.0.1:11434".into()],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    assert!(old.cancelled.load(Ordering::Acquire));
    assert!(!other.cancelled.load(Ordering::Acquire));
    assert!(!other_world.cancelled.load(Ordering::Acquire));
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                allow_http_origins: vec!["http://127.0.0.1:11434".into()],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    assert!(old.revoked.load(Ordering::Acquire));
    let new = start(&a, 11434);
    assert!(!new.revoked.load(Ordering::Acquire));
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                revoke_runtime: vec![RuntimePermission::HttpRequest],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    assert!(new.cancelled.load(Ordering::Acquire));
    assert!(other.cancelled.load(Ordering::Acquire));
    assert!(!other_world.cancelled.load(Ordering::Acquire));
}
struct HeldVault {
    entered: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl SecretVault for HeldVault {
    fn store(&self, _: &SecretPath, _: &SecretValue) -> Result<(), SecretAccessError> {
        Ok(())
    }
    fn delete(&self, _: &SecretPath) -> Result<bool, SecretAccessError> {
        Ok(false)
    }
    fn load(&self, _: &SecretPath) -> Result<Option<SecretValue>, SecretAccessError> {
        self.entered.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        Ok(Some(SecretValue::new("synthetic-local-test")))
    }
}
#[test]
fn secret_revoke_during_vault_read_blocks_disclosure_even_after_regrant() {
    let (entered, wait) = mpsc::sync_channel(1);
    let (resume, gate) = mpsc::sync_channel(1);
    let p = LivePolicyControl::new(
        ExecutionContexts::default(),
        RuntimePermissionManager::default(),
        SecretManager::with_vault(Arc::new(HeldVault {
            entered,
            resume: Mutex::new(gate),
        })),
    );
    let t = bind(&p.contexts, "a", b"v1");
    let path = SecretPath::parse("ai.one").unwrap();
    let allow = change(
        &t,
        LivePolicyDelta {
            allow_secret_read: vec![path.clone()],
            ..Default::default()
        },
    );
    p.apply(&allow, || Ok(())).unwrap();
    let secrets = p.secrets.clone();
    let key = path.clone();
    let reader = std::thread::spawn(move || {
        secrets.read_for_component(&ComponentRef::new("a", "backend"), &key)
    });
    wait.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    p.apply(
        &change(
            &t,
            LivePolicyDelta {
                deny_secret_read: vec![path],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    p.apply(&allow, || Ok(())).unwrap();
    resume.send(()).unwrap();
    assert!(matches!(
        reader.join().unwrap(),
        Err(SecretAccessError::AccessDenied)
    ));
}
#[test]
fn lifecycle_cannot_replace_target_during_durable_publication() {
    let p = control();
    let t = bind(&p.contexts, "a", b"v1");
    let contexts = p.contexts.clone();
    let (entered, wait) = mpsc::sync_channel(1);
    let (resume, gate) = mpsc::sync_channel(1);
    let update = p.clone();
    let decision = change(
        &t,
        LivePolicyDelta {
            grant_runtime: vec![RuntimePermission::HttpRequest],
            ..Default::default()
        },
    );
    let writer = std::thread::spawn(move || {
        update.apply(&decision, || {
            entered.send(()).unwrap();
            gate.recv().unwrap();
            Ok(())
        })
    });
    wait.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let (replaced, observe) = mpsc::sync_channel(1);
    let replacement = std::thread::spawn(move || {
        contexts.revoke_instance_if(&"a".into(), None).dispose();
        let next = bind(&contexts, "a", b"v2");
        replaced.send(next).unwrap();
    });
    assert!(
        observe
            .recv_timeout(std::time::Duration::from_millis(40))
            .is_err()
    );
    resume.send(()).unwrap();
    writer.join().unwrap().unwrap();
    let next = observe
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    replacement.join().unwrap();
    assert_ne!(next.activation, t.activation);
}

#[test]
fn revoking_a_does_not_invalidate_b_read_already_waiting_in_vault() {
    let (entered, wait) = mpsc::sync_channel(1);
    let (resume, gate) = mpsc::sync_channel(1);
    let p = LivePolicyControl::new(
        ExecutionContexts::default(),
        RuntimePermissionManager::default(),
        SecretManager::with_vault(Arc::new(HeldVault {
            entered,
            resume: Mutex::new(gate),
        })),
    );
    let a = bind(&p.contexts, "a", b"v1");
    let b = bind(&p.contexts, "b", b"v1");
    let path = SecretPath::parse("ai.shared").unwrap();
    for t in [&a, &b] {
        p.apply(
            &change(
                t,
                LivePolicyDelta {
                    allow_secret_read: vec![path.clone()],
                    ..Default::default()
                },
            ),
            || Ok(()),
        )
        .unwrap();
    }
    let secrets = p.secrets.clone();
    let read_path = path.clone();
    let reader = std::thread::spawn(move || {
        secrets.read_for_component(&ComponentRef::new("b", "backend"), &read_path)
    });
    wait.recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                deny_secret_read: vec![path],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    resume.send(()).unwrap();
    assert_eq!(
        reader.join().unwrap().unwrap().expose_secret(),
        "synthetic-local-test"
    );
}

#[test]
fn runtime_revoke_cancels_only_affected_capability_for_exact_owner() {
    let p = control();
    let a = bind(&p.contexts, "a", b"v1");
    let b = bind(&p.contexts, "b", b"v1");
    for t in [&a, &b] {
        p.apply(
            &change(
                t,
                LivePolicyDelta {
                    grant_runtime: vec![
                        RuntimePermission::BackgroundTask,
                        RuntimePermission::LoopbackListen,
                    ],
                    ..Default::default()
                },
            ),
            || Ok(()),
        )
        .unwrap();
    }
    let token = |t: &LivePolicyTarget, permission| {
        p.runtime
            .register_resource(
                &ComponentRef::new(t.instance_id.as_str(), "backend"),
                t.activation.parse().unwrap(),
                permission,
                None,
            )
            .unwrap()
    };
    let task = token(&a, RuntimePermission::BackgroundTask);
    let socket = token(&a, RuntimePermission::LoopbackListen);
    let other = token(&b, RuntimePermission::BackgroundTask);
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                revoke_runtime: vec![RuntimePermission::BackgroundTask],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    assert!(task.revoked.load(Ordering::Acquire));
    assert!(!socket.revoked.load(Ordering::Acquire));
    assert!(!other.revoked.load(Ordering::Acquire));
    p.apply(
        &change(
            &a,
            LivePolicyDelta {
                grant_runtime: vec![RuntimePermission::BackgroundTask],
                ..Default::default()
            },
        ),
        || Ok(()),
    )
    .unwrap();
    assert!(task.revoked.load(Ordering::Acquire));
}
