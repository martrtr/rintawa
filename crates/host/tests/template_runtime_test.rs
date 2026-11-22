//! Real rtwKit WASM acceptance for template lifecycle and exact Host provider binding.

use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result, ensure};
use rintawa_artifacts::{
    ArtifactDigest, AssetRef, RtwArchive, RtwLimits, RtwPackEntry, pack_entries,
};
use rintawa_host::{HOST_SCOPE, HostError, HostHome, HostRuntime};
use rintawa_sdk::{
    contracts::ComponentRef,
    runtime_permissions::RuntimePermission,
    types::RuntimeScopeId,
    world::{ItemId, WorldId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::WorldCommand;
use serde_json::{Value, json};

fn artifacts() -> Result<std::path::PathBuf> {
    let path = std::env::var_os("RINTAWA_TEMPLATE_ARTIFACTS")
        .context("set RINTAWA_TEMPLATE_ARTIFACTS to real built rtwKit RTWs")?;
    Ok(path.into())
}

fn baseline(home: &HostHome, artifacts: &Path) -> Result<ArtifactDigest> {
    let installed = home.install_local_rtw(artifacts.join("web-runtime.rtw"), Some(true))?;
    let owner = ComponentRef::new(installed.activation.instance_id, "runtime");
    for permission in [
        RuntimePermission::BackgroundTask,
        RuntimePermission::LoopbackListen,
    ] {
        home.grant_runtime_permission(RuntimeScopeId::new(HOST_SCOPE), owner.clone(), permission)?;
    }
    Ok(installed.activation.digest)
}

fn command(host: &mut HostRuntime, world: WorldId, key: &str, payload: Value) -> Result<()> {
    host.submit_world_command(world, WorldCommand::system(key.parse()?, payload))?;
    Ok(())
}

fn storage(home: &HostHome, world: WorldId) -> Result<SqliteWorldStorage> {
    Ok(SqliteWorldStorage::open(
        home.root()
            .join("worlds")
            .join(world.to_string())
            .join("world.sqlite"),
    )?)
}

fn card_data(reference: &AssetRef) -> Value {
    json!({"name":"Fixture","source":{
        "digest":reference.digest.to_string(),
        "size":reference.size.to_string(),
        "mediaType":reference.media_type.to_string()
    },"portrait":null})
}

fn repack_runtime(source: &Path) -> Result<Vec<u8>> {
    let mut archive = RtwArchive::open(source, RtwLimits::default())?;
    let manifest = archive.manifest().clone();
    let paths = archive
        .entries()
        .map(|entry| entry.path.clone())
        .filter(|path| path.as_str() != "rtw.toml")
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    for path in paths {
        let mut data = archive.read(&path)?;
        if path.as_str() == "manifest.toml" {
            let text = String::from_utf8(data)?;
            ensure!(
                text.contains("version = \"0.1.0\""),
                "unexpected web-runtime manifest"
            );
            data = text
                .replacen("version = \"0.1.0\"", "version = \"0.1.1\"", 1)
                .into_bytes();
        }
        entries.push(RtwPackEntry::new(path, data));
    }
    Ok(pack_entries(&manifest, entries, RtwLimits::default())?)
}

fn repack_template(source: &Path, runtime: &[u8]) -> Result<Vec<u8>> {
    let mut archive = RtwArchive::open(source, RtwLimits::default())?;
    let manifest = archive.manifest().clone();
    let paths = archive
        .entries()
        .map(|entry| entry.path.clone())
        .filter(|path| path.as_str() != "rtw.toml")
        .collect::<Vec<_>>();
    let runtime_digest = ArtifactDigest::sha256(runtime).to_string();
    let mut entries = Vec::new();
    for path in paths {
        let data = match path.as_str() {
            "extensions/web-runtime.rtw" => runtime.to_vec(),
            "template.json" => {
                let mut descriptor: Value = serde_json::from_slice(&archive.read(&path)?)?;
                descriptor["hostRequirements"][0]["artifact"] = json!(runtime_digest);
                let artifact = descriptor["artifacts"]
                    .as_array_mut()
                    .context("missing embedded artifacts")?
                    .iter_mut()
                    .find(|row| row["path"] == "extensions/web-runtime.rtw")
                    .context("missing runtime artifact")?;
                artifact["digest"] = json!(runtime_digest);
                serde_json::to_vec(&descriptor)?
            }
            _ => archive.read(&path)?,
        };
        entries.push(RtwPackEntry::new(path, data));
    }
    Ok(pack_entries(&manifest, entries, RtwLimits::default())?)
}

#[test]
#[ignore = "requires built rtwKit RTWs via RINTAWA_TEMPLATE_ARTIFACTS"]
fn template_runtime_roundtrip_and_live_provider_digest() -> Result<()> {
    let artifacts = artifacts()?;
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    let original_runtime = baseline(&home, &artifacts)?;
    let template = home.install_package(artifacts.join("TextRoleplay.rtw"))?;
    ensure!(
        matches!(
            home.create_world_from_template(&template.digest, None, false),
            Err(HostError::TemplateApprovalRequired(_))
        ),
        "template creation without explicit World permission approval must fail"
    );
    ensure!(
        home.list_worlds()?.is_empty(),
        "approval failure published a World"
    );
    let a = home
        .create_world_from_template(&template.digest, Some("A"), true)?
        .world;
    let b = home
        .create_world_from_template(&template.digest, Some("B"), true)?
        .world;
    ensure!(a.id != b.id, "template reused World identity");
    let pa = home.load_world_composition(a.id)?;
    let pb = home.load_world_composition(b.id)?;
    ensure!(
        pa.activations.iter().all(|left| pb
            .activations
            .iter()
            .all(|right| left.instance_id != right.instance_id)),
        "template reused overlay instance identity"
    );
    let mut host = HostRuntime::start(&home)?;
    host.activate_world(&home, a.id)?;
    host.activate_world(&home, b.id)?;
    ensure!(
        host.is_world_active(a.id) && host.is_world_active(b.id),
        "real RTWs failed to activate"
    );

    let source_bytes = br#"{"spec":"chara_card_v2","spec_version":"2.0","data":{"name":"Fixture","description":"Local test"}}"#;
    let reference = home
        .asset_store()
        .import_bytes(source_bytes, "application/json")?
        .asset_ref()
        .clone();
    let character = ItemId::new();
    let conversation = ItemId::new();
    let message = ItemId::new();
    let entry = ItemId::new();
    command(
        &mut host,
        a.id,
        "rintawa.character.import@1",
        json!({"itemId":character,"card":card_data(&reference)}),
    )?;
    command(
        &mut host,
        a.id,
        "rintawa.chat.create@1",
        json!({"conversationId":conversation,"title":"First chat","characterId":character}),
    )?;
    command(
        &mut host,
        a.id,
        "rintawa.chat.append@1",
        json!({"conversationId":conversation,"messageId":message,"entryId":entry,"text":"Hello"}),
    )?;
    host.deactivate_world(a.id)?;
    host.deactivate_world(b.id)?;
    let a_data = storage(&home, a.id)?;
    let b_data = storage(&home, b.id)?;
    ensure!(
        a_data
            .load_property(character, &"rintawa.character.card@1".parse()?)?
            .is_some(),
        "real Character System did not commit"
    );
    ensure!(
        a_data
            .load_property(message, &"rintawa.chat.message@1".parse()?)?
            .is_some(),
        "real Chat System did not commit"
    );
    ensure!(
        b_data.load_item(conversation)?.is_none(),
        "World B saw World A data"
    );
    drop(a_data);
    drop(b_data);
    let exported = root.path().join("exported.rtw");
    home.export_world_template(a.id, &exported)?;
    host.shutdown()?;

    let clean = HostHome::open(root.path().join("clean"))?;
    baseline(&clean, &artifacts)?;
    let restored = clean.install_package(&exported)?;
    ensure!(
        matches!(
            clean.create_world_from_template(&restored.digest, None, false),
            Err(HostError::TemplateApprovalRequired(_))
        ),
        "exported grants approved themselves"
    );
    let c = clean
        .create_world_from_template(&restored.digest, Some("C"), true)?
        .world;
    let d = clean
        .create_world_from_template(&restored.digest, Some("D"), true)?
        .world;
    ensure!(a.id != c.id && c.id != d.id, "fork reused World identity");
    let pc = clean.load_world_composition(c.id)?;
    let pd = clean.load_world_composition(d.id)?;
    ensure!(
        pc.activations.iter().all(|left| pd
            .activations
            .iter()
            .all(|right| left.instance_id != right.instance_id)),
        "fork reused instance identity"
    );
    clean.asset_store().verify(&reference)?;
    let mut bytes = Vec::new();
    clean
        .asset_store()
        .open_asset(&reference)?
        .read_to_end(&mut bytes)?;
    ensure!(bytes == source_bytes, "export changed source asset bytes");
    let mut fresh = HostRuntime::start(&clean)?;
    fresh.activate_world(&clean, c.id)?;
    fresh.activate_world(&clean, d.id)?;
    let extra_message = ItemId::new();
    let extra_entry = ItemId::new();
    command(
        &mut fresh,
        c.id,
        "rintawa.chat.append@1",
        json!({"conversationId":conversation,"messageId":extra_message,"entryId":extra_entry,"text":"Only C"}),
    )?;
    let extra_character = ItemId::new();
    command(
        &mut fresh,
        c.id,
        "rintawa.character.import@1",
        json!({"itemId":extra_character,"card":card_data(&reference)}),
    )?;
    fresh.deactivate_world(c.id)?;
    fresh.deactivate_world(d.id)?;
    let c_data = storage(&clean, c.id)?;
    let d_data = storage(&clean, d.id)?;
    ensure!(
        c_data
            .load_property(message, &"rintawa.chat.message@1".parse()?)?
            .is_some(),
        "fork lost original Chat data"
    );
    ensure!(
        d_data
            .load_property(message, &"rintawa.chat.message@1".parse()?)?
            .is_some(),
        "second fork lost original Chat data"
    );
    ensure!(
        c_data.load_item(entry)?.is_some() && d_data.load_item(entry)?.is_some(),
        "fork lost original relation ID"
    );
    ensure!(
        c_data
            .load_property(character, &"rintawa.character.card@1".parse()?)?
            .is_some()
            && d_data
                .load_property(character, &"rintawa.character.card@1".parse()?)?
                .is_some(),
        "fork lost original Character data"
    );
    ensure!(
        c_data
            .load_property(extra_message, &"rintawa.chat.message@1".parse()?)?
            .is_some(),
        "fork C append failed"
    );
    ensure!(
        d_data.load_item(extra_message)?.is_none(),
        "fork D saw C append"
    );
    ensure!(
        c_data
            .load_property(extra_character, &"rintawa.character.card@1".parse()?)?
            .is_some()
            && d_data.load_item(extra_character)?.is_none(),
        "fork Character binding or World isolation failed"
    );
    drop(c_data);
    drop(d_data);

    // Repoint the same baseline subject/instance on disk while `fresh` still runs the original provider.
    let runtime_b = repack_runtime(&artifacts.join("web-runtime.rtw"))?;
    let digest_b = ArtifactDigest::sha256(&runtime_b);
    ensure!(
        digest_b != original_runtime,
        "provider variant did not change bytes"
    );
    let runtime_b_path = root.path().join("web-runtime-b.rtw");
    fs::write(&runtime_b_path, &runtime_b)?;
    let original_instance = clean.load_profile()?.activations[0].instance_id.clone();
    let repointed = clean.install_local_rtw(&runtime_b_path, Some(true))?;
    ensure!(
        repointed.activation.digest == digest_b,
        "baseline was not repointed"
    );
    ensure!(
        repointed.activation.instance_id == original_instance,
        "baseline repoint did not preserve the provider ComponentRef"
    );
    let template_b = repack_template(&artifacts.join("TextRoleplay.rtw"), &runtime_b)?;
    let template_b_path = root.path().join("template-b.rtw");
    fs::write(&template_b_path, template_b)?;
    let installed_b = clean.install_package(&template_b_path)?;
    let e = clean
        .create_world_from_template(&installed_b.digest, Some("E"), true)?
        .world;
    let failed = fresh.activate_world(&clean, e.id).unwrap_err();
    ensure!(
        matches!(&failed, HostError::InvalidTemplate(message) if message.contains("exact Host provider")),
        "wrong failure for repointed live provider: {failed:?}"
    );
    ensure!(!fresh.is_world_active(e.id), "rejected World became active");
    clean.select_stored_rtw(&original_runtime, Some(true))?;
    fresh.shutdown()?;
    Ok(())
}
