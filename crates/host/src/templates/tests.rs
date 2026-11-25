//! Native template safety, independence, resource closure and stopped export.
use super::*;
use rintawa_artifacts::{AssetDigest, RtwManifest, RtwPackEntry, pack_entries};
use rintawa_sdk::{
    types::ExtensionId,
    world::{ItemId, WorldId},
};
use rintawa_world::{PropertyDefinition, StateChange, WorldTransaction};
use serde_json::json;
fn package(entries: Vec<(&str, Vec<u8>)>, content: &str, entry: &str) -> anyhow::Result<Vec<u8>> {
    Ok(pack_entries(
        &RtwManifest {
            format: 1,
            content: ContentType::parse(content)?,
            entry: ArtifactPath::parse(entry)?,
        },
        entries
            .into_iter()
            .map(|(name, bytes)| Ok(RtwPackEntry::new(ArtifactPath::parse(name)?, bytes)))
            .collect::<anyhow::Result<Vec<_>>>()?,
        RtwLimits::default(),
    )?)
}
fn definition() -> serde_json::Value {
    json!({"type":"object","properties":{"text":{"type":"string"},"resource":{"type":"object"}},"required":["text","resource"],"additionalProperties":false})
}
struct Fixture {
    bytes: Vec<u8>,
    item: ItemId,
    relation: ItemId,
    reference: AssetRef,
}
fn fixture(directory: &Path) -> anyhow::Result<Fixture> {
    let declaration = json!({"format":1,"properties":[{"key":"story.record@1","componentId":"backend","schema":definition(),"assetRefs":"browser-v1","portable":true}]});
    let extension = package(vec![("manifest.toml", b"id = 'story.test'\nname = 'Story'\nversion = '1.0.0'\nsdk = '^0.0'\n[[components]]\nid = 'backend'\nkind = 'runtime'\ntarget = 'rintawa.runtime.wasm-component@1'\nentry = 'backend.wasm'\n[components.permissions]\nruntime = ['background-task']\n".to_vec()),("backend.wasm",b"not executed by install/create/export".to_vec()),("world-portability.json",serde_json::to_vec(&declaration)?)],"rintawa.extension@1","manifest.toml")?;
    let digest = ArtifactDigest::sha256(&extension);
    let seed = directory.join("seed-source.sqlite");
    let storage = SqliteWorldStorage::create_with_manifest(
        &seed,
        WorldId::new(),
        &WorldMetadata::new("Source", None, None)?.write_to_manifest(&WorldManifest::default())?,
    )?;
    storage.register_definition(&PropertyDefinition::new(
        "story.record@1".parse()?,
        ExtensionId::new("story.test"),
        definition(),
    ))?;
    let (item, other, relation) = (ItemId::new(), ItemId::new(), ItemId::new());
    let asset = b"exact portable source";
    let reference = AssetRef::new(
        AssetDigest::sha256(asset),
        asset.len() as u64,
        "application/json",
    )?;
    let mut tx = WorldTransaction::new();
    tx.push_change(StateChange::CreateObject { id: item });
    tx.push_change(StateChange::CreateObject { id: other });
    tx.push_change(StateChange::CreateRelation {
        id: relation,
        subject: item,
        object: other,
    });
    tx.push_change(StateChange::SetProperty{item,key:"story.record@1".parse()?,data:json!({"text":"saved","resource":{"digest":reference.digest,"size":reference.size.to_string(),"mediaType":"application/json"}})});
    storage.apply(&tx)?;
    let copy = directory.join("seed-copy.sqlite");
    storage.export_snapshot(&copy)?;
    drop(storage);
    let data = json!({"format":1,"name":"Independent story","components":[{"slot":"story","artifact":digest,"enabled":true}],"composition":{"requestedGrants":[{"slot":"story","componentId":"backend","permission":"background-task"}]},"seed":"seed.sqlite","assets":[{"reference":reference,"path":"assets/source"}],"artifacts":[{"digest":digest,"path":"artifacts/story.rtw"}]});
    let bytes = package(
        vec![
            ("template.json", serde_json::to_vec(&data)?),
            ("seed.sqlite", std::fs::read(copy)?),
            ("assets/source", asset.to_vec()),
            ("artifacts/story.rtw", extension),
        ],
        CONTENT,
        "template.json",
    )?;
    Ok(Fixture {
        bytes,
        item,
        relation,
        reference,
    })
}
#[test]
fn template_roundtrip_forks_graph_assets_and_approvals_in_clean_home() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let source = fixture(root.path())?;
    let package = root.path().join("story.rtw");
    std::fs::write(&package, &source.bytes)?;
    let home = HostHome::open(root.path().join("home"))?;
    let installed = home.install_package(&package)?;
    assert!(home.load_profile()?.activations.is_empty());
    assert!(home.list_worlds()?.is_empty());
    assert_eq!(home.list_templates()?.len(), 1);
    assert!(matches!(
        home.create_world_from_template(&installed.digest, None, false),
        Err(HostError::TemplateApprovalRequired(_))
    ));
    assert!(home.list_worlds()?.is_empty());
    let a = home
        .create_world_from_template(&installed.digest, Some("A"), true)?
        .world;
    let b = home
        .create_world_from_template(&installed.digest, Some("B"), true)?
        .world;
    assert_ne!(a.id, b.id);
    let pa = home.load_world_composition(a.id)?;
    let pb = home.load_world_composition(b.id)?;
    assert_ne!(pa.activations[0].instance_id, pb.activations[0].instance_id);
    let sa = home.open_world_storage(a.id)?;
    let sb = home.open_world_storage(b.id)?;
    assert_eq!(
        sa.load_item(source.relation)?,
        sb.load_item(source.relation)?
    );
    assert_eq!(
        sa.load_property(source.item, &"story.record@1".parse()?)?
            .unwrap()
            .data()["resource"]["size"],
        source.reference.size.to_string()
    );
    assert!(
        home.export_world_template(a.id, root.path().join("busy.rtw"))
            .is_err()
    );
    assert!(!root.path().join("busy.rtw").exists());
    let mut tx = WorldTransaction::new();
    tx.push_change(StateChange::SetProperty{item:source.item,key:"story.record@1".parse()?,data:json!({"text":"only A","resource":{"digest":source.reference.digest,"size":source.reference.size.to_string(),"mediaType":"application/json"}})});
    sa.apply(&tx)?;
    assert_eq!(
        sb.load_property(source.item, &"story.record@1".parse()?)?
            .unwrap()
            .data()["text"],
        "saved"
    );
    drop(sa);
    drop(sb);
    let exported = root.path().join("export.rtw");
    home.export_world_template(a.id, &exported)?;
    assert!(home.export_world_template(a.id, &exported).is_err());
    let clean = HostHome::open(root.path().join("clean"))?;
    let restored = clean.install_package(&exported)?;
    assert!(clean.load_profile()?.activations.is_empty());
    assert!(matches!(
        clean.create_world_from_template(&restored.digest, None, false),
        Err(HostError::TemplateApprovalRequired(_))
    ));
    let fork = clean
        .create_world_from_template(&restored.digest, None, true)?
        .world;
    assert_ne!(fork.id, a.id);
    clean.asset_store.verify(&source.reference)?;
    let data = clean.open_world_storage(fork.id)?;
    assert!(data.load_item(source.relation)?.is_some());
    assert_eq!(
        data.load_property(source.item, &"story.record@1".parse()?)?
            .unwrap()
            .data()["text"],
        "only A"
    );
    assert_eq!(
        home.list_templates()?.len(),
        1,
        "export did not install itself"
    );
    Ok(())
}
#[test]
fn template_rejects_corrupt_embedded_resources_without_publishing_world() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let good = fixture(root.path())?;
    let original = root.path().join("good.rtw");
    std::fs::write(&original, &good.bytes)?;
    let mut archive = RtwArchive::open(&original, RtwLimits::default())?;
    let paths = archive
        .entries()
        .map(|e| e.path.clone())
        .filter(|p| p.as_str() != "rtw.toml")
        .collect::<Vec<_>>();
    let entries = paths
        .into_iter()
        .map(|p| {
            let mut bytes = archive.read(&p).unwrap();
            if p.as_str() == "assets/source" {
                bytes[0] ^= 1;
            }
            RtwPackEntry::new(p, bytes)
        })
        .collect::<Vec<_>>();
    let broken = pack_entries(archive.manifest(), entries, RtwLimits::default())?;
    let path = root.path().join("bad.rtw");
    std::fs::write(&path, broken)?;
    let home = HostHome::open(root.path().join("home"))?;
    assert!(home.install_package(path).is_err());
    assert!(home.list_worlds()?.is_empty());
    assert!(home.list_templates()?.is_empty());
    assert!(home.load_profile()?.activations.is_empty());
    Ok(())
}
#[test]
fn staging_recovery_removes_only_abandoned_private_construction() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let live = Stage::new(&home)?;
    let orphan = Stage::new(&home)?;
    let orphan_path = orphan.path().to_owned();
    let Stage {
        directory,
        _ownership,
    } = orphan;
    let _ = directory.keep();
    drop(_ownership);
    home.recover_world_staging()?;
    assert!(live.path().exists());
    assert!(!orphan_path.exists());
    assert!(home.list_worlds()?.is_empty());
    Ok(())
}

