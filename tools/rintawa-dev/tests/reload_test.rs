//! Integration tests for development watch and reload behavior.

use std::fs;

use rintawa_dev::{DevError, ReloadOutcome, ReloadingDevSession};
use rintawa_extension_engine::ExtensionState;
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

fn write_extension(path: &std::path::Path, version: &str, component: &str) -> std::io::Result<()> {
    fs::write(
        path.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        path.join("manifest.toml"),
        format!(
            "id = \"dev.reload\"\nname = \"Dev Reload\"\nversion = \"{version}\"\nsdk = \"^0.0\"\n{component}"
        ),
    )?;
    Ok(())
}

fn start(path: &std::path::Path) -> anyhow::Result<ReloadingDevSession> {
    Ok(ReloadingDevSession::start(
        path,
        ExtensionInstanceId::new("reload-instance"),
        RuntimeScopeId::new("reload-scope"),
    )?)
}

#[test]
fn test_reload_should_skip_byte_identical_snapshot() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_extension(temp.path(), "0.1.0", "")?;
    let mut runner = start(temp.path())?;

    assert_eq!(runner.reload()?, ReloadOutcome::Unchanged);
    assert_eq!(runner.state(), Some(ExtensionState::Active));
    runner.shutdown()?;
    Ok(())
}

#[test]
fn test_reload_should_replace_running_snapshot_after_source_change() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_extension(temp.path(), "0.1.0", "")?;
    let mut runner = start(temp.path())?;
    let previous = runner
        .session()
        .expect("initial session must exist")
        .digest()
        .clone();

    write_extension(temp.path(), "0.2.0", "")?;
    let outcome = runner.reload()?;
    let current = runner
        .session()
        .expect("reloaded session must exist")
        .digest()
        .clone();

    assert!(matches!(
        outcome,
        ReloadOutcome::Reloaded { previous: old, current: new }
            if old == previous && new == current
    ));
    assert_ne!(previous, current);
    assert_eq!(runner.state(), Some(ExtensionState::Active));
    runner.shutdown()?;
    Ok(())
}

#[test]
fn test_reload_should_keep_session_running_when_build_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_extension(temp.path(), "0.1.0", "")?;
    let mut runner = start(temp.path())?;
    let previous = runner
        .session()
        .expect("initial session must exist")
        .digest()
        .clone();
    fs::write(
        temp.path().join("rintawa-dev.toml"),
        "schema = 1\nbuild = [\"rintawa-command-that-does-not-exist\"]\n",
    )?;

    assert!(runner.reload().is_err());
    assert_eq!(
        runner
            .session()
            .expect("failed build must preserve session")
            .digest(),
        &previous
    );
    assert_eq!(runner.state(), Some(ExtensionState::Active));
    runner.shutdown()?;
    Ok(())
}

#[test]
fn test_reload_should_restore_previous_snapshot_when_new_runtime_fails() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_extension(temp.path(), "0.1.0", "")?;
    let mut runner = start(temp.path())?;
    let previous = runner
        .session()
        .expect("initial session must exist")
        .digest()
        .clone();

    write_extension(
        temp.path(),
        "0.2.0",
        "\n[[components]]\nid = \"native\"\nkind = \"runtime\"\ntarget = \"native\"\nrequired = true\n",
    )?;
    let error = runner
        .reload()
        .expect_err("unsupported required target must reject reload");

    assert!(matches!(error, DevError::ReloadFailed { .. }));
    assert_eq!(
        runner
            .session()
            .expect("rollback must restore previous session")
            .digest(),
        &previous
    );
    assert_eq!(runner.state(), Some(ExtensionState::Active));
    runner.shutdown()?;
    Ok(())
}
