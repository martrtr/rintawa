//! Unit tests for WASM host state, sandbox boundaries, and delegated execution.

use super::*;
use crate::secrets::InMemorySecretVault;
use rintawa_sdk::{
    api::{LogLevel, LoggerApi},
    secrets::{SecretPath, SecretPathPattern, SecretValue},
    types::ExtensionId,
};
use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
};
use url::Url;

fn test_instance_id() -> ExtensionInstanceId {
    ExtensionInstanceId::new("test-instance")
}

fn test_scope_id() -> RuntimeScopeId {
    RuntimeScopeId::new("test")
}

fn begin_test_registration(state: &mut WasmHostState, extension_id: ExtensionId) {
    state
        .begin_registration(extension_id, test_instance_id(), test_scope_id())
        .unwrap();
}

#[test]
fn test_should_reject_non_public_http_fetch_destinations() {
    for url in [
        "http://example.com/index.json",
        "https://127.0.0.1/index.json",
        "https://10.0.0.1/index.json",
        "https://169.254.169.254/latest/meta-data",
        "https://198.18.0.58/index.json",
        "https://[::1]/index.json",
        "https://[fc00::1]/index.json",
        "https://[fe80::1]/index.json",
        "https://[fec0::1]/index.json",
    ] {
        let parsed = Url::parse(url).unwrap();
        assert!(matches!(
            build_bounded_https_client(&parsed, Duration::from_millis(50)),
            Err(HttpFetchError::InvalidUrl | HttpFetchError::ForbiddenDestination)
        ));
    }
}

#[test]
fn test_should_allow_benchmarking_fake_ip_only_for_domain_tunnel_resolution() {
    let synthetic = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 58));
    let tunnel_source = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1));
    let ordinary_source = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));

    assert!(!is_allowed_public_ip(synthetic));
    assert!(is_allowed_domain_destination_with_source(
        synthetic,
        Some(tunnel_source)
    ));
    assert!(!is_allowed_domain_destination_with_source(
        synthetic,
        Some(ordinary_source)
    ));
    assert!(!is_allowed_domain_destination_with_source(
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        Some(tunnel_source)
    ));
}

