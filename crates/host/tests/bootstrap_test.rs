use std::{fs, time::Duration};

use rintawa_artifacts::{ImportDisposition, RtwLimits, pack_directory};
use rintawa_extension_engine::UnresolvedContractReason;
use rintawa_host::{HOST_SCOPE, HostError, HostHome, HostRuntime};
use rintawa_sdk::{
    contracts::{ComponentRef, host_shell_contract_key, ui_layer_contract_key},
    runtime_permissions::RuntimePermission,
    types::RuntimeScopeId,
};

const TEST_TARGET_PROVIDER_COMPONENT: &[u8] =
    include_bytes!("../../extension-engine/tests/fixtures/target-provider/component.wasm");
const TEST_TASK_COMPONENT: &[u8] =
    include_bytes!("../../extension-engine/tests/fixtures/task-runtime/component.wasm");

fn build_target_provider(root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let source = root.join("target-provider-source");
    fs::create_dir_all(&source)?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        source.join("manifest.toml"),
        r#"id = "bootstrap.target-provider"
name = "Target Provider"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "provider.wasm"
"#,
    )?;
    fs::write(source.join("provider.wasm"), TEST_TARGET_PROVIDER_COMPONENT)?;
    let artifact = root.join("target-provider.rtw");
    pack_directory(&source, &artifact, RtwLimits::default())?;
    Ok(artifact)
}

fn build_task_runtime(
    root: &std::path::Path,
    requests_background_task: bool,
) -> anyhow::Result<std::path::PathBuf> {
    let suffix = if requests_background_task {
        "requested"
    } else {
        "unrequested"
    };
    let source = root.join(format!("task-runtime-source-{suffix}"));
    fs::create_dir_all(&source)?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    let permission = if requests_background_task {
        "\n[components.permissions]\nruntime = [\"background-task\"]\n"
    } else {
        ""
    };
    fs::write(
        source.join("manifest.toml"),
        format!(
            r#"id = "bootstrap.task-runtime"
name = "Task Runtime"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "task.wasm"
{permission}"#,
        ),
    )?;
    fs::write(source.join("task.wasm"), TEST_TASK_COMPONENT)?;
    let artifact = root.join(format!("task-runtime-{suffix}.rtw"));
    pack_directory(&source, &artifact, RtwLimits::default())?;
    Ok(artifact)
}

fn build_target_dependent(root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let source = root.join("target-dependent-source");
    fs::create_dir_all(&source)?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        source.join("manifest.toml"),
        r#"id = "bootstrap.target-dependent"
name = "Target Dependent"
version = "0.1.0"
sdk = "^0.0"

[[components]]
id = "hosted"
kind = "runtime"
target = "test.wasm-target@1"
entry = "payload.bin"
"#,
    )?;
    fs::write(source.join("payload.bin"), b"hosted payload")?;
    let artifact = root.join("target-dependent.rtw");
    pack_directory(&source, &artifact, RtwLimits::default())?;
    Ok(artifact)
}

fn build_extension(
    root: &std::path::Path,
    version: &str,
    name: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let source = root.join(format!("source-{version}"));
    fs::create_dir_all(&source)?;
    fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        source.join("manifest.toml"),
        format!(
            "id = \"example.bootstrap\"\nname = \"{name}\"\nversion = \"{version}\"\nsdk = \"^0.0\"\n"
        ),
    )?;
    let artifact = root.join(format!("example-{version}.rtw"));
    pack_directory(&source, &artifact, RtwLimits::default())?;
    Ok(artifact)
}

#[test]
fn local_install_enable_disable_and_restart_are_persistent() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let home_path = root.path().join("home");

    let home = HostHome::open(&home_path)?;
    let installed = home.install_local_rtw(&artifact, Some(false))?;
    assert_eq!(installed.disposition, ImportDisposition::Imported);
    assert!(!installed.activation.enabled);

    let reopened = HostHome::open(&home_path)?;
    let listed = reopened.list_activations()?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].subject, "example.bootstrap");
    assert!(!listed[0].enabled);

    reopened.set_enabled("example.bootstrap", true)?;
    let restarted = HostHome::open(&home_path)?;
    assert!(restarted.list_activations()?[0].enabled);
    let runtime = HostRuntime::start(&restarted)?;
    assert_eq!(runtime.host_shell_provider(), None);
    assert_eq!(runtime.ui_layer_provider(), None);
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn manual_update_repoints_activation_and_preserves_enabled_state() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let first = build_extension(root.path(), "0.0.1", "First")?;
    let second = build_extension(root.path(), "0.0.2", "Second")?;
    let home = HostHome::open(root.path().join("home"))?;

    let first_install = home.install_local_rtw(&first, Some(false))?;
    let second_install = home.install_local_rtw(&second, None)?;
    assert_ne!(
        first_install.activation.digest,
        second_install.activation.digest
    );
    assert_eq!(second_install.activation.version.as_deref(), Some("0.0.2"));
    assert!(!second_install.activation.enabled);

    let listed = home.list_activations()?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].digest, second_install.activation.digest);
    assert_eq!(listed[0].version.as_deref(), Some("0.0.2"));
    Ok(())
}

