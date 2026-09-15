use std::fs;

use rintawa_dev::{DevError, DevProject};

fn write_minimal_rtw_root(path: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    fs::write(
        path.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        path.join("manifest.toml"),
        "id = \"dev.example\"\nname = \"Dev Example\"\nversion = \"0.1.0\"\nsdk = \"^0.0\"\n",
    )?;
    Ok(())
}

#[test]
fn test_should_prepare_zero_config_rtw_snapshot() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_minimal_rtw_root(temp.path())?;

    let project = DevProject::open(temp.path())?;
    let snapshot = project.prepare_snapshot()?;
    let archive = snapshot.store().open_artifact(snapshot.digest())?;

    assert_eq!(
        archive.manifest().content.to_string(),
        "rintawa.extension@1"
    );
    Ok(())
}

#[test]
fn test_should_reject_artifact_root_parent_escape() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("rintawa-dev.toml"),
        "schema = 1\nartifact-root = \"../outside\"\n",
    )?;

    let error = DevProject::open(temp.path()).expect_err("parent escape must be rejected");
    assert!(matches!(error, DevError::InvalidArtifactRoot(path) if path == "../outside"));
    Ok(())
}

#[test]
fn test_should_reject_unknown_config_schema() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("rintawa-dev.toml"),
        "schema = 2\nartifact-root = \".\"\n",
    )?;

    let error = DevProject::open(temp.path()).expect_err("unknown schema must be rejected");
    assert!(matches!(error, DevError::UnsupportedConfigSchema(2)));
    Ok(())
}

#[test]
fn test_should_run_explicit_build_command_before_snapshot() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_minimal_rtw_root(temp.path())?;
    let cargo = std::env::var("CARGO")?;
    let config = format!("schema = 1\nartifact-root = \".\"\nbuild = [{cargo:?}, \"--version\"]\n");
    fs::write(temp.path().join("rintawa-dev.toml"), config)?;

    let project = DevProject::open(temp.path())?;
    let snapshot = project.prepare_snapshot()?;
    snapshot.store().verify(snapshot.digest())?;
    Ok(())
}

#[test]
fn test_should_reject_empty_build_program() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("rintawa-dev.toml"),
        "schema = 1\nbuild = [\"\"]\n",
    )?;

    let error = DevProject::open(temp.path()).expect_err("empty build program must be rejected");
    assert!(matches!(error, DevError::EmptyBuildCommand));
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_should_reject_artifact_root_symlink_escape() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let project_root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    write_minimal_rtw_root(outside.path())?;
    symlink(outside.path(), project_root.path().join("build"))?;
    fs::write(
        project_root.path().join("rintawa-dev.toml"),
        "schema = 1\nartifact-root = \"build\"\n",
    )?;

    let project = DevProject::open(project_root.path())?;
    let error = match project.prepare_snapshot() {
        Ok(_) => panic!("artifact root symlink must not escape the selected project"),
        Err(error) => error,
    };
    assert!(matches!(error, DevError::InvalidArtifactRoot(path) if path == "build"));
    Ok(())
}