#[test]
fn test_should_classify_public_and_special_ip_ranges() {
    assert!(is_allowed_public_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    assert!(is_allowed_public_ip(IpAddr::V6(
        "2606:4700:4700::1111".parse().unwrap()
    )));

    for address in [
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
        IpAddr::V6("2001:db8::1".parse().unwrap()),
    ] {
        assert!(!is_allowed_public_ip(address));
    }
}

struct TestLogger;

impl LoggerApi for TestLogger {
    fn log(&self, _level: LogLevel, _message: &str) {}
}

struct TestRuntimeContext {
    extension_id: ExtensionId,
    instance_id: ExtensionInstanceId,
    scope_id: RuntimeScopeId,
    component_id: ComponentId,
    logger: TestLogger,
    effects: HashMap<RuntimeEffectId, RuntimeEffect>,
    next_effect_sequence: u64,
    registration_attempts: u64,
    failing_registration_attempts: HashSet<u64>,
}

impl TestRuntimeContext {
    fn new() -> Self {
        Self {
            extension_id: ExtensionId::new("rintawa.chat"),
            instance_id: test_instance_id(),
            scope_id: test_scope_id(),
            component_id: ComponentId::new("chat-runtime"),
            logger: TestLogger,
            effects: HashMap::new(),
            next_effect_sequence: 0,
            registration_attempts: 0,
            failing_registration_attempts: HashSet::new(),
        }
    }

    fn fail_registration_on(&mut self, attempt: u64) {
        self.failing_registration_attempts.insert(attempt);
    }
}

impl ComponentContext for TestRuntimeContext {
    fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    fn extension_instance_id(&self) -> &ExtensionInstanceId {
        &self.instance_id
    }

    fn runtime_scope_id(&self) -> &RuntimeScopeId {
        &self.scope_id
    }

    fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    fn logger(&self) -> &dyn LoggerApi {
        &self.logger
    }

    fn register_runtime_effect(
        &mut self,
        effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        self.registration_attempts += 1;
        if self
            .failing_registration_attempts
            .contains(&self.registration_attempts)
        {
            return Err(ExtensionError::Message(String::from(
                "simulated effect registration failure",
            )));
        }
        let effect_id = RuntimeEffectId::new(format!("effect-{}", self.next_effect_sequence));
        self.next_effect_sequence += 1;
        self.effects.insert(effect_id.clone(), effect);
        Ok(effect_id)
    }

    fn revoke_runtime_effect(&mut self, effect_id: &RuntimeEffectId) -> ExtensionResult<()> {
        self.effects.remove(effect_id);
        Ok(())
    }

    fn revoke_all_runtime_effects(&mut self) -> ExtensionResult<()> {
        self.effects.clear();
        Ok(())
    }
}

#[test]
fn test_should_abort_failed_guest_execution_and_revoke_effects() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    let mut context = TestRuntimeContext::new();
    let effect_id = RuntimeEffectId::new("effect-active");
    let effect = RuntimeEffect::event_subscription("dialogue.message");
    context.effects.insert(effect_id.clone(), effect.clone());
    state.effect_handles.insert(
        String::from("guest-handle"),
        ActiveWasmRuntimeEffect {
            owner: rintawa_sdk::contracts::ComponentRef::new(
                test_instance_id(),
                ComponentId::new("chat-runtime"),
            ),
            effect_id,
            effect,
        },
    );
    state.begin_guest_execution();

    let error = state.abort_guest_execution(
        &mut context,
        ExtensionError::Message(String::from("simulated callback failure")),
    );

    assert_eq!(error.to_string(), "simulated callback failure");
    assert!(context.effects.is_empty());
    assert!(state.effect_handles.is_empty());
    assert!(!state.runtime_effects_active);
    assert!(!state.secret_access_active);
    assert!(!state.service_access_active);
    assert!(!state.ui_access_active);
}

#[test]
fn test_should_enforce_wasm_ui_message_limit_for_registration_and_string_ids() {
    let budget = WasmExecutionBudget {
        max_host_message_bytes: 4,
        ..WasmExecutionBudget::default()
    };
    let secrets = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
    let mut state = WasmHostState::with_host_services_and_budget(
        ComponentId::new("runtime"),
        WasmHostServices::standalone(secrets),
        false,
        &budget,
    );
    begin_test_registration(&mut state, ExtensionId::new("example.extension"));

    assert!(matches!(
        PortableUiHost::register_surface(
            &mut state,
            String::from("12345"),
            WitPlacementHint::Primary,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
        ),
        Err(PortableUiError::MessageTooLarge)
    ));

    state.finish_registration().unwrap();
    state.begin_guest_execution();
    assert!(matches!(
        PortableUiHost::unmount_surface(&mut state, String::from("12345")),
        Err(PortableUiError::MessageTooLarge)
    ));
}

#[test]
fn test_should_count_ui_registration_structure_overhead() {
    let budget = WasmExecutionBudget {
        max_host_message_bytes: 64,
        ..WasmExecutionBudget::default()
    };
    let secrets = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
    let mut state = WasmHostState::with_host_services_and_budget(
        ComponentId::new("runtime"),
        WasmHostServices::standalone(secrets),
        false,
        &budget,
    );
    begin_test_registration(&mut state, ExtensionId::new("example.extension"));

    assert!(matches!(
        PortableUiHost::register_surface(
            &mut state,
            String::from("x"),
            WitPlacementHint::Primary,
            None,
            None,
            None,
            Vec::new(),
            vec![String::new(); 32],
        ),
        Err(PortableUiError::MessageTooLarge)
    ));
}

#[test]
fn test_should_report_event_publication_as_unavailable() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));

    assert!(matches!(
        HostOperations::publish_event(
            &mut state,
            String::from("dialogue.message"),
            b"payload".to_vec(),
        ),
        Err(PublishError::Unavailable)
    ));
}

