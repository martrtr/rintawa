//! Integration tests for development-session lifecycle behavior.

use std::fs;

use rintawa_dev::{DevProject, DevSession};
use rintawa_extension_engine::ExtensionState;
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

fn write_headless_extension(path: &std::path::Path) -> std::io::Result<()> {
    fs::write(
        path.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        path.join("manifest.toml"),
        "id = \"dev.session\"\nname = \"Dev Session\"\nversion = \"0.1.0\"\nsdk = \"^0.0\"\n",
    )?;
    Ok(())
}

#[test]
fn test_should_run_snapshot_through_extension_engine_lifecycle() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_headless_extension(temp.path())?;
    let project = DevProject::open(temp.path())?;
    let session = DevSession::start(
        &project,
        ExtensionInstanceId::new("test-instance"),
        RuntimeScopeId::new("test-scope"),
    )?;

    assert_eq!(session.extension_id().as_str(), "dev.session");
    assert_eq!(session.instance_id().as_str(), "test-instance");
    assert_eq!(session.state(), Some(ExtensionState::Active));
    session.shutdown()?;
    Ok(())
}
