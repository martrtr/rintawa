//! Admission and retirement tests; file/broker semantics are tested in Host.
use super::*;
use crate::runtime_permissions::RuntimePermissionManager;
use rintawa_sdk::runtime_permissions::RuntimePermission;
use std::{sync::mpsc, thread, time::Duration};
type Gate = (mpsc::SyncSender<()>, mpsc::Receiver<()>);
#[derive(Default)]
struct Resources {
    entries: Mutex<HashMap<String, ComponentPrincipal>>,
    taking: Mutex<Option<Gate>>,
    cleaning: Mutex<Option<Gate>>,
    probe: Mutex<Option<(ExecutionContexts, ExecutionContext)>>,
}
impl Resources {
    fn wait(gate: &Mutex<Option<Gate>>) {
        if let Some((entered, resume)) = gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
}
impl UserResourceAccess for Resources {
    fn import_resource(
        &self,
        _: &[u8],
        _: &str,
        _: Option<&str>,
    ) -> HostAccessResult<UserResourceRef> {
        Err(HostAccessError::Unavailable)
    }
    fn read_resource(
        &self,
        _: &UserResourceRef,
        _: usize,
        _: &ComponentPrincipal,
    ) -> HostAccessResult<Vec<u8>> {
        Err(HostAccessError::Unavailable)
    }
    fn release_resource(
        &self,
        _: &UserResourceRef,
        _: &ComponentPrincipal,
    ) -> HostAccessResult<()> {
        Err(HostAccessError::Unavailable)
    }
    fn take_resource(
        &self,
        reference: &UserResourceRef,
        principal: &ComponentPrincipal,
        id: &str,
    ) -> HostAccessResult<UserResourceRef> {
        Self::wait(&self.taking);
        self.entries
            .lock()
            .unwrap()
            .insert(id.to_string(), principal.clone());
        Ok(UserResourceRef {
            id: id.into(),
            ..reference.clone()
        })
    }
    fn release_owned_resources(&self, principal: &ComponentPrincipal) {
        if let Some((contexts, context)) = self.probe.lock().unwrap().as_ref() {
            assert!(
                !matches!(
                    contexts.try_validate(context),
                    Err(TransportError::Unavailable)
                ),
                "cleanup must run after View unlock"
            );
        }
        Self::wait(&self.cleaning);
        self.entries
            .lock()
            .unwrap()
            .retain(|_, owner| owner != principal);
    }
}
fn bind(contexts: &ExecutionContexts, id: &str) -> ExecutionContext {
    let owner = ComponentRef::new(id, "backend");
    contexts
        .begin(owner.clone(), RuntimeScopeId::new(format!("world/{id}")))
        .unwrap();
    contexts.activate_instance(&owner.instance_id);
    contexts.context(&owner, false).unwrap()
}
fn take(
    contexts: &ExecutionContexts,
    context: &ExecutionContext,
    resources: &Arc<Resources>,
    id: &str,
) {
    contexts
        .take_resource(
            context,
            resources.clone(),
            &UserResourceRef {
                id: format!("upload-{id}"),
                size: 1,
                media_type: "application/octet-stream".into(),
                name: None,
            },
            id,
        )
        .unwrap()
        .unwrap();
}
fn gate() -> (Gate, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
    let (signal, entered) = mpsc::sync_channel(1);
    let (resume, wait) = mpsc::sync_channel(1);
    ((signal, wait), entered, resume)
}
#[test]
fn retirement_sweeps_exact_activation_and_its_dependents_after_unlock() {
    let contexts = ExecutionContexts::default();
    let resources = Arc::new(Resources::default());
    let provider = bind(&contexts, "provider");
    let a = bind(&contexts, "a");
    let b = bind(&contexts, "b");
    let other = bind(&contexts, "other");
    contexts
        .bind_provider(&a, provider.principal.component())
        .unwrap();
    contexts
        .bind_provider(&b, provider.principal.component())
        .unwrap();
    for (id, context) in [("root", &provider), ("a", &a), ("b", &b), ("other", &other)] {
        take(&contexts, context, &resources, id);
    }
    *resources.probe.lock().unwrap() = Some((contexts.clone(), a.clone()));
    let retired = contexts.revoke_instance_if(&a.principal.instance_id, None);
    assert_eq!(retired.owners, vec![a.principal.component()]);
    retired.dispose();
    assert_eq!(resources.entries.lock().unwrap().len(), 3);
    assert!(resources.entries.lock().unwrap().contains_key("b"));
    let retired = contexts.revoke_instance_if(&provider.principal.instance_id, None);
    assert_eq!(retired.owners.len(), 2);
    retired.dispose();
    assert_eq!(
        resources
            .entries
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["other"]
    );
    contexts
        .revoke_instance_if(&other.principal.instance_id, None)
        .dispose();
}
#[test]
fn replacing_binding_sweeps_old_generation_and_stale_retirement_preserves_new() {
    let contexts = ExecutionContexts::default();
    let resources = Arc::new(Resources::default());
    let old = bind(&contexts, "same");
    take(&contexts, &old, &resources, "old");
    let fresh = bind(&contexts, "same");
    assert_ne!(old.principal.activation, fresh.principal.activation);
    assert!(resources.entries.lock().unwrap().is_empty());
    take(&contexts, &fresh, &resources, "fresh");
    let retired = contexts.revoke_instance_if(&old.principal.instance_id, Some(&old));
    assert!(retired.owners.is_empty());
    retired.dispose();
    assert!(resources.entries.lock().unwrap().contains_key("fresh"));
    assert!(matches!(
        contexts.take_resource(
            &old,
            resources.clone(),
            &UserResourceRef {
                id: "old-upload".into(),
                size: 1,
                media_type: "a/b".into(),
                name: None
            },
            "late"
        ),
        Err(TransportError::Inactive)
    ));
    contexts
        .revoke_instance_if(&fresh.principal.instance_id, Some(&fresh))
        .dispose();
    assert!(resources.entries.lock().unwrap().is_empty());
}
#[test]
fn racing_take_cannot_survive_effective_permission_revocation() {
    let contexts = ExecutionContexts::default();
    let resources = Arc::new(Resources::default());
    let context = bind(&contexts, "owner");
    let permissions = RuntimePermissionManager::default();
    permissions
        .grant(
            context.principal.component(),
            RuntimePermission::UserResourceRead,
        )
        .unwrap();
    let (blocked, entered, resume) = gate();
    *resources.taking.lock().unwrap() = Some(blocked);
    let taking = {
        let contexts = contexts.clone();
        let resources = resources.clone();
        let permissions = permissions.clone();
        let context = context.clone();
        thread::spawn(move || {
            permissions
                .while_granted(
                    &context.principal.component(),
                    RuntimePermission::UserResourceRead,
                    || take(&contexts, &context, &resources, "taken"),
                )
                .unwrap()
                .unwrap();
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done, retired) = mpsc::sync_channel(1);
    let revoking = {
        let contexts = contexts.clone();
        let permissions = permissions.clone();
        let owner = context.principal.component();
        thread::spawn(move || {
            permissions.revoke_component(&owner, &contexts);
            done.send(()).unwrap();
        })
    };
    assert!(matches!(
        retired.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    resume.send(()).unwrap();
    taking.join().unwrap();
    revoking.join().unwrap();
    retired.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(resources.entries.lock().unwrap().is_empty());
    assert_eq!(
        permissions.has_grant(
            &context.principal.component(),
            RuntimePermission::UserResourceRead
        ),
        Ok(false)
    );
}
#[test]
fn same_activation_regrant_waits_until_previous_cleanup_finishes() {
    let contexts = ExecutionContexts::default();
    let resources = Arc::new(Resources::default());
    let context = bind(&contexts, "owner");
    let permissions = RuntimePermissionManager::default();
    permissions
        .grant(
            context.principal.component(),
            RuntimePermission::UserResourceRead,
        )
        .unwrap();
    permissions
        .while_granted(
            &context.principal.component(),
            RuntimePermission::UserResourceRead,
            || take(&contexts, &context, &resources, "old"),
        )
        .unwrap()
        .unwrap();
    let (blocked, entered, resume) = gate();
    *resources.cleaning.lock().unwrap() = Some(blocked);
    *resources.probe.lock().unwrap() = Some((contexts.clone(), context.clone()));
    let revoking = {
        let contexts = contexts.clone();
        let permissions = permissions.clone();
        let owner = context.principal.component();
        thread::spawn(move || permissions.revoke_component(&owner, &contexts))
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done, granted) = mpsc::sync_channel(1);
    let granting = {
        let contexts = contexts.clone();
        let resources = resources.clone();
        let permissions = permissions.clone();
        let context = context.clone();
        thread::spawn(move || {
            permissions
                .grant(
                    context.principal.component(),
                    RuntimePermission::UserResourceRead,
                )
                .unwrap();
            permissions
                .while_granted(
                    &context.principal.component(),
                    RuntimePermission::UserResourceRead,
                    || take(&contexts, &context, &resources, "new"),
                )
                .unwrap()
                .unwrap();
            done.send(()).unwrap();
        })
    };
    assert!(matches!(
        granted.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    resume.send(()).unwrap();
    revoking.join().unwrap();
    granting.join().unwrap();
    granted.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        resources
            .entries
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["new"]
    );
    contexts
        .revoke_instance_if(&context.principal.instance_id, None)
        .dispose();
}