#[test]
fn test_should_commit_wasm_capability_contributions_only_after_registration_finishes() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));

    RegistrationHost::register_capability(
        &mut state,
        String::from("rintawa.ai"),
        String::from("{}"),
    )
    .unwrap();

    let registrations = state.finish_registration().unwrap();

    assert_eq!(registrations.contributions.len(), 1);
    assert_eq!(registrations.contributions[0].id.as_str(), "rintawa.ai");
    assert_eq!(
        registrations.contributions[0].kind,
        ContributionKind::capability()
    );
}

#[test]
fn test_should_stage_wasm_contract_registrations() {
    let mut state = WasmHostState::new(ComponentId::new("runtime"));
    begin_test_registration(&mut state, ExtensionId::new("example.extension"));

    RegistrationHost::define_contract(
        &mut state,
        String::from("example.service"),
        1,
        WitResolutionPolicy::Single,
        WitContractProtocol::Service,
    )
    .unwrap();
    RegistrationHost::provide_contract(
        &mut state,
        String::from("example.service"),
        1,
        vec![String::from("ai.api_keys.*")],
    )
    .unwrap();
    RegistrationHost::consume_contract(
        &mut state,
        String::from("example.dependency"),
        1,
        true,
        Vec::new(),
    )
    .unwrap();

    let registrations = state.finish_registration().unwrap();
    assert_eq!(registrations.definitions.len(), 1);
    assert_eq!(registrations.providers.len(), 1);
    assert_eq!(registrations.consumers.len(), 1);
    assert_eq!(
        registrations.definitions[0].contract.to_string(),
        "example.service@1"
    );
    assert_eq!(
        registrations.definitions[0].resolution,
        ContractResolutionPolicy::Single
    );
    assert_eq!(registrations.providers[0].required_grants.len(), 1);
    assert!(registrations.consumers[0].required);
}

#[test]
fn test_should_stage_and_apply_wasm_portable_ui_operations() {
    let mut state = WasmHostState::new(ComponentId::new("runtime"));
    let extension_id = ExtensionId::new("example.extension");
    begin_test_registration(&mut state, extension_id.clone());

    PortableUiHost::register_surface(
        &mut state,
        String::from("example.main"),
        WitPlacementHint::Primary,
        None,
        None,
        None,
        Vec::new(),
        vec![String::from(rintawa_sdk::ui::UI_CAPABILITY_TEXT)],
    )
    .unwrap();
    let registrations = state.finish_registration().unwrap();
    assert_eq!(registrations.ui_surfaces.len(), 1);
    assert_eq!(registrations.ui_surfaces[0].id.as_str(), "example.main");

    let owner =
        rintawa_sdk::contracts::ComponentRef::new(test_instance_id(), ComponentId::new("runtime"));
    state
        .ui
        .register_instance(
            test_instance_id(),
            test_scope_id(),
            vec![rintawa_ui_runtime::OwnedUiSurfaceContribution {
                owner: owner.clone(),
                contribution: registrations.ui_surfaces[0].clone(),
            }],
            Vec::new(),
        )
        .unwrap();

    let snapshot = UiSurfaceSnapshot {
        surface_id: UiSurfaceId::new("example.main"),
        revision: 1,
        root: rintawa_sdk::ui::UiNodeId::new("root"),
        nodes: vec![rintawa_sdk::ui::UiNode::new(
            "root",
            rintawa_sdk::ui::UiNodeKind::Text(rintawa_sdk::ui::UiTextNode {
                text: String::from("hello"),
            }),
        )],
    };
    state.begin_guest_execution();
    PortableUiHost::mount_surface(&mut state, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    state.finish_service_execution();
    assert_eq!(state.ui.presentation_surfaces().len(), 0);

    state
        .ui
        .set_instance_active(&test_instance_id(), true)
        .unwrap();
    assert_eq!(state.ui.presentation_surfaces().len(), 1);

    let patch = UiPatchBatch {
        surface_id: UiSurfaceId::new("example.main"),
        base_revision: 1,
        next_revision: 2,
        patches: vec![rintawa_sdk::ui::UiPatch::UpsertNode {
            node: rintawa_sdk::ui::UiNode::new(
                "root",
                rintawa_sdk::ui::UiNodeKind::Text(rintawa_sdk::ui::UiTextNode {
                    text: String::from("updated"),
                }),
            ),
        }],
    };
    state.begin_guest_execution();
    PortableUiHost::patch_surface(&mut state, serde_json::to_vec(&patch).unwrap()).unwrap();
    PortableUiHost::unmount_surface(&mut state, String::from("example.main")).unwrap();
    state.finish_service_execution();
    assert!(state.ui.presentation_surfaces().is_empty());
}

#[test]
fn test_should_reject_wasm_service_calls_outside_execution_scope() {
    let mut state = WasmHostState::new(ComponentId::new("runtime"));
    begin_test_registration(&mut state, ExtensionId::new("example.extension"));
    state.finish_registration().unwrap();

    assert!(matches!(
        ServicesHost::call(
            &mut state,
            String::from("example.service"),
            1,
            b"payload".to_vec(),
        ),
        Err(ServiceTransportError::Unavailable)
    ));
}

#[test]
fn test_should_install_and_revoke_wasm_runtime_effects_with_handles() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    let _ = state
        .finish_registration()
        .expect("test registration should finish");
    let mut context = TestRuntimeContext::new();

    state.begin_guest_execution();
    let handle =
        RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message")).unwrap();
    state.finish_guest_execution(&mut context).unwrap();
    assert_eq!(context.effects.len(), 1);

    state.begin_guest_execution();
    RuntimeEffectsHost::unsubscribe_event(&mut state, handle).unwrap();
    state.finish_guest_execution(&mut context).unwrap();
    assert!(context.effects.is_empty());
}

