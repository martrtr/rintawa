use std::fs;

use rintawa_artifacts::{ImportDisposition, RtwLimits, pack_directory};
use rintawa_extension_engine::UnresolvedContractReason;
use rintawa_host::{HOST_SCOPE, HostError, HostHome, HostRuntime};
use rintawa_sdk::{
    contracts::{ComponentRef, host_shell_contract_key},
    types::RuntimeScopeId,
};

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
fn test_should_read_legacy_profile_schema_one_without_preferences() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home_path = root.path().join("home");
    let profile_dir = home_path.join("profiles");
    fs::create_dir_all(&profile_dir)?;
    fs::write(
        profile_dir.join(rintawa_host::BASELINE_PROFILE_FILE),
        "schema = 1\n",
    )?;

    let home = HostHome::open(&home_path)?;
    let profile = home.load_profile()?;
    assert_eq!(profile.schema, rintawa_host::PROFILE_SCHEMA);
    assert!(profile.activations.is_empty());
    assert!(profile.preferred_providers.is_empty());
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
