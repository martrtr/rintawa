//! A stopped World's explicit portable closure, packaged without approvals/secrets.
use crate::templates::publication::Stage;
use crate::templates::{
    Asset, Authoritative, Composition, Descriptor, Embedded, Grant, HostRequirement, Preferred,
    Ref, Slot, invalid, json, recipe,
};
use crate::world_runtime_scope_id;
use crate::{CompositionProfile, HostHome, HostResult, WORLD_COMPOSITION_FILE, WorldMetadata};
use rintawa_artifacts::{ArtifactPath, RtwArchive, RtwLimits};
use rintawa_sdk::world::WorldId;
use rintawa_sdk::{contracts::ComponentRef, types::ExtensionInstanceId};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};

fn slot_ref(
    owner: &ComponentRef,
    slots: &std::collections::HashMap<ExtensionInstanceId, String>,
) -> HostResult<Ref> {
    Ok(Ref {
        slot: slots
            .get(&owner.instance_id)
            .ok_or_else(|| invalid("composition references an absent World instance"))?
            .clone(),
        component_id: owner.component_id.to_string(),
    })
}
impl HostHome {
    /// Exports a stopped supported World as a self-contained template, without starting it.
    ///
    /// Active/busy writers, unsupported schemas, incomplete resources or unresolved
    /// Host providers fail without modifying the source or replacing an output file.
    pub fn export_world_template(
        &self,
        world: WorldId,
        destination: impl AsRef<Path>,
    ) -> HostResult<()> {
        let stage = Stage::new(self)?;
        let _lifecycle = self.lock_world_lifecycle(world)?;
        let directory = self.existing_world_directory(world)?;
        let _composition = CompositionProfile::lock(&directory.join(WORLD_COMPOSITION_FILE))?;
        let profile = self.load_world_composition_locked(world)?;
        profile.validate_scope(&world_runtime_scope_id(world))?;
        self.resolve_execution_requirements(&profile.execution_requirements)?;
        let storage = self.open_world_storage(world)?;
        let state = storage.load_session()?;
        let metadata = WorldMetadata::from_manifest(state.manifest())?;
        let layout = stage.path().join("package");
        std::fs::create_dir(&layout)?;
        storage.export_snapshot(layout.join("seed.sqlite"))?;
        let slots = profile
            .activations
            .iter()
            .enumerate()
            .map(|(index, activation)| {
                (activation.instance_id.clone(), format!("instance-{index}"))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut manifests = BTreeMap::new();
        let mut components = Vec::new();
        for activation in &profile.activations {
            let manifest = self.extension_manifest(&activation.artifact)?;
            let slot = slots[&activation.instance_id].clone();
            components.push(Slot {
                slot: slot.clone(),
                artifact: activation.artifact.clone(),
                enabled: activation.enabled,
                package_id: Some(manifest.id.to_string()),
                version: Some(manifest.version.clone()),
            });
            manifests.insert(slot, manifest);
        }
        let mut data = Descriptor {
            format: 1,
            name: metadata.title().to_owned(),
            description: metadata.description().unwrap_or_default().to_owned(),
            components,
            composition: Composition::default(),
            entry: None,
            host_requirements: profile
                .execution_requirements
                .iter()
                .map(|r| HostRequirement {
                    artifact: r.artifact.clone(),
                    component_id: r.component_id.to_string(),
                    target: r.target.clone(),
                })
                .collect(),
            seed: Some(ArtifactPath::parse("seed.sqlite")?),
            assets: Vec::new(),
            artifacts: Vec::new(),
        };
        for selected in &profile.preferred_providers {
            data.composition.preferred_providers.push(Preferred {
                contract_id: selected.contract_id.to_string(),
                contract_version: selected.contract_version.major(),
                provider: slot_ref(&selected.provider(), &slots)?,
            });
        }
        for selected in &profile.authoritative_providers {
            data.composition
                .authoritative_providers
                .push(Authoritative {
                    point: selected.point.clone(),
                    provider: slot_ref(&selected.provider(), &slots)?,
                });
        }
        for grant in &profile.runtime_permissions {
            let owner = slot_ref(&grant.owner(), &slots)?;
            data.composition.requested_grants.push(Grant {
                slot: owner.slot,
                component_id: owner.component_id,
                permission: grant.permission,
            });
        }
        if let Some(entry) = &profile.entry {
            data.entry = Some(slot_ref(
                &ComponentRef::new(entry.instance_id.clone(), entry.component_id.clone()),
                &slots,
            )?);
        }
        let _ = recipe(&data, &manifests)?; // Refuse unrecorded target providers, never infer baseline ownership.
        let references =
            self.portable_resources(&data, &manifests, Some(&layout.join("seed.sqlite")))?;
        std::fs::create_dir(layout.join("assets"))?;
        for reference in references.values() {
            let path = ArtifactPath::parse(format!("assets/{}", reference.digest.hex()))?;
            let destination = layout.join(path.as_str());
            if !destination.exists() {
                let mut source = self.asset_store.open_asset(reference)?;
                let mut output = File::create(destination)?;
                std::io::copy(&mut source, &mut output)?;
            }
            data.assets.push(Asset {
                reference: reference.clone(),
                path,
            });
        }
        std::fs::create_dir(layout.join("artifacts"))?;
        let required = data
            .components
            .iter()
            .map(|slot| slot.artifact.clone())
            .chain(data.host_requirements.iter().map(|r| r.artifact.clone()))
            .collect::<BTreeSet<_>>();
        for digest in required {
            let path = ArtifactPath::parse(format!("artifacts/{}.rtw", digest.hex()))?;
            self.store
                .copy_artifact(&digest, &mut File::create(layout.join(path.as_str()))?)?;
            data.artifacts.push(Embedded { digest, path });
        }
        data.validate().map_err(invalid)?;
        let descriptor =
            serde_json::to_vec_pretty(&data).map_err(|error| invalid(error.to_string()))?;
        // Keep output consumable by the same parser; fail before publishing an oversized descriptor.
        let _: Descriptor = json(&descriptor)?;
        std::fs::write(layout.join("template.json"), descriptor)?;
        std::fs::write(
            layout.join("rtw.toml"),
            "format = 1\ncontent = \"rintawa.world-template@1\"\nentry = \"template.json\"\n",
        )?;
        let packed = stage.path().join("validated-export.rtw");
        rintawa_artifacts::pack_directory(layout, &packed, RtwLimits::default())?;
        // Use the same closure/CRC/aggregate validation as a clean-home import.
        // A legacy generic CAS import must not produce an unimportable export.
        let mut archive = RtwArchive::open(&packed, RtwLimits::default())?;
        self.prepare(&mut archive, &stage)?;
        let destination = destination.as_ref();
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        std::io::copy(&mut File::open(packed)?, temporary.as_file_mut())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        crate::templates::publication::sync_directory(parent)?;
        Ok(())
    }
}