#[test]
fn test_should_reject_wasm_effect_or_registration_outside_its_lifecycle_scope() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));

    assert!(matches!(
        RuntimeEffectsHost::subscribe_event(&mut state, String::from("dialogue.message")),
        Err(RuntimeEffectError::RuntimeNotActive)
    ));
    assert!(matches!(
        RegistrationHost::register_capability(
            &mut state,
            String::from("rintawa.ai"),
            String::from("{}"),
        ),
        Err(RegistrationError::RegistrationNotActive)
    ));
}

#[test]
fn test_should_read_only_host_granted_wasm_secret_during_and_after_execution() {
    let manager = SecretManager::with_vault(Arc::new(InMemorySecretVault::default()));
    let extension_id = ExtensionId::new("official_ai");
    let component_id = ComponentId::new("provider");
    let allowed_path = SecretPath::parse("ai.api_keys.openai").unwrap();

    manager
        .store(&allowed_path, &SecretValue::new("test-key"))
        .unwrap();
    manager
        .grant_read(
            rintawa_sdk::contracts::ComponentRef::new(test_instance_id(), component_id.clone()),
            SecretPathPattern::parse("ai.api_keys.*").unwrap(),
        )
        .unwrap();

    let mut state = WasmHostState::with_secret_manager(component_id, manager);
    begin_test_registration(&mut state, extension_id);
    state.finish_registration().unwrap();

    assert!(matches!(
        SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
        Err(SecretError::AccessNotActive)
    ));

    state.begin_guest_execution();
    assert!(matches!(
        SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
        Ok(value) if value == "test-key"
    ));
    assert!(matches!(
        SecretsHost::read(&mut state, String::from("ai.api_keys_backup.openai")),
        Err(SecretError::AccessDenied)
    ));

    let mut context = TestRuntimeContext::new();
    state.finish_guest_execution(&mut context).unwrap();
    assert!(matches!(
        SecretsHost::read(&mut state, String::from("ai.api_keys.openai")),
        Err(SecretError::AccessNotActive)
    ));
}

#[test]
fn test_should_roll_back_effects_when_a_callback_batch_fails() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    let _ = state
        .finish_registration()
        .expect("test registration should finish");
    let mut context = TestRuntimeContext::new();
    context.fail_registration_on(2);

    state.begin_guest_execution();
    state
        .subscribe_event(String::from("dialogue.first"))
        .unwrap();
    state
        .subscribe_event(String::from("dialogue.second"))
        .unwrap();

    assert!(state.finish_guest_execution(&mut context).is_err());
    assert!(context.effects.is_empty());
}