#[test]
fn template_rejects_embedded_installers_and_crc_failures_before_shared_publication()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let nested = package(
        vec![("template.json", b"{}".to_vec())],
        CONTENT,
        "template.json",
    )?;
    let digest = ArtifactDigest::sha256(&nested);
    for referenced in [false, true] {
        let slots = if referenced {
            json!([{"slot":"bad","artifact":digest,"enabled":true}])
        } else {
            json!([])
        };
        let body = json!({"format":1,"name":"Outer","components":slots,"composition":{},"artifacts":[{"digest":digest,"path":"inner.rtw"}]});
        let outer = package(
            vec![
                ("template.json", serde_json::to_vec(&body)?),
                ("inner.rtw", nested.clone()),
            ],
            CONTENT,
            "template.json",
        )?;
        let path = root.path().join(format!("nested-{referenced}.rtw"));
        std::fs::write(&path, outer)?;
        assert!(home.install_package(path).is_err());
        assert!(!home.store.digests()?.contains(&digest));
    }
    let fixture = fixture(root.path())?;
    let good = root.path().join("good.rtw");
    std::fs::write(&good, fixture.bytes)?;
    let mut archive = RtwArchive::open(&good, RtwLimits::default())?;
    let mut inner = archive.read(&ArtifactPath::parse("artifacts/story.rtw")?)?;
    let mut changed = false;
    for index in 0..inner.len().saturating_sub(46) {
        if &inner[index..index + 4] == b"PK\x01\x02" {
            let length = u16::from_le_bytes([inner[index + 28], inner[index + 29]]) as usize;
            if inner.get(index + 46..index + 46 + length) == Some(b"backend.wasm".as_slice()) {
                inner[index + 16] ^= 1;
                changed = true;
                break;
            }
        }
    }
    assert!(changed);
    let digest = ArtifactDigest::sha256(&inner);
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&archive.read(&ArtifactPath::parse("template.json")?)?)?;
    descriptor["components"][0]["artifact"] = json!(digest);
    descriptor["artifacts"][0]["digest"] = json!(digest);
    let paths = archive
        .entries()
        .map(|entry| entry.path.clone())
        .filter(|path| path.as_str() != "rtw.toml")
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    for path in paths {
        let bytes = match path.as_str() {
            "template.json" => serde_json::to_vec(&descriptor)?,
            "artifacts/story.rtw" => inner.clone(),
            _ => archive.read(&path)?,
        };
        entries.push((path.to_string(), bytes));
    }
    let outer = package(
        entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.clone()))
            .collect(),
        CONTENT,
        "template.json",
    )?;
    let path = root.path().join("bad-inner-crc.rtw");
    std::fs::write(&path, outer)?;
    assert!(home.install_package(path).is_err());
    assert!(!home.store.digests()?.contains(&digest));
    assert!(home.list_worlds()?.is_empty());
    assert!(home.list_templates()?.is_empty());
    // An older generic CAS ingestion path does not make this dependency trusted.
    home.import_rtw_bytes(&inner)?;
    descriptor["artifacts"] = json!([]);
    let thin_entries = entries
        .iter()
        .filter(|(name, _)| name != "artifacts/story.rtw")
        .map(|(name, bytes)| {
            Ok((
                name.as_str(),
                if name == "template.json" {
                    serde_json::to_vec(&descriptor)?
                } else {
                    bytes.clone()
                },
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let thin = package(thin_entries, CONTENT, "template.json")?;
    let thin_path = root.path().join("thin-bad-crc.rtw");
    std::fs::write(&thin_path, &thin)?;
    let error = home.install_package(thin_path).unwrap_err();
    assert!(error.to_string().to_lowercase().contains("checksum"));
    let raw = home.import_rtw_bytes(&thin)?;
    let error = home
        .create_world_from_template(raw.digest(), None, true)
        .unwrap_err();
    assert!(error.to_string().to_lowercase().contains("checksum"));
    assert!(home.list_worlds()?.is_empty());
    // A stopped old World may also reference a generic-imported bad dependency.
    // Export must reject before publishing an archive that clean-home import rejects.
    let valid = home.install_package(&good)?;
    let world = home
        .create_world_from_template(&valid.digest, None, true)?
        .world;
    home.select_world_stored_rtw(world.id, &digest, Some(true))?;
    let output = root.path().join("must-not-publish.rtw");
    let error = home.export_world_template(world.id, &output).unwrap_err();
    assert!(error.to_string().to_lowercase().contains("checksum"));
    assert!(!output.exists());
    Ok(())
}

#[test]
fn template_catalog_reports_invalid_generic_import_without_hiding_valid_templates()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let invalid_bytes = package(
        vec![("template.json", b"{}".to_vec())],
        CONTENT,
        "template.json",
    )?;
    let invalid = home.import_rtw_bytes(&invalid_bytes)?.digest().clone();
    let valid_bytes = package(
        vec![(
            "template.json",
            serde_json::to_vec(
                &json!({"format":1,"name":"Valid","components":[],"composition":{}}),
            )?,
        )],
        CONTENT,
        "template.json",
    )?;
    let file = root.path().join("valid.rtw");
    std::fs::write(&file, valid_bytes)?;
    home.install_package(file)?;
    let catalog = home.list_templates()?;
    assert_eq!(catalog.len(), 2);
    assert_eq!(
        catalog
            .iter()
            .filter(|entry| entry.invalid_reason.is_none())
            .count(),
        1
    );
    assert!(
        catalog
            .iter()
            .find(|entry| entry.digest == invalid)
            .unwrap()
            .invalid_reason
            .is_some()
    );
    assert!(
        home.create_world_from_template(&invalid, None, true)
            .is_err()
    );
    assert!(home.list_worlds()?.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn template_staging_is_private_before_any_source_data_is_written() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path())?;
    let stage = Stage::new(&home)?;
    assert_eq!(
        std::fs::metadata(stage.path())?.permissions().mode() & 0o077,
        0
    );
    Ok(())
}
