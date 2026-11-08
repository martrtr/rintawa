//! Real CLI smoke test against a previously nonexistent local Host home.
//!
//! The test launches the compiled binary, not library helpers, so it covers
//! argument routing, first-run initialization, subsequent reads and shutdown.

use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};
use rintawa_host::HostHome;
use rintawa_sdk::world::WorldId;

fn cli(home: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_rintawa"))
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .context("launch rintawa binary")?;
    if !output.status.success() {
        bail!(
            "rintawa {args:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("CLI stdout must be UTF-8")
}

#[test]
fn test_should_bootstrap_two_worlds_from_empty_home_and_reopen_both() -> Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("brand-new-rintawa-home");
    assert!(!home.exists());

    let first = cli(&home, &["world-create"])?;
    let first_id: WorldId = first
        .lines()
        .find_map(|line| line.strip_prefix("world = "))
        .context("CLI must print the first World ID")?
        .parse()?;
    assert!(first.contains("title = New World"));

    // Starting the real application with no installed extensions must not
    // require legacy RTWs or a prior World import.
    cli(&home, &["run", "--once"])?;

    let second = cli(&home, &["world-create"])?;
    let second_id: WorldId = second
        .lines()
        .find_map(|line| line.strip_prefix("world = "))
        .context("CLI must print the second World ID")?
        .parse()?;
    assert_ne!(first_id, second_id);

    let listed = cli(&home, &["world-list"])?;
    assert_eq!(listed.lines().count(), 2);
    assert!(listed.contains(&first_id.to_string()));
    assert!(listed.contains(&second_id.to_string()));

    for world_id in [first_id, second_id] {
        let info = cli(&home, &["world-info", &world_id.to_string()])?;
        assert!(info.contains(&format!("world = {world_id}")));
        assert!(info.contains("title = New World"));
        assert!(info.contains("properties = 0"));

        let world_dir = home.join("worlds").join(world_id.to_string());
        assert!(world_dir.join("world.sqlite").is_file());
        assert!(world_dir.join("composition.toml").is_file());
        assert!(!world_dir.join("catalog.toml").exists());
    }
    assert!(!home.join("identity.toml").exists());
    assert!(!home.join("content").join("library.toml").exists());

    cli(&home, &["run", "--once"])?;
    let reopened = HostHome::open(&home)?;
    assert_eq!(reopened.list_worlds()?.len(), 2);
    for world_id in [first_id, second_id] {
        let state = reopened.load_world_state(world_id)?;
        assert_eq!(state.id(), world_id);
        assert!(state.properties().is_empty());
        assert_eq!(
            state.manifest().as_value()["metadata"]["title"],
            "New World"
        );
    }
    Ok(())
}