#[test]
fn test_should_report_a_failed_rollback_without_retaining_a_stale_handle() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    let _ = state
        .finish_registration()
        .expect("test registration should finish");
    let mut context = TestRuntimeContext::new();

    state.begin_guest_execution();
    let active_handle = state
        .subscribe_event(String::from("dialogue.active"))
        .unwrap();
    state.finish_guest_execution(&mut context).unwrap();

    context.fail_registration_on(2);
    context.fail_registration_on(3);
    state.begin_guest_execution();
    state.unsubscribe_event(active_handle.clone()).unwrap();
    state.subscribe_event(String::from("dialogue.new")).unwrap();

    assert!(matches!(
        state.finish_guest_execution(&mut context),
        Err(ExtensionError::RuntimeEffectRollbackFailed { .. })
    ));
    assert!(context.effects.is_empty());

    state.begin_guest_execution();
    assert!(matches!(
        state.unsubscribe_event(active_handle),
        Err(RuntimeEffectError::UnknownEffect)
    ));
}

#[test]
fn test_should_reject_unversioned_execution_target_from_guest_start() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    let _ = state
        .finish_registration()
        .expect("test registration should finish");

    state.begin_start_execution();
    assert!(matches!(
        ExecutionTargetsHost::register_target(&mut state, String::from("runtime")),
        Err(TargetRegistrationError::InvalidTarget)
    ));
    assert!(state.pending_execution_targets.is_empty());
}

#[test]
fn test_should_discard_pending_execution_targets_when_start_scope_aborts() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    let _ = state
        .finish_registration()
        .expect("test registration should finish");

    state.begin_start_execution();
    assert!(
        ExecutionTargetsHost::register_target(&mut state, String::from("example.runtime@1"))
            .is_ok()
    );
    assert_eq!(
        state.pending_execution_targets,
        vec![String::from("example.runtime@1")]
    );

    state.discard_guest_execution();
    assert!(state.pending_execution_targets.is_empty());
}

#[test]
fn test_should_require_explicit_runtime_policy_permissions() {
    let mut state = WasmHostState::new(ComponentId::new("runtime"));
    begin_test_registration(&mut state, ExtensionId::new("admin.ui"));
    state.finish_registration().unwrap();
    state.begin_guest_execution();

    assert!(matches!(
        RuntimePolicyHost::list_components(&mut state),
        Err(RuntimePolicyError::PermissionDenied)
    ));
    assert!(matches!(
        RuntimePolicyHost::inspect_artifact(&mut state, String::from("sha256:deadbeef")),
        Err(RuntimePolicyError::PermissionDenied)
    ));
    assert!(matches!(
        RuntimePolicyHost::grant(
            &mut state,
            String::from("host"),
            String::from("target"),
            String::from("runtime"),
            String::from("background-task"),
        ),
        Err(RuntimePolicyError::PermissionDenied)
    ));

    let owner = state.registered_owner().unwrap();
    state
        .runtime_permissions
        .grant(owner.clone(), RuntimePermission::RuntimePolicyRead)
        .unwrap();
    assert!(matches!(
        RuntimePolicyHost::list_components(&mut state),
        Err(RuntimePolicyError::Unavailable)
    ));
    assert!(matches!(
        RuntimePolicyHost::inspect_artifact(&mut state, String::from("sha256:deadbeef")),
        Err(RuntimePolicyError::Unavailable)
    ));

    state
        .runtime_permissions
        .grant(owner, RuntimePermission::RuntimePolicyWrite)
        .unwrap();
    assert!(matches!(
        RuntimePolicyHost::grant(
            &mut state,
            String::from("host"),
            String::from("target"),
            String::from("runtime"),
            String::from("background-task"),
        ),
        Err(RuntimePolicyError::Unavailable)
    ));
    assert!(matches!(
        RuntimePolicyHost::revoke(
            &mut state,
            String::from("host"),
            String::from("target"),
            String::from("runtime"),
            String::from("background-task"),
        ),
        Err(RuntimePolicyError::Unavailable)
    ));
}

