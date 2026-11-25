//! Concurrent profile updates through real independent HostHome processes.

use rintawa_host::{CompositionProfile, HOST_SCOPE, HostHome, RuntimePermissionGrant};
use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, types::RuntimeScopeId,
};

#[test]
fn test_should_preserve_concurrent_profile_updates_across_processes() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let home = HostHome::open(directory.path())?;
    let scope = RuntimeScopeId::new(HOST_SCOPE);
    let owner = ComponentRef::new("revoked-owner", "runtime");
    let mut initial = CompositionProfile::default();
    initial
        .runtime_permissions
        .push(RuntimePermissionGrant::new(
            scope.clone(),
            owner.clone(),
            RuntimePermission::HttpRequest,
        ));
    let profile = directory.path().join("profiles/baseline.toml");
    std::fs::write(&profile, toml::to_string(&initial)?)?;
    let mut children = Vec::new();
    for index in 0..8 {
        children.push(
            std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", "test_profile_process_probe", "--nocapture"])
                .env("RINTAWA_PROFILE_PROBE_HOME", directory.path())
                .env("RINTAWA_PROFILE_PROBE_INDEX", index.to_string())
                .stdout(std::process::Stdio::null())
                .spawn()?,
        );
    }
    home.revoke_runtime_permission(&scope, &owner, RuntimePermission::HttpRequest)?;
    for mut child in children {
        assert!(child.wait()?.success());
    }
    for index in 0..8 {
        let owner = ComponentRef::new(format!("writer-{index}"), "runtime");
        for key in 0..12 {
            assert_eq!(
                home.get_preference(
                    &RuntimeScopeId::new(HOST_SCOPE),
                    &owner,
                    &format!("key-{key}")
                )?,
                Some(format!("value-{key}"))
            );
        }
    }
    assert!(home.load_profile()?.runtime_permissions.is_empty());
    // Reads are side-effect free; failed validation must leave the old file intact.
    let before = std::fs::read(&profile)?;
    assert!(
        home.set_preference(
            RuntimeScopeId::new(HOST_SCOPE),
            ComponentRef::new("writer-0", "runtime"),
            String::new(),
            String::from("invalid")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(profile)?, before);
    Ok(())
}

#[test]
fn test_profile_process_probe() -> anyhow::Result<()> {
    let Some(root) = std::env::var_os("RINTAWA_PROFILE_PROBE_HOME") else {
        return Ok(());
    };
    let index = std::env::var("RINTAWA_PROFILE_PROBE_INDEX")?;
    let home = HostHome::open(root)?;
    let owner = ComponentRef::new(format!("writer-{index}"), "runtime");
    for key in 0..12 {
        home.set_preference(
            RuntimeScopeId::new(HOST_SCOPE),
            owner.clone(),
            format!("key-{key}"),
            format!("value-{key}"),
        )?;
    }
    Ok(())
}
