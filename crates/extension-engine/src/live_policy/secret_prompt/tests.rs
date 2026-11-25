//! Prompt admission and commit fence regressions without GUI or real user vaults.

use super::*;
use crate::{
    execution_context::{ExecutionContexts, VerifiedComponent},
    live_policy::{LivePolicyChange, LivePolicyDelta},
    runtime_permissions::RuntimePermissionManager,
    secrets::{InMemorySecretVault, SecretManager},
};
use rintawa_sdk::{manifest::ComponentPermissions, secrets::SecretPathPattern};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> (LivePolicyControl, LivePolicyTarget) {
    let control = LivePolicyControl::new(
        ExecutionContexts::default(),
        RuntimePermissionManager::default(),
        SecretManager::with_vault(Arc::new(InMemorySecretVault::default())),
    );
    for (component, permissions) in [
        (
            "settings",
            ComponentPermissions {
                runtime: vec![RuntimePermission::SecretPrompt],
                ..Default::default()
            },
        ),
        (
            "backend",
            ComponentPermissions {
                secret_read: vec![SecretPathPattern::parse("mail.*").unwrap()],
                ..Default::default()
            },
        ),
    ] {
        control
            .contexts
            .begin_verified(
                ComponentRef::new("instance", component),
                "host".into(),
                Some(VerifiedComponent {
                    digest: rintawa_artifacts::ArtifactDigest::sha256(b"same artifact"),
                    published_key: None,
                    permissions,
                }),
            )
            .unwrap();
    }
    control.contexts.activate_instance(&"instance".into());
    let requester = control
        .snapshot("host", "instance", "settings")
        .unwrap()
        .target;
    (control, requester)
}

fn grant(control: &LivePolicyControl, requester: &LivePolicyTarget, allow: bool) {
    let delta = if allow {
        LivePolicyDelta {
            grant_runtime: vec![RuntimePermission::SecretPrompt],
            ..Default::default()
        }
    } else {
        LivePolicyDelta {
            revoke_runtime: vec![RuntimePermission::SecretPrompt],
            ..Default::default()
        }
    };
    control
        .apply(
            &LivePolicyChange {
                target: requester.clone(),
                delta,
            },
            || Ok(()),
        )
        .unwrap();
}

#[test]
fn prompt_requires_manifest_grant_and_exact_verified_sibling_without_expanding_authority() {
    let (control, requester) = fixture();
    let slot = SecretPath::parse("mail.password").unwrap();
    assert!(
        control
            .admit_secret_prompt(&requester, &"backend".into(), slot.clone())
            .is_err()
    );
    grant(&control, &requester, true);
    assert!(
        control
            .admit_secret_prompt(&requester, &"foreign-component".into(), slot.clone())
            .is_err()
    );
    assert!(
        control
            .admit_secret_prompt(
                &requester,
                &"backend".into(),
                SecretPath::parse("other.slot").unwrap()
            )
            .is_err()
    );
    for field in 0..3 {
        let mut wrong = requester.clone();
        match field {
            0 => wrong.scope_id = "other-world".into(),
            1 => wrong.activation = "0".into(),
            _ => wrong.artifact = "other-artifact".into(),
        }
        assert!(
            control
                .admit_secret_prompt(&wrong, &"backend".into(), slot.clone())
                .is_err()
        );
    }
    let authority = control
        .admit_secret_prompt(&requester, &"backend".into(), slot.clone())
        .unwrap();
    let owner = ComponentRef::new("instance", "backend");
    assert_eq!(authority.target().component_id, "backend");
    assert_eq!(authority.slot(), &slot);
    assert!(
        !control
            .secrets
            .has_read_grant(&owner, &SecretPathPattern::Exact(slot.clone()))
    );
    authority
        .bind(SecretPath::parse("managed.entry_first").unwrap(), || Ok(()))
        .unwrap();
    assert!(
        control
            .secrets
            .has_read_grant(&owner, &SecretPathPattern::Exact(slot))
    );
}

#[test]
fn revoke_regrant_and_activation_replacement_never_revive_an_old_prompt() {
    let (control, requester) = fixture();
    grant(&control, &requester, true);
    let authority = control
        .admit_secret_prompt(
            &requester,
            &"backend".into(),
            SecretPath::parse("mail.password").unwrap(),
        )
        .unwrap();
    grant(&control, &requester, false);
    grant(&control, &requester, true);
    assert_eq!(authority.validate(), Err(HostAccessError::Rejected));
    assert!(
        authority
            .bind(
                SecretPath::parse("managed.entry_first").unwrap(),
                || panic!("revoked prompt must not persist")
            )
            .is_err()
    );
    let fresh = control
        .admit_secret_prompt(
            &requester,
            &"backend".into(),
            SecretPath::parse("mail.password").unwrap(),
        )
        .unwrap();
    control
        .contexts
        .revoke_instance_if(&"instance".into(), None)
        .dispose();
    assert!(fresh.validate().is_err());
    assert!(
        fresh
            .bind(
                SecretPath::parse("managed.entry_first").unwrap(),
                || panic!("stopped prompt must not persist")
            )
            .is_err()
    );
}

#[test]
fn prompt_permission_revoke_is_ordered_after_an_admitted_commit_without_engine_reentry() {
    let (control, requester) = fixture();
    grant(&control, &requester, true);
    let authority = control
        .admit_secret_prompt(
            &requester,
            &"backend".into(),
            SecretPath::parse("mail.password").unwrap(),
        )
        .unwrap();
    let (entered, wait) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        authority.bind(SecretPath::parse("managed.entry_first").unwrap(), || {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(())
        })
    });
    wait.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        control.apply(
            &LivePolicyChange {
                target: requester.clone(),
                delta: LivePolicyDelta {
                    revoke_runtime: vec![RuntimePermission::SecretPrompt],
                    ..Default::default()
                }
            },
            || panic!("busy fence must not persist")
        ),
        Err(HostAccessError::Unavailable)
    );
    release.send(()).unwrap();
    worker.join().unwrap().unwrap();
    grant(&control, &requester, false);
    assert!(control.secrets.has_read_grant(
        &ComponentRef::new("instance", "backend"),
        &SecretPathPattern::Exact(SecretPath::parse("mail.password").unwrap())
    ));
}
