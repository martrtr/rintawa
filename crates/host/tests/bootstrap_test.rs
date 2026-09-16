use std::fs;

use rintawa_artifacts::{ImportDisposition, RtwLimits, pack_directory};
use rintawa_host::{HostHome, HostRuntime};

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
    HostRuntime::start(&restarted)?.shutdown()?;
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
