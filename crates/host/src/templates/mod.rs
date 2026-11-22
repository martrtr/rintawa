//! Offline immutable World templates, with no guest installer or implicit grants.
mod export;
mod format;
mod publication;
mod resources;

use crate::{
    ActivationRecord, AuthoritativeProviderSelection, CompositionEntry, CompositionProfile,
    DEFAULT_WORLD_SCOPE, HostError, HostHome, HostResult, PreferredProviderSelection,
    RuntimePermissionGrant, WorldMetadata, WorldSummary,
};
use format::*;
use publication::Stage;
use rintawa_artifacts::{
    ArtifactDigest, ArtifactPath, AssetRef, ContentType, RtwArchive, RtwLimits,
};
use rintawa_extension_engine::{ExtensionEngine, RtwExtensionLoader};
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey, ContractVersion},
    manifest::ExtensionManifest,
    types::{ComponentId, ExtensionInstanceId, RuntimeScopeId},
};
use rintawa_storage::SqliteWorldStorage;
use rintawa_world::WorldManifest;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

const WEB_TARGET: &str = "rintawa.runtime.web-bundle@2";
const CONTENT: &str = "rintawa.world-template@1";
const PORTABILITY: &str = "world-portability.json";
const JSON_LIMIT: u64 = 1024 * 1024;
fn invalid(reason: impl Into<String>) -> HostError {
    HostError::InvalidTemplate(reason.into())
}
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> HostResult<T> {
    format::parse(bytes).map_err(invalid)
}

/// An installed template is immutable package metadata, never a running World.
#[derive(Debug, Clone)]
pub struct TemplateSummary {
    /// Exact installed template artifact.
    pub digest: ArtifactDigest,
    /// Display name from its validated descriptor.
    pub name: String,
    /// Bounded description.
    pub description: String,
    /// A malformed generic CAS import is visible but cannot be selected as a template.
    pub invalid_reason: Option<String>,
}
/// A generic RTW installation changes no composition or permission policy.
#[derive(Debug, Clone)]
pub struct PackageInstallResult {
    /// Exact verified artifact identity.
    pub digest: ArtifactDigest,
    /// Root content type, interpreted only by its designated handler.
    pub content: ContentType,
}
/// A fully published independent World and its optional starting component.
#[derive(Debug, Clone)]
pub struct CreatedTemplateWorld {
    /// Published World identity and metadata.
    pub world: WorldSummary,
    /// Resolved fresh World component, without implicit activation or permissions.
    pub entry: Option<ComponentRef>,
}
struct Prepared {
    data: Descriptor,
    manifests: BTreeMap<String, ExtensionManifest>,
    seed: Option<std::path::PathBuf>,
}
fn descriptor(archive: &mut RtwArchive) -> HostResult<Descriptor> {
    if archive.manifest().content.to_string() != CONTENT
        || archive.manifest().entry.as_str() != "template.json"
    {
        return Err(invalid("expected rintawa.world-template@1 / template.json"));
    }
    let data: Descriptor =
        json(&archive.read_with_limit(&ArtifactPath::parse("template.json")?, JSON_LIMIT)?)?;
    data.validate().map_err(invalid)?;
    Ok(data)
}
impl HostHome {
    pub(crate) fn package_transfer_staging(&self) -> HostResult<(tempfile::TempDir, File)> {
        let stage = Stage::new(self)?;
        Ok((stage.directory, stage._ownership))
    }