#[test]
fn generic_import_does_not_activate_until_exact_digest_is_selected() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let bytes = fs::read(&artifact)?;
    let home = HostHome::open(root.path().join("home"))?;

    let imported = home.import_rtw_bytes(&bytes)?;
    assert!(home.list_activations()?.is_empty());

    let selected = home.select_stored_rtw(imported.digest(), Some(false))?;
    assert_eq!(selected.subject, "example.bootstrap");
    assert_eq!(selected.version.as_deref(), Some("0.0.1"));
    assert!(!selected.enabled);
    assert_eq!(home.list_activations()?.len(), 1);
    Ok(())
}

#[test]
fn removing_activation_cleans_profile_policy_but_keeps_cas_bytes() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_task_runtime(root.path(), true)?;
    let home = HostHome::open(root.path().join("home"))?;
    let installed = home.install_local_rtw(&artifact, None)?;
    let scope = RuntimeScopeId::new(HOST_SCOPE);
    let owner = ComponentRef::new("bootstrap.task-runtime", "runtime");
    home.grant_runtime_permission(scope, owner, RuntimePermission::BackgroundTask)?;

    home.remove_activation("bootstrap.task-runtime")?;
    let profile = home.load_profile()?;
    assert!(profile.activations.is_empty());
    assert!(profile.runtime_permissions.is_empty());
    assert!(
        home.artifact_store()
            .open_artifact(&installed.activation.digest)
            .is_ok()
    );
    Ok(())
}

#[test]
fn importing_identical_bytes_reuses_cas_object() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let home = HostHome::open(root.path().join("home"))?;

    assert_eq!(
        home.install_local_rtw(&artifact, None)?.disposition,
        ImportDisposition::Imported
    );
    assert_eq!(
        home.install_local_rtw(&artifact, None)?.disposition,
        ImportDisposition::AlreadyPresent
    );
    Ok(())
}

#[test]
fn test_should_read_legacy_profile_schemas_without_runtime_permissions() -> anyhow::Result<()> {
    for schema in [1, 2, 3] {
        let root = tempfile::tempdir()?;
        let home_path = root.path().join(format!("home-{schema}"));
        let profile_dir = home_path.join("profiles");
        fs::create_dir_all(&profile_dir)?;
        fs::write(
            profile_dir.join(rintawa_host::BASELINE_PROFILE_FILE),
            format!("schema = {schema}\n"),
        )?;

        let home = HostHome::open(&home_path)?;
        let profile = home.load_profile()?;
        assert_eq!(profile.schema, rintawa_host::PROFILE_SCHEMA);
        assert!(profile.activations.is_empty());
        assert!(profile.preferred_providers.is_empty());
        assert!(profile.runtime_permissions.is_empty());
    }
    Ok(())
}

#[test]
fn test_preferences_are_owner_scoped_persistent_and_bounded() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    let scope = RuntimeScopeId::new(HOST_SCOPE);
    let owner = ComponentRef::new("example.manager", "runtime");
    let other = ComponentRef::new("example.other", "runtime");

    assert_eq!(home.get_preference(&scope, &owner, "repositories")?, None);

    home.set_preference(
        scope.clone(),
        owner.clone(),
        "repositories".to_string(),
        "[\"https://example.invalid/index.json\"]".to_string(),
    )?;

    assert_eq!(
        home.get_preference(&scope, &owner, "repositories")?
            .as_deref(),
        Some("[\"https://example.invalid/index.json\"]")
    );
    assert_eq!(home.get_preference(&scope, &other, "repositories")?, None);

    let reopened = HostHome::open(&home_path)?;
    assert_eq!(
        reopened
            .get_preference(&scope, &owner, "repositories")?
            .as_deref(),
        Some("[\"https://example.invalid/index.json\"]")
    );

    assert!(matches!(
        reopened.set_preference(
            scope.clone(),
            owner.clone(),
            String::new(),
            "value".to_string(),
        ),
        Err(HostError::InvalidPreference(_))
    ));
    assert!(matches!(
        reopened.set_preference(
            scope.clone(),
            owner.clone(),
            "oversized".to_string(),
            "x".repeat(64 * 1024 + 1),
        ),
        Err(HostError::InvalidPreference(_))
    ));

    reopened.delete_preference(&scope, &owner, "repositories")?;
    assert_eq!(
        reopened.get_preference(&scope, &owner, "repositories")?,
        None
    );
    Ok(())
}