#[test]
fn test_should_require_explicit_runtime_permission_for_background_tasks() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    state.finish_registration().unwrap();
    state.task_handler_available = true;
    state.begin_guest_execution();

    assert!(matches!(
        RuntimeTasksHost::spawn_periodic(&mut state, 10),
        Err(RuntimeTaskError::PermissionDenied)
    ));

    let owner = state.registered_owner().unwrap();
    state
        .runtime_permissions
        .grant(owner, RuntimePermission::BackgroundTask)
        .unwrap();
    let handle = RuntimeTasksHost::spawn_periodic(&mut state, 10).unwrap();
    assert!(state.active_tasks.contains_key(&handle));
    RuntimeTasksHost::cancel(&mut state, handle).unwrap();
    assert!(state.active_tasks.is_empty());
}

#[test]
fn test_should_bound_background_task_count_and_interval() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("rintawa.chat"));
    state.finish_registration().unwrap();
    state.task_handler_available = true;
    state.max_background_tasks = 1;
    state.min_background_task_interval_ms = 25;
    let owner = state.registered_owner().unwrap();
    state
        .runtime_permissions
        .grant(owner, RuntimePermission::BackgroundTask)
        .unwrap();
    state.begin_guest_execution();

    assert!(matches!(
        RuntimeTasksHost::spawn_periodic(&mut state, 10),
        Err(RuntimeTaskError::InvalidInterval)
    ));
    let _ = RuntimeTasksHost::spawn_periodic(&mut state, 25).unwrap();
    assert!(matches!(
        RuntimeTasksHost::spawn_periodic(&mut state, 25),
        Err(RuntimeTaskError::LimitExceeded)
    ));
}

#[test]
fn test_should_require_delegated_principal_own_runtime_permissions() {
    let mut state = WasmHostState::new(ComponentId::new("provider-runtime"));
    begin_test_registration(&mut state, ExtensionId::new("runtime.provider"));
    state.finish_registration().unwrap();
    state.task_handler_available = true;
    let root_owner = state.registered_owner().unwrap();
    state
        .runtime_permissions
        .grant(root_owner.clone(), RuntimePermission::BackgroundTask)
        .unwrap();
    state
        .runtime_permissions
        .grant(root_owner, RuntimePermission::LoopbackListen)
        .unwrap();

    let dependent_owner =
        rintawa_sdk::contracts::ComponentRef::new("dependent-instance", "hosted-component");
    state.begin_delegated_guest_execution(dependent_owner.clone());
    assert!(matches!(
        RuntimeTasksHost::spawn_periodic(&mut state, 10),
        Err(RuntimeTaskError::PermissionDenied)
    ));
    assert!(matches!(
        NetworkHost::listen_loopback(&mut state, 0),
        Err(NetworkError::PermissionDenied)
    ));

    state
        .runtime_permissions
        .grant(dependent_owner.clone(), RuntimePermission::BackgroundTask)
        .unwrap();
    state
        .runtime_permissions
        .grant(dependent_owner.clone(), RuntimePermission::LoopbackListen)
        .unwrap();
    let task = RuntimeTasksHost::spawn_periodic(&mut state, 10).unwrap();
    let listener = NetworkHost::listen_loopback(&mut state, 0).unwrap();
    assert_eq!(
        state.active_tasks.get(&task).map(|active| &active.owner),
        Some(&dependent_owner)
    );
    assert_eq!(
        state
            .network_handles
            .get(&listener.handle)
            .map(|resource| &resource.owner),
        Some(&dependent_owner)
    );
}