    /// Validates/stores a local RTW without selecting or starting any activation.
    pub fn install_package(&self, source: impl AsRef<Path>) -> HostResult<PackageInstallResult> {
        let stage = Stage::new(self)?;
        let copy = stage.path().join("package.rtw");
        let mut input = File::open(source)?;
        if !input.metadata()?.is_file() {
            return Err(invalid("package source must be a regular file"));
        }
        let maximum = RtwLimits::default().max_archive_bytes;
        let mut output = File::create(&copy)?;
        if std::io::copy(&mut (&mut input).take(maximum + 1), &mut output)? > maximum {
            return Err(invalid("package exceeds RTW archive limit"));
        }
        output.sync_all()?;
        let mut archive = RtwArchive::open(&copy, RtwLimits::default())?;
        let content = archive.manifest().content.clone();
        // Validate all container bytes, including payloads not understood by Core.
        let paths: Vec<_> = archive.entries().map(|entry| entry.path.clone()).collect();
        for path in paths {
            archive.copy_to(&path, &mut std::io::sink(), u64::MAX)?;
        }
        if content.to_string() == CONTENT {
            self.prepare(&mut archive, &stage)?;
        }
        let imported = self.store.import(&copy)?;
        Ok(PackageInstallResult {
            digest: imported.digest().clone(),
            content,
        })
    }
    /// Lists validated installed templates without activating their packages.
    pub fn list_templates(&self) -> HostResult<Vec<TemplateSummary>> {
        let mut result = Vec::new();
        for digest in self.store.digests()? {
            let mut archive = self.store.open_artifact(&digest)?;
            if archive.manifest().content.to_string() == CONTENT {
                match descriptor(&mut archive) {
                    Ok(data) => result.push(TemplateSummary {
                        digest,
                        name: data.name,
                        description: data.description,
                        invalid_reason: None,
                    }),
                    Err(error) => {
                        let mut reason = error.to_string();
                        let boundary = reason
                            .char_indices()
                            .map(|(index, _)| index)
                            .take_while(|index| *index <= 2048)
                            .last()
                            .unwrap_or(0);
                        if reason.len() > 2048 {
                            reason.truncate(boundary);
                        }
                        result.push(TemplateSummary {
                            digest,
                            name: "Invalid template".into(),
                            description: String::new(),
                            invalid_reason: Some(reason),
                        });
                    }
                }
            }
        }
        Ok(result)
    }
    /// Returns validated template metadata, including exact requests for review.
    pub fn template_info(&self, digest: &ArtifactDigest) -> HostResult<String> {
        let mut archive = self.store.open_artifact(digest)?;
        let data = descriptor(&mut archive)?;
        serde_json::to_string_pretty(&data).map_err(|error| invalid(error.to_string()))
    }
    /// Creates a new independent World from an installed exact template revision.
    /// The approval flag covers only the listed World grants, never Host providers.
    pub fn create_world_from_template(
        &self,
        digest: &ArtifactDigest,
        title: Option<&str>,
        approve_runtime_permissions: bool,
    ) -> HostResult<CreatedTemplateWorld> {
        let stage = Stage::new(self)?;
        let mut archive = self.store.open_artifact(digest)?;
        let prepared = self.prepare(&mut archive, &stage)?;
        self.check_host_requirements(&prepared.data)?;
        if !approve_runtime_permissions && !prepared.data.composition.requested_grants.is_empty() {
            return Err(HostError::TemplateApprovalRequired(
                serde_json::to_string(&prepared.data.composition.requested_grants)
                    .map_err(|error| invalid(error.to_string()))?,
            ));
        }
        let recipe = recipe(&prepared.data, &prepared.manifests)?;
        let source_manifest = match &prepared.seed {
            Some(seed) => SqliteWorldStorage::validate_seed(seed)?.manifest().clone(),
            None => WorldManifest::default(),
        };
        let metadata = WorldMetadata::new(
            title.unwrap_or(&prepared.data.name),
            Some(prepared.data.description.clone()),
            None,
        )?;
        // Keep an exported cover unless the caller explicitly replaces it later.
        let metadata = if let Some(seed) = &prepared.seed {
            let old =
                WorldMetadata::from_manifest(SqliteWorldStorage::validate_seed(seed)?.manifest())?;
            WorldMetadata::new(
                metadata.title(),
                metadata.description().map(str::to_owned),
                old.cover().cloned(),
            )?
        } else {
            metadata
        };
        let manifest = metadata.write_to_manifest(&source_manifest)?;
        self.publish_world(&recipe, prepared.seed.as_deref(), &manifest)
    }

