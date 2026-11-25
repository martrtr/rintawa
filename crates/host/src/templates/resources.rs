//! Exact declared schema ownership and AssetRef closure.
use crate::templates::{
    AssetEncoding, Descriptor, JSON_LIMIT, PORTABILITY, Portability, Ref, asset_key, component,
    invalid, json,
};
use crate::{HostHome, HostResult};
use rintawa_artifacts::{ArtifactPath, AssetRef};
use rintawa_sdk::manifest::ExtensionManifest;
use rintawa_storage::SqliteWorldStorage;
use std::{collections::BTreeMap, path::Path};
impl HostHome {
    pub(super) fn portable_resources(
        &self,
        data: &Descriptor,
        manifests: &BTreeMap<String, ExtensionManifest>,
        seed: Option<&Path>,
    ) -> HostResult<BTreeMap<(String, u64, String), AssetRef>> {
        let mut declarations = BTreeMap::new();
        let mut names = BTreeMap::new();
        for slot in &data.components {
            let mut archive = self.store.open_artifact(&slot.artifact)?;
            let path = ArtifactPath::parse(PORTABILITY)?;
            if !archive.contains(&path) {
                continue;
            }
            let metadata: Portability = json(&archive.read_with_limit(&path, JSON_LIMIT)?)?;
            metadata.validate().map_err(invalid)?;
            for declaration in metadata.properties {
                let _ = component(
                    &Ref {
                        slot: slot.slot.clone(),
                        component_id: declaration.component_id.clone(),
                    },
                    manifests,
                )?;
                let key: rintawa_sdk::world::SchemaKey = declaration.key.parse().map_err(
                    |error: rintawa_sdk::world::SchemaKeyParseError| invalid(error.to_string()),
                )?;
                let owner = (&slot.slot, &declaration.component_id);
                if let Some(previous) =
                    names.insert(key.id().to_string(), (owner.0.clone(), owner.1.clone()))
                    && previous != (owner.0.clone(), owner.1.clone())
                {
                    return Err(invalid("duplicate portable property-name owners"));
                }
                if declarations.insert(key, declaration).is_some() {
                    return Err(invalid("duplicate portable schema declarations"));
                }
            }
        }
        let mut references = BTreeMap::new();
        let Some(seed) = seed else {
            return Ok(references);
        };
        let state = SqliteWorldStorage::validate_seed(seed)?;
        let storage = SqliteWorldStorage::open(seed)?;
        let snapshot = storage.snapshot()?;
        collect(
            state.manifest().as_value(),
            AssetEncoding::Native,
            &mut references,
            0,
        )?;
        for definition in state.properties().iter() {
            let declaration = declarations.get(definition.key()).ok_or_else(|| {
                invalid(format!("unsupported portable schema {}", definition.key()))
            })?;
            if !declaration.portable || declaration.schema != *definition.schema() {
                return Err(invalid(format!(
                    "portable schema mismatch {}",
                    definition.key()
                )));
            }
            let mut after = None;
            loop {
                let rows = snapshot.find_properties(definition.key(), after, 1)?;
                let Some(row) = rows.first() else {
                    break;
                };
                collect(row.data(), declaration.asset_refs, &mut references, 0)?;
                after = Some(row.item());
            }
        }
        Ok(references)
    }
}
fn collect(
    value: &serde_json::Value,
    encoding: AssetEncoding,
    output: &mut BTreeMap<(String, u64, String), AssetRef>,
    depth: usize,
) -> HostResult<()> {
    if depth > 128 {
        return Err(invalid(
            "portable state exceeds the existing JSON nesting bound",
        ));
    }
    match value {
        serde_json::Value::Object(fields) => {
            let media = if encoding == AssetEncoding::Native {
                "media_type"
            } else {
                "mediaType"
            };
            if fields.len() == 3
                && fields.contains_key("digest")
                && fields.contains_key("size")
                && fields.contains_key(media)
            {
                let reference: AssetRef = if encoding == AssetEncoding::Native {
                    serde_json::from_value(value.clone())
                        .map_err(|error| invalid(error.to_string()))?
                } else {
                    let size = fields["size"]
                        .as_str()
                        .ok_or_else(|| invalid("browser AssetRef size must be decimal text"))?;
                    let parsed: u64 = size.parse().map_err(|_| invalid("invalid AssetRef size"))?;
                    if parsed.to_string() != size {
                        return Err(invalid("noncanonical AssetRef size"));
                    }
                    AssetRef::new(
                        fields["digest"]
                            .as_str()
                            .ok_or_else(|| invalid("invalid AssetRef digest"))?
                            .parse()?,
                        parsed,
                        fields[media]
                            .as_str()
                            .ok_or_else(|| invalid("invalid AssetRef media type"))?,
                    )?
                };
                output.insert(asset_key(&reference), reference);
            } else {
                for value in fields.values() {
                    collect(value, encoding, output, depth + 1)?;
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect(value, encoding, output, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