#[test]
fn test_should_persist_runtime_permission_and_apply_it_during_bootstrap() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_task_runtime(root.path(), true)?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    home.install_local_rtw(&artifact, None)?;

    let scope = RuntimeScopeId::new(HOST_SCOPE);
    let owner = ComponentRef::new("bootstrap.task-runtime", "runtime");
    home.grant_runtime_permission(
        scope.clone(),
        owner.clone(),
        RuntimePermission::BackgroundTask,
    )?;

    let profile = home.load_profile()?;
    assert_eq!(profile.schema, rintawa_host::PROFILE_SCHEMA);
    assert_eq!(profile.runtime_permissions.len(), 1);
    assert_eq!(profile.runtime_permissions[0].scope_id, scope);
    assert_eq!(profile.runtime_permissions[0].owner(), owner);
    assert_eq!(
        profile.runtime_permissions[0].permission,
        RuntimePermission::BackgroundTask
    );

    let profile_source = fs::read_to_string(
        home_path
            .join("profiles")
            .join(rintawa_host::BASELINE_PROFILE_FILE),
    )?;
    assert!(profile_source.contains("schema = 4"));
    assert!(profile_source.contains("[[runtime_permissions]]"));
    assert!(profile_source.contains("permission = \"background-task\""));

    let reopened = HostHome::open(&home_path)?;
    let mut runtime = HostRuntime::start(&reopened)?;
    let delay = runtime
        .poll_runtime()?
        .expect("persisted background-task grant should schedule a cooperative wake-up");
    std::thread::sleep(delay + Duration::from_millis(2));
    assert_eq!(runtime.poll_runtime()?, None);
    runtime.shutdown()?;

    reopened.revoke_runtime_permission(
        &RuntimeScopeId::new(HOST_SCOPE),
        &ComponentRef::new("bootstrap.task-runtime", "runtime"),
        RuntimePermission::BackgroundTask,
    )?;
    assert!(reopened.load_profile()?.runtime_permissions.is_empty());
    Ok(())
}

#[test]
fn test_should_reject_runtime_permission_not_requested_by_exact_artifact() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_task_runtime(root.path(), false)?;
    let home = HostHome::open(root.path().join("home"))?;
    home.install_local_rtw(&artifact, None)?;

    assert!(matches!(
        home.grant_runtime_permission(
            RuntimeScopeId::new(HOST_SCOPE),
            ComponentRef::new("bootstrap.task-runtime", "runtime"),
            RuntimePermission::BackgroundTask,
        ),
        Err(HostError::RuntimePermissionNotRequested { .. })
    ));
    Ok(())
}

#[test]
fn test_should_fail_closed_on_stale_persisted_runtime_permission() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_task_runtime(root.path(), true)?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    home.install_local_rtw(&artifact, None)?;

    let profile_path = home_path
        .join("profiles")
        .join(rintawa_host::BASELINE_PROFILE_FILE);
    let mut profile_source = fs::read_to_string(&profile_path)?;
    profile_source.push_str(
        r#"
[[runtime_permissions]]
scope-id = "host"
instance-id = "bootstrap.task-runtime"
component-id = "runtime"
permission = "loopback-listen"
"#,
    );
    fs::write(&profile_path, profile_source)?;

    let error = match HostRuntime::start(&home) {
        Ok(runtime) => {
            runtime.shutdown()?;
            anyhow::bail!("stale runtime permission must not expand manifest policy");
        }
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostError::Engine(rintawa_extension_engine::EngineError::RuntimePermissionNotRequested {
            permission,
            ..
        }) if permission == "loopback-listen"
    ));
    Ok(())
}

#[test]
fn test_should_reject_duplicate_runtime_permission_records() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let profile_dir = home_path.join("profiles");
    fs::create_dir_all(&profile_dir)?;
    fs::write(
        profile_dir.join(rintawa_host::BASELINE_PROFILE_FILE),
        r#"schema = 3

[[runtime_permissions]]
scope-id = "host"
instance-id = "example"
component-id = "runtime"
permission = "background-task"

[[runtime_permissions]]
scope-id = "host"
instance-id = "example"
component-id = "runtime"
permission = "background-task"
"#,
    )?;

    let home = HostHome::open(&home_path)?;
    assert!(matches!(
        home.load_profile(),
        Err(HostError::DuplicateRuntimePermissionGrant { .. })
    ));
    Ok(())
}