    fn extension_manifest(&self, digest: &ArtifactDigest) -> HostResult<ExtensionManifest> {
        Ok(RtwExtensionLoader::new().read_stored_manifest(
            &ExtensionEngine::new(),
            &self.store,
            digest,
        )?)
    }
    fn prepare(&self, archive: &mut RtwArchive, stage: &Stage) -> HostResult<Prepared> {
        let data = descriptor(archive)?;
        let mut expanded_bytes = archive
            .entries()
            .map(|entry| entry.uncompressed_size)
            .sum::<u64>();
        let mut expanded_entries = archive.entry_count();
        let required = data
            .components
            .iter()
            .map(|slot| slot.artifact.clone())
            .chain(
                data.host_requirements
                    .iter()
                    .map(|requirement| requirement.artifact.clone()),
            )
            .collect::<BTreeSet<_>>();
        let mut verified = BTreeSet::new();
        for (index, embedded) in data.artifacts.iter().enumerate() {
            if !required.contains(&embedded.digest) {
                return Err(invalid("unreferenced embedded artifact"));
            }
            let path = stage.path().join(format!("artifact-{index}.rtw"));
            archive.copy_to(&embedded.path, &mut File::create(&path)?, u64::MAX)?;
            let mut inner = RtwArchive::open(&path, RtwLimits::default())?;
            validate_dependency(&mut inner, &mut expanded_bytes, &mut expanded_entries)?;
            self.store.import_expected(path, &embedded.digest)?;
            verified.insert(embedded.digest.clone());
        }
        let mut missing = BTreeSet::new();
        for digest in required.difference(&verified) {
            match self.store.open_artifact(digest) {
                Ok(mut inner) => {
                    validate_dependency(&mut inner, &mut expanded_bytes, &mut expanded_entries)?
                }
                Err(rintawa_artifacts::RtwError::StoredArtifactNotFound(_)) => {
                    missing.insert(digest.to_string());
                }
                Err(error) => return Err(error.into()),
            }
        }
        if !missing.is_empty() {
            return Err(invalid(format!(
                "missing exact dependencies: {}",
                missing.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
        let mut manifests = BTreeMap::new();
        for slot in &data.components {
            manifests.insert(slot.slot.clone(), self.extension_manifest(&slot.artifact)?);
        }
        for requirement in &data.host_requirements {
            let manifest = self.extension_manifest(&requirement.artifact)?;
            if !manifest
                .components
                .iter()
                .any(|component| component.id.as_str() == requirement.component_id)
            {
                return Err(invalid("missing Host provider component"));
            }
        }
        let _ = recipe(&data, &manifests)?; // Validate references and requested capabilities before copying state.
        let seed = if let Some(seed) = &data.seed {
            let path = stage.path().join("seed.sqlite");
            archive.copy_to(seed, &mut File::create(&path)?, u64::MAX)?;
            Some(path)
        } else {
            None
        };
        let references = self.portable_resources(&data, &manifests, seed.as_deref())?;
        let declared: BTreeSet<_> = data
            .assets
            .iter()
            .map(|asset| asset_key(&asset.reference))
            .collect();
        if references.keys().cloned().collect::<BTreeSet<_>>() != declared {
            return Err(invalid(
                "embedded asset list differs from portable state closure",
            ));
        }
        for (index, asset) in data.assets.iter().enumerate() {
            let path = stage.path().join(format!("asset-{index}"));
            archive.copy_to(&asset.path, &mut File::create(&path)?, asset.reference.size)?;
            let imported = self
                .asset_store
                .import(&path, asset.reference.media_type.to_string())?;
            if imported.asset_ref() != &asset.reference {
                return Err(invalid("embedded asset digest/size mismatch"));
            }
        }
        Ok(Prepared {
            data,
            manifests,
            seed,
        })
    }
    fn check_host_requirements(&self, data: &Descriptor) -> HostResult<()> {
        let requirements = data
            .host_requirements
            .iter()
            .map(|r| crate::HostExecutionRequirement {
                artifact: r.artifact.clone(),
                component_id: ComponentId::new(&r.component_id),
                target: r.target.clone(),
            })
            .collect::<Vec<_>>();
        self.resolve_execution_requirements(&requirements)
            .map(|_| ())
    }
    pub(crate) fn resolve_execution_requirements(
        &self,
        requirements: &[crate::HostExecutionRequirement],
    ) -> HostResult<Vec<(String, ComponentRef)>> {
        let baseline = self.load_profile()?;
        let mut result = Vec::new();
        for requirement in requirements {
            let candidates = baseline
                .activations
                .iter()
                .filter(|activation| {
                    activation.enabled && activation.artifact == requirement.artifact
                })
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                return Err(invalid(format!(
                    "select one exact Host prerequisite {}",
                    requirement.artifact
                )));
            }
            let activation = candidates[0];
            let manifest = self.extension_manifest(&requirement.artifact)?;
            let component = manifest
                .components
                .iter()
                .find(|component| component.id == requirement.component_id)
                .ok_or_else(|| invalid("Host prerequisite component missing"))?;
            result.push((
                requirement.target.clone(),
                ComponentRef::new(activation.instance_id.clone(), component.id.clone()),
            ));
        }
        Ok(result)
    }
}
fn component<'a>(
    reference: &Ref,
    manifests: &'a BTreeMap<String, ExtensionManifest>,
) -> HostResult<&'a rintawa_sdk::manifest::ComponentDescriptor> {
    manifests
        .get(&reference.slot)
        .and_then(|manifest| {
            manifest
                .components
                .iter()
                .find(|component| component.id.as_str() == reference.component_id)
        })
        .ok_or_else(|| {
            invalid(format!(
                "missing component {}:{}",
                reference.slot, reference.component_id
            ))
        })
}
fn recipe(
    data: &Descriptor,
    manifests: &BTreeMap<String, ExtensionManifest>,
) -> HostResult<CompositionProfile> {
    let scope = RuntimeScopeId::new(DEFAULT_WORLD_SCOPE);
    let mut recipe = CompositionProfile {
        execution_requirements: data
            .host_requirements
            .iter()
            .map(|r| crate::HostExecutionRequirement {
                artifact: r.artifact.clone(),
                component_id: ComponentId::new(&r.component_id),
                target: r.target.clone(),
            })
            .collect(),
        ..CompositionProfile::default()
    };
    for slot in &data.components {
        let manifest = manifests
            .get(&slot.slot)
            .ok_or_else(|| invalid("unresolved slot"))?;
        recipe.activations.push(ActivationRecord {
            subject: manifest.id.to_string(),
            content: ContentType::parse("rintawa.extension@1")?,
            artifact: slot.artifact.clone(),
            instance_id: ExtensionInstanceId::new(&slot.slot),
            scope_id: scope.clone(),
            enabled: slot.enabled,
        });
        for descriptor in &manifest.components {
            if descriptor.required
                && descriptor.target.as_str() != rintawa_sdk::manifest::WASM_COMPONENT_TARGET_V1
                && !data
                    .host_requirements
                    .iter()
                    .any(|requirement| requirement.target == descriptor.target.as_str())
            {
                return Err(invalid(format!(
                    "missing explicit Host target prerequisite {}",
                    descriptor.target
                )));
            }
        }
    }
    for selected in &data.composition.preferred_providers {
        component(&selected.provider, manifests)?;
        let version = ContractVersion::new(selected.contract_version);
        recipe
            .preferred_providers
            .push(PreferredProviderSelection::new(
                scope.clone(),
                ContractKey::new(selected.contract_id.clone(), version),
                ComponentRef::new(
                    selected.provider.slot.as_str(),
                    selected.provider.component_id.as_str(),
                ),
            ));
    }
    for selected in &data.composition.authoritative_providers {
        component(&selected.provider, manifests)?;
        recipe
            .authoritative_providers
            .push(AuthoritativeProviderSelection::new(
                scope.clone(),
                selected.point.clone(),
                ComponentRef::new(
                    selected.provider.slot.as_str(),
                    selected.provider.component_id.as_str(),
                ),
            ));
    }
    for grant in &data.composition.requested_grants {
        let descriptor = component(
            &Ref {
                slot: grant.slot.clone(),
                component_id: grant.component_id.clone(),
            },
            manifests,
        )?;
        if !descriptor.permissions.runtime.contains(&grant.permission) {
            return Err(invalid(
                "template requests a capability absent from its exact manifest",
            ));
        }
        recipe.runtime_permissions.push(RuntimePermissionGrant::new(
            scope.clone(),
            ComponentRef::new(grant.slot.as_str(), grant.component_id.as_str()),
            grant.permission,
        ));
    }
    if let Some(entry) = &data.entry {
        let descriptor = component(entry, manifests)?;
        if descriptor.target.as_str() != WEB_TARGET {
            return Err(invalid("entry is not a web component"));
        }
        recipe.entry = Some(CompositionEntry {
            instance_id: ExtensionInstanceId::new(&entry.slot),
            component_id: ComponentId::new(&entry.component_id),
        });
    }
    recipe.validate_scope(&scope)?;
    Ok(recipe)
}
fn asset_key(reference: &AssetRef) -> (String, u64, String) {
    (
        reference.digest.to_string(),
        reference.size,
        reference.media_type.to_string(),
    )
}

#[cfg(test)]
mod tests;

fn validate_dependency(
    archive: &mut RtwArchive,
    expanded_bytes: &mut u64,
    expanded_entries: &mut usize,
) -> HostResult<()> {
    if archive.manifest().content.to_string() != "rintawa.extension@1" {
        return Err(invalid("dependency is not an extension RTW"));
    }
    *expanded_bytes = expanded_bytes.saturating_add(
        archive
            .entries()
            .map(|entry| entry.uncompressed_size)
            .sum::<u64>(),
    );
    *expanded_entries = expanded_entries.saturating_add(archive.entry_count());
    let limits = RtwLimits::default();
    if *expanded_bytes > limits.max_uncompressed_bytes || *expanded_entries > limits.max_entries {
        return Err(invalid(
            "template and dependencies exceed aggregate RTW expansion limits",
        ));
    }
    let paths = archive
        .entries()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    for path in paths {
        archive.copy_to(&path, &mut std::io::sink(), u64::MAX)?;
    }
    Ok(())
}