#[test]
fn test_should_exchange_bounded_bytes_through_owner_scoped_loopback_handles() {
    let mut state = WasmHostState::new(ComponentId::new("runtime"));
    begin_test_registration(&mut state, ExtensionId::new("runtime.provider"));
    state.finish_registration().unwrap();
    let owner = state.registered_owner().unwrap();
    state
        .runtime_permissions
        .grant(owner.clone(), RuntimePermission::LoopbackListen)
        .unwrap();
    state
        .runtime_permissions
        .grant(owner.clone(), RuntimePermission::LoopbackConnect)
        .unwrap();
    state.begin_guest_execution();

    let listener = NetworkHost::listen_loopback(&mut state, 0).unwrap();
    assert_ne!(listener.port, 0);
    let client = NetworkHost::connect_loopback(&mut state, listener.port).unwrap();
    let server = (0..50)
        .find_map(|_| match NetworkHost::accept(&mut state, listener.handle) {
            Ok(Some(handle)) => Some(handle),
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(1));
                None
            }
            Err(error) => panic!("loopback accept failed: {error:?}"),
        })
        .expect("loopback connection should become acceptable");

    let empty_read = NetworkHost::read(&mut state, server, 0)
        .expect("zero-length read should validate the handle without consuming the stream");
    assert!(empty_read.data.is_empty());
    assert!(!empty_read.eof);

    assert!(matches!(
        NetworkHost::write(&mut state, client, b"ping".to_vec()),
        Ok(4)
    ));
    let received = (0..50)
        .find_map(|_| match NetworkHost::read(&mut state, server, 16) {
            Ok(result) if !result.data.is_empty() => Some(result),
            Ok(_) | Err(NetworkError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(1));
                None
            }
            Err(error) => panic!("loopback read failed: {error:?}"),
        })
        .expect("loopback payload should become readable");
    assert_eq!(received.data, b"ping");
    assert!(!received.eof);

    state.max_network_io_bytes = 3;
    assert!(matches!(
        NetworkHost::write(&mut state, client, b"four".to_vec()),
        Err(NetworkError::MessageTooLarge)
    ));
    state.revoke_runtime_resources_for_owner(&owner);
    assert!(state.network_handles.is_empty());
}

#[test]
fn test_should_reject_target_publication_without_provider_exports() {
    let runtime_engine = WasmRuntimeEngine::new().expect("test WASM runtime should initialize");
    let component = runtime_engine
        .load_component_from_bytes(
            ComponentId::new("ordinary-component"),
            include_str!("../../../tests/fixtures/stateful_component.wat").as_bytes(),
        )
        .expect("ordinary test component should compile");
    let mut runtime = component
        .runtime()
        .expect("test runtime lock should be available");
    let instance = component
        .ensure_instance(&mut runtime)
        .expect("ordinary test component should instantiate");

    let error = WasmTargetProviderEndpoint::ensure_target_provider(instance)
        .expect_err("ordinary component must not satisfy target-provider exports");
    assert!(
        error
            .to_string()
            .contains("without target-provider exports")
    );
}

#[test]
fn test_should_fail_fast_when_target_provider_runtime_is_reentered() {
    let runtime = Arc::new(Mutex::new(WasmSharedRuntime {
        instance: None,
        failed_lifecycle_callback: None,
    }));
    let endpoint = WasmTargetProviderEndpoint {
        runtime: runtime.clone(),
        budget: WasmExecutionBudget::default(),
    };
    let guard = runtime
        .lock()
        .expect("test runtime lock should be available");

    let error = match endpoint.runtime_for_callback() {
        Ok(_) => {
            panic!("reentrant target-provider callback must not block or acquire the lock")
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("runtime is busy"));
    drop(guard);
}

#[test]
fn test_should_drop_root_wasm_instance_after_poisoned_stop_cleanup() {
    let runtime_engine = WasmRuntimeEngine::new().expect("test WASM runtime should initialize");
    let mut component = runtime_engine
        .load_component_from_bytes(
            ComponentId::new("ordinary-component"),
            include_str!("../../../tests/fixtures/stateful_component.wat").as_bytes(),
        )
        .expect("ordinary test component should compile");
    {
        let mut runtime = component
            .runtime()
            .expect("test runtime lock should be available");
        component
            .ensure_instance(&mut runtime)
            .expect("ordinary test component should instantiate");
    }

    let shared = component.runtime.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _runtime = shared
            .lock()
            .expect("test runtime lock should start healthy");
        panic!("poison test WASM runtime lock");
    }));

    let mut context = TestRuntimeContext::new();
    let error = component
        .stop(&mut context)
        .expect_err("poisoned stop must remain caller-visible");
    assert!(error.to_string().contains("lock was poisoned"));

    let runtime = match shared.lock() {
        Ok(runtime) => runtime,
        Err(poisoned) => poisoned.into_inner(),
    };
    assert!(runtime.instance.is_none());
    assert_eq!(runtime.failed_lifecycle_callback, Some("stop"));
}

