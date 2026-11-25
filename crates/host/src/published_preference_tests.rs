//! Tests for exact published preference persistence and concurrent saves.

use super::*;

const KEY: &str = "fixture.published.v1";
const ORIGINAL: &str = "{ \"entries\": [] }";

fn publisher() -> anyhow::Result<(tempfile::TempDir, HostHome, ActivationRecord, ComponentRef)> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let activation = ActivationRecord {
        subject: "fixture.publisher".into(),
        content: EXTENSION_CONTENT_V1.parse()?,
        artifact: ArtifactDigest::sha256(b"exact fixture bytes"),
        instance_id: ExtensionInstanceId::new("fixture.publisher"),
        scope_id: RuntimeScopeId::new(HOST_SCOPE),
        enabled: true,
    };
    let owner = ComponentRef::new(activation.instance_id.clone(), "backend");
    let profile = CompositionProfile {
        activations: vec![activation.clone()],
        ..Default::default()
    };
    profile.save(&home.profile_path)?;
    Ok((root, home, activation, owner))
}

#[test]
fn test_published_preference_compares_raw_strings_and_absence() -> anyhow::Result<()> {
    let (_root, home, activation, owner) = publisher()?;
    let compare = |expected, value| {
        home.compare_exchange_published_preference(
            &owner,
            &activation.artifact,
            KEY,
            expected,
            value,
        )
    };
    assert!(!compare(Some(""), Some(""))?);
    assert!(compare(None, Some(""))?);
    assert!(!compare(None, None)?);
    assert!(compare(Some(""), Some(ORIGINAL))?);
    assert!(!compare(Some("{\"entries\":[]}"), None)?);
    assert!(!compare(
        Some("{ \"entries\": [], \"primary\": null }"),
        None
    )?);

    let reopened = HostHome::open(home.root())?;
    assert_eq!(
        reopened
            .get_preference(&activation.scope_id, &owner, KEY)?
            .as_deref(),
        Some(ORIGINAL)
    );
    assert!(compare(Some(ORIGINAL), Some(ORIGINAL))?);
    assert!(compare(Some(ORIGINAL), None)?);
    assert_eq!(
        reopened.get_preference(&activation.scope_id, &owner, KEY)?,
        None
    );
    Ok(())
}

#[test]
fn test_published_preference_rejects_busy_or_changed_baseline_selection() -> anyhow::Result<()> {
    let (_root, home, activation, owner) = publisher()?;
    let compare = |expected, value| {
        home.compare_exchange_published_preference(
            &owner,
            &activation.artifact,
            KEY,
            expected,
            value,
        )
    };
    assert!(compare(None, Some(ORIGINAL))?);
    let lock = home.lock_composition_scope(&activation.scope_id)?;
    assert!(
        matches!(compare(Some(ORIGINAL), None), Err(HostError::Io(error))
        if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    drop(lock);

    home.set_enabled_in_scope(&activation.scope_id, &activation.subject, false)?;
    assert!(matches!(
        compare(Some(ORIGINAL), None),
        Err(HostError::ActivationNotFound(_))
    ));
    let mut profile = home.load_profile()?;
    let new_digest = ArtifactDigest::sha256(b"replacement fixture bytes");
    profile.activations[0].enabled = true;
    profile.activations[0].artifact = new_digest.clone();
    profile.save(&home.profile_path)?;
    assert!(matches!(
        compare(Some(ORIGINAL), None),
        Err(HostError::ActivationNotFound(_))
    ));
    assert_eq!(
        home.get_preference(&activation.scope_id, &owner, KEY)?
            .as_deref(),
        Some(ORIGINAL)
    );
    assert!(home.compare_exchange_published_preference(
        &owner,
        &new_digest,
        KEY,
        Some(ORIGINAL),
        None
    )?);
    Ok(())
}

#[test]
fn test_published_preference_contenders_cannot_both_commit() -> anyhow::Result<()> {
    let (_root, home, activation, owner) = publisher()?;
    let compare = |value| {
        home.compare_exchange_published_preference(
            &owner,
            &activation.artifact,
            KEY,
            Some(ORIGINAL),
            Some(value),
        )
    };
    home.set_preference(
        activation.scope_id.clone(),
        owner.clone(),
        KEY.into(),
        ORIGINAL.into(),
    )?;
    let parallel = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        let handles = ["first World", "second World"].map(|value| {
            let parallel = &parallel;
            let compare = &compare;
            scope.spawn(move || {
                parallel.wait();
                compare(value)
            })
        });
        parallel.wait();
        let contenders = handles.map(|handle| handle.join().unwrap());
        assert_eq!(
            contenders
                .iter()
                .filter(|result| matches!(result, Ok(true)))
                .count(),
            1
        );
        for contender in contenders {
            if let Err(error) = contender {
                assert!(
                    matches!(error, HostError::Io(error) if error.kind() == std::io::ErrorKind::WouldBlock)
                );
            }
        }
    });
    assert!(!compare("stale Home save")?);
    let actual = home.get_preference(&activation.scope_id, &owner, KEY)?;
    assert!(matches!(
        actual.as_deref(),
        Some("first World" | "second World")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_published_preference_save_failure_preserves_profile() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let (_root, home, activation, owner) = publisher()?;
    let compare = |expected, value| {
        home.compare_exchange_published_preference(
            &owner,
            &activation.artifact,
            KEY,
            expected,
            value,
        )
    };
    assert!(compare(None, Some(ORIGINAL))?);
    let before = std::fs::read(&home.profile_path)?;
    let directory = home.profile_path.parent().unwrap();
    let permissions = std::fs::metadata(directory)?.permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o555))?;
    // A privileged process may bypass this filesystem fault.
    let result = if tempfile::NamedTempFile::new_in(directory).is_err() {
        Some(compare(Some(ORIGINAL), Some("uncommitted")))
    } else {
        eprintln!("directory write denial cannot be enforced for this user");
        None
    };
    std::fs::set_permissions(directory, permissions)?;
    if let Some(result) = result {
        assert!(
            matches!(result, Err(HostError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert_eq!(std::fs::read(&home.profile_path)?, before);
    }
    Ok(())
}
