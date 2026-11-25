//! Atomic upfront local approvals for freshly materialized Worlds.
use super::*;
use rintawa_artifacts::{RtwManifest, RtwPackEntry, pack_entries};
use serde_json::{Value, json};

fn package(content: &str, entry: &str, files: &[(&str, &[u8])]) -> anyhow::Result<Vec<u8>> {
    let files = files
        .iter()
        .map(|&(path, bytes)| {
            Ok(RtwPackEntry::new(
                ArtifactPath::parse(path)?,
                bytes.to_vec(),
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(pack_entries(
        &RtwManifest {
            format: 1,
            content: ContentType::parse(content)?,
            entry: ArtifactPath::parse(entry)?,
        },
        files,
        RtwLimits::default(),
    )?)
}

#[cfg(target_os = "linux")]
#[test]
fn managed_secret_values_and_bindings_are_absent_from_world_exports_and_fresh_copies()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("source"))?;
    home.select_secret_backend(crate::SecretBackend::LocalFile)?;
    let request = installed_fixture(&home)?;
    let created = home.create_world_from_template_with_permissions(&request.to_string())?;
    let sentinel = "synthetic shared export sentinel";
    let entry = home.create_shared_secret(
        "private label",
        &rintawa_sdk::secrets::SecretValue::new(sentinel),
    )?;
    let mut profile = home.load_world_composition(created.world.id)?;
    for grant in &mut profile.secret_read_grants {
        grant.entry_id = Some(entry.id);
    }
    home.save_world_composition(created.world.id, &profile)?;
    assert_eq!(home.list_secret_consumers()?.len(), 2);
    assert!(
        profile
            .materialize_all(crate::world_runtime_scope_id(
                rintawa_sdk::world::WorldId::new()
            ))
            .secret_read_grants
            .is_empty()
    );
    let database = std::fs::read(
        home.root()
            .join("worlds")
            .join(created.world.id.to_string())
            .join("world.sqlite"),
    )?;
    assert!(
        !database
            .windows(sentinel.len())
            .any(|bytes| bytes == sentinel.as_bytes())
    );
    let output = root.path().join("export.rtw");
    home.export_world_template(created.world.id, &output)?;
    let mut archive = rintawa_artifacts::RtwArchive::open(&output, RtwLimits::default())?;
    let paths = archive
        .entries()
        .map(|e| e.path.clone())
        .collect::<Vec<_>>();
    for path in paths {
        assert!(!path.as_str().contains("credentials"));
        let bytes = archive.read(&path)?;
        for excluded in [
            sentinel.to_owned(),
            "private label".to_owned(),
            entry.id.to_string(),
            entry.id.simple().to_string(),
        ] {
            assert!(
                !bytes
                    .windows(excluded.len())
                    .any(|part| part == excluded.as_bytes())
            );
        }
    }
    let clean = HostHome::open(root.path().join("clean"))?;
    for destination in [&home, &clean] {
        let installed = destination.install_package(&output)?;
        let copy = destination.create_world_from_template(&installed.digest, None, true)?;
        assert!(
            destination
                .load_world_composition(copy.world.id)?
                .secret_read_grants
                .is_empty()
        );
    }
    Ok(())
}

fn installed_fixture(home: &HostHome) -> anyhow::Result<Value> {
    let manifest = br#"
id = "fixture.creation-permissions"
name = "Creation permission fixture"
version = "0.1.0"
sdk = "^0.0"
[[components]]
id = "backend"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "backend.wasm"
[components.permissions]
runtime = ["http-request"]
secret-read = ["connector.credentials.*"]
"#;
    let artifact = package(
        "rintawa.extension@1",
        "manifest.toml",
        &[
            ("manifest.toml", manifest),
            ("backend.wasm", b"offline fixture"),
        ],
    )?;
    let digest = ArtifactDigest::sha256(&artifact);
    let descriptor = serde_json::to_vec(&json!({
        "format": 1,
        "name": "Create World",
        "components": [
            {"slot": "alpha", "artifact": digest, "enabled": true},
            {"slot": "beta", "artifact": digest, "enabled": true},
            {"slot": "gamma", "artifact": digest, "enabled": false}
        ],
        "composition": {"requestedGrants": [
            {"slot": "alpha", "componentId": "backend", "permission": "http-request"},
            {"slot": "beta", "componentId": "backend", "permission": "http-request"}
        ]},
        "artifacts": [{"digest": digest, "path": "connector.rtw"}]
    }))?;
    let template = package(
        CONTENT,
        "template.json",
        &[("template.json", &descriptor), ("connector.rtw", &artifact)],
    )?;
    let source = home.root.join("template.rtw");
    std::fs::write(&source, template)?;
    let installed = home.install_package(&source)?;
    Ok(json!({
        "digest": installed.digest,
        "title": "New World",
        "approveRuntimePermissions": true,
        "permissions": [
            {"slot": "beta", "componentId": "backend", "artifact": digest,
             "httpOrigins": ["http://localhost:9002"], "secretRead": ["connector.credentials.second"]},
            {"slot": "alpha", "componentId": "backend", "artifact": digest,
             "httpOrigins": ["http://localhost:9001"], "secretRead": ["connector.credentials.first"]}
        ]
    }))
}

#[test]
fn template_creation_permissions_apply_to_fresh_exact_slots_and_stay_local() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let request = installed_fixture(&home)?;
    let created = home.create_world_from_template_with_permissions(&request.to_string())?;
    let profile = home.load_world_composition(created.world.id)?;
    assert_eq!(created.world.title, "New World");
    assert_eq!(profile.http_loopback_origins.len(), 2);
    assert_eq!(profile.secret_read_grants.len(), 2);
    let scope = crate::world_runtime_scope_id(created.world.id);
    for (index, origin, path) in [
        (0, "http://localhost:9001", "connector.credentials.first"),
        (1, "http://localhost:9002", "connector.credentials.second"),
    ] {
        let instance = &profile.activations[index].instance_id;
        assert!(profile.http_loopback_origins.iter().any(|grant| {
            grant.scope_id == scope
                && &grant.instance_id == instance
                && grant.component_id.as_str() == "backend"
                && grant.origin == origin
        }));
        assert!(profile.secret_read_grants.iter().any(|grant| {
            grant.scope_id == scope
                && &grant.instance_id == instance
                && grant.component_id.as_str() == "backend"
                && grant.path.as_str() == path
        }));
    }
    let output = root.path().join("export.rtw");
    home.export_world_template(created.world.id, &output)?;
    let clean = HostHome::open(root.path().join("clean"))?;
    let exported = clean.install_package(output)?;
    let copy = clean.create_world_from_template(&exported.digest, None, true)?;
    let copied_profile = clean.load_world_composition(copy.world.id)?;
    assert!(copied_profile.http_loopback_origins.is_empty());
    assert!(copied_profile.secret_read_grants.is_empty());
    assert_eq!(copied_profile.runtime_permissions.len(), 2);
    Ok(())
}

#[test]
fn template_creation_permissions_reject_invalid_consent_without_publication() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let valid = installed_fixture(&home)?;
    for (pointer, replacement) in [
        ("/approveRuntimePermissions", json!(false)),
        ("/permissions/0/slot", json!("missing")),
        ("/permissions/0/slot", json!("gamma")),
        ("/permissions/0/componentId", json!("missing")),
        ("/permissions/0/artifact", valid["digest"].clone()),
        (
            "/permissions/0/secretRead",
            json!(["other.credentials.second"]),
        ),
        (
            "/permissions/0/secretRead",
            json!(["connector.credentials.*"]),
        ),
        (
            "/permissions/0/secretRead",
            json!(vec!["connector.credentials.second"; 64]),
        ),
        (
            "/permissions/0/httpOrigins",
            json!(["http://localhost:9002/"]),
        ),
        (
            "/permissions/0/httpOrigins",
            json!(["http://localhost:9002", "http://localhost:9002"]),
        ),
        ("/permissions/1", valid["permissions"][0].clone()),
    ] {
        let mut request = valid.clone();
        *request.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            home.create_world_from_template_with_permissions(&request.to_string())
                .is_err(),
            "accepted {pointer} in {request}"
        );
        assert!(home.list_worlds()?.is_empty());
    }
    let mut unknown = valid.clone();
    unknown["permissions"][0]["label"] = json!("unknown");
    assert!(
        home.create_world_from_template_with_permissions(&unknown.to_string())
            .is_err()
    );
    let duplicate =
        valid
            .to_string()
            .replacen("\"title\":", "\"title\":\"duplicate\",\"title\":", 1);
    assert!(
        home.create_world_from_template_with_permissions(&duplicate)
            .is_err()
    );
    assert!(home.list_worlds()?.is_empty());
    assert_eq!(
        std::fs::read_dir(home.root.join("world-staging"))?.count(),
        0
    );
    Ok(())
}

#[test]
fn template_creation_permissions_fail_before_publication_if_profile_cannot_save()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let request = installed_fixture(&home)?;
    crate::BEFORE_INITIAL_COMPOSITION.with(|hook| {
        let staging = home.root.join("world-staging");
        *hook.borrow_mut() = Some(Box::new(move |_| {
            let publish_stage = std::fs::read_dir(staging)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.join(crate::WORLD_DATABASE_FILE).is_file())
                .unwrap();
            std::fs::create_dir(publish_stage.join(crate::WORLD_COMPOSITION_FILE)).unwrap();
        }));
    });
    assert!(matches!(
        home.create_world_from_template_with_permissions(&request.to_string()),
        Err(HostError::Io(_))
    ));
    assert!(home.list_worlds()?.is_empty());
    assert_eq!(
        std::fs::read_dir(home.root.join("world-staging"))?.count(),
        0
    );
    Ok(())
}