#[test]
fn test_should_revoke_delegated_resources_when_provider_lock_is_poisoned() {
    let runtime_engine = WasmRuntimeEngine::new().expect("test WASM runtime should initialize");
    let component = runtime_engine
        .load_component_from_bytes(
            ComponentId::new("ordinary-component"),
            include_str!("../../../tests/fixtures/stateful_component.wat").as_bytes(),
        )
        .expect("ordinary test component should compile");
    let owner = rintawa_sdk::contracts::ComponentRef::new(
        test_instance_id(),
        ComponentId::new("chat-runtime"),
    );
    {
        let mut runtime = component
            .runtime()
            .expect("test runtime lock should be available");
        let instance = component
            .ensure_instance(&mut runtime)
            .expect("ordinary test component should instantiate");
        instance.store.data_mut().active_tasks.insert(
            7,
            ActiveWasmTask {
                owner: owner.clone(),
                interval: Duration::from_millis(10),
                next_due: Instant::now(),
            },
        );
    }

    let shared = component.runtime.clone();
    let endpoint = WasmTargetProviderEndpoint {
        runtime: shared.clone(),
        budget: WasmExecutionBudget::default(),
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _runtime = shared
            .lock()
            .expect("test runtime lock should start healthy");
        panic!("poison test target-provider runtime lock");
    }));

    let mut context = TestRuntimeContext::new();
    let error = endpoint
        .stop_component(42, &mut context)
        .expect_err("poisoned target stop must remain caller-visible");
    assert!(error.to_string().contains("lock was poisoned"));

    let runtime = match shared.lock() {
        Ok(runtime) => runtime,
        Err(poisoned) => poisoned.into_inner(),
    };
    let instance = runtime
        .instance
        .as_ref()
        .expect("provider store should remain available for host cleanup inspection");
    assert!(
        instance
            .store
            .data()
            .active_tasks
            .values()
            .all(|task| task.owner != owner)
    );
}

#[test]
fn test_should_preserve_other_owner_effect_handles_after_delegated_abort() {
    let mut state = WasmHostState::new(ComponentId::new("chat-runtime"));
    let owner = rintawa_sdk::contracts::ComponentRef::new(
        test_instance_id(),
        ComponentId::new("chat-runtime"),
    );
    let other_owner = rintawa_sdk::contracts::ComponentRef::new(
        test_instance_id(),
        ComponentId::new("other-runtime"),
    );
    state.effect_handles.insert(
        String::from("own"),
        ActiveWasmRuntimeEffect {
            owner: owner.clone(),
            effect_id: RuntimeEffectId::new("own-effect"),
            effect: RuntimeEffect::event_subscription("own.topic"),
        },
    );
    state.effect_handles.insert(
        String::from("other"),
        ActiveWasmRuntimeEffect {
            owner: other_owner,
            effect_id: RuntimeEffectId::new("other-effect"),
            effect: RuntimeEffect::event_subscription("other.topic"),
        },
    );
    let mut context = TestRuntimeContext::new();
    state.begin_delegated_guest_execution(owner);

    let error = state.abort_guest_execution(
        &mut context,
        ExtensionError::Message(String::from("simulated delegated failure")),
    );

    assert_eq!(error.to_string(), "simulated delegated failure");
    assert!(!state.effect_handles.contains_key("own"));
    assert!(state.effect_handles.contains_key("other"));
}