#[test]
fn test_should_persist_preferred_provider_in_flat_profile_schema() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let home_path = root.path().join("home");
    let home = HostHome::open(&home_path)?;
    home.install_local_rtw(&artifact, None)?;

    let scope = RuntimeScopeId::new(HOST_SCOPE);
    let contract = host_shell_contract_key();
    let provider = ComponentRef::new("example.bootstrap", "shell");
    home.set_preferred_provider(scope.clone(), contract.clone(), provider.clone())?;

    let reopened = HostHome::open(&home_path)?;
    let profile = reopened.load_profile()?;
    assert_eq!(profile.preferred_providers.len(), 1);
    assert_eq!(profile.preferred_providers[0].scope_id, scope);
    assert_eq!(profile.preferred_providers[0].contract(), contract);
    assert_eq!(profile.preferred_providers[0].provider(), provider);

    let profile_source = fs::read_to_string(
        home_path
            .join("profiles")
            .join(rintawa_host::BASELINE_PROFILE_FILE),
    )?;
    assert!(profile_source.contains("contract-id = \"rintawa.host.shell\""));
    assert!(profile_source.contains("contract-version = 1"));
    assert!(profile_source.contains("provider-instance-id = \"example.bootstrap\""));
    assert!(profile_source.contains("provider-component-id = \"shell\""));
    assert!(!profile_source.contains("[preferred_providers.provider]"));

    reopened
        .clear_preferred_provider(&RuntimeScopeId::new(HOST_SCOPE), &host_shell_contract_key())?;
    assert!(reopened.load_profile()?.preferred_providers.is_empty());
    Ok(())
}

#[test]
fn test_should_fail_explicit_unavailable_host_shell_without_fallback() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let home = HostHome::open(root.path().join("home"))?;
    home.install_local_rtw(&artifact, None)?;
    home.set_preferred_provider(
        RuntimeScopeId::new(HOST_SCOPE),
        host_shell_contract_key(),
        ComponentRef::new("example.bootstrap", "missing-shell"),
    )?;

    let error = match HostRuntime::start(&home) {
        Ok(runtime) => {
            runtime.shutdown()?;
            anyhow::bail!("explicit unavailable Host Shell selection should fail startup");
        }
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostError::ContractRoleUnavailable {
            reason: UnresolvedContractReason::PreferredProviderUnavailable,
            ..
        }
    ));
    Ok(())
}

#[test]
fn test_should_revisit_deferred_activation_after_runtime_provider_starts() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let dependent = build_target_dependent(root.path())?;
    let provider = build_target_provider(root.path())?;
    let home = HostHome::open(root.path().join("home"))?;

    home.install_local_rtw(&dependent, Some(true))?;
    home.install_local_rtw(&provider, Some(true))?;
    let profile = home.load_profile()?;
    assert_eq!(profile.activations[0].subject, "bootstrap.target-dependent");
    assert_eq!(profile.activations[1].subject, "bootstrap.target-provider");

    let runtime = HostRuntime::start(&home)?;
    runtime.shutdown()?;
    Ok(())
}

#[test]
fn test_should_report_fixed_point_when_required_execution_target_never_appears()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let dependent = build_target_dependent(root.path())?;
    let home = HostHome::open(root.path().join("home"))?;
    home.install_local_rtw(&dependent, Some(true))?;

    let error = match HostRuntime::start(&home) {
        Ok(runtime) => {
            runtime.shutdown()?;
            anyhow::bail!("bootstrap should not ignore a permanently deferred activation");
        }
        Err(error) => error,
    };
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("bootstrap.target-dependent"));
    assert!(diagnostic.contains("test.wasm-target@1"));
    assert!(matches!(
        error,
        HostError::BootstrapStalled(stall)
            if stall.blocked.is_empty()
                && stall.batch_error.is_none()
                && stall.deferred.len() == 1
                && stall.deferred[0].instance_id.as_str() == "bootstrap.target-dependent"
                && stall.deferred[0].missing_required_targets.len() == 1
                && stall.deferred[0].missing_required_targets[0].component_id.as_str() == "hosted"
                && stall.deferred[0].missing_required_targets[0].target.as_str() == "test.wasm-target@1"
    ));
    Ok(())
}

#[test]
fn test_should_fail_explicit_unavailable_ui_layer_without_fallback() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let artifact = build_extension(root.path(), "0.0.1", "Bootstrap")?;
    let home = HostHome::open(root.path().join("home"))?;
    home.install_local_rtw(&artifact, None)?;
    home.set_preferred_provider(
        RuntimeScopeId::new(HOST_SCOPE),
        ui_layer_contract_key(),
        ComponentRef::new("example.bootstrap", "missing-layer"),
    )?;

    let error = match HostRuntime::start(&home) {
        Ok(runtime) => {
            runtime.shutdown()?;
            anyhow::bail!("explicit unavailable UI Layer selection should fail startup");
        }
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostError::ContractRoleUnavailable {
            reason: UnresolvedContractReason::PreferredProviderUnavailable,
            ..
        }
    ));
    Ok(())
}
