//! Typed, bounded offline metadata for one installed World-template RTW.

use std::collections::{HashMap, HashSet};

use rintawa_artifacts::{ArtifactDigest, ArtifactPath, AssetRef};
use rintawa_extension_engine::validate_transport_point;
use rintawa_sdk::{runtime_permissions::RuntimePermission, world::SchemaKey};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Descriptor {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub components: Vec<Slot>,
    pub composition: Composition,
    pub entry: Option<Ref>,
    #[serde(default)]
    pub host_requirements: Vec<HostRequirement>,
    pub seed: Option<ArtifactPath>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub artifacts: Vec<Embedded>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Slot {
    pub slot: String,
    pub artifact: ArtifactDigest,
    pub enabled: bool,
    pub package_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Composition {
    #[serde(default)]
    pub preferred_providers: Vec<Preferred>,
    #[serde(default)]
    pub authoritative_providers: Vec<Authoritative>,
    #[serde(default)]
    pub requested_grants: Vec<Grant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Ref {
    pub slot: String,
    pub component_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Preferred {
    pub contract_id: String,
    pub contract_version: u32,
    pub provider: Ref,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Authoritative {
    pub point: String,
    pub provider: Ref,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Grant {
    pub slot: String,
    pub component_id: String,
    pub permission: RuntimePermission,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HostRequirement {
    pub artifact: ArtifactDigest,
    pub component_id: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Asset {
    pub reference: AssetRef,
    pub path: ArtifactPath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Embedded {
    pub digest: ArtifactDigest,
    pub path: ArtifactPath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Portability {
    pub format: u32,
    pub properties: Vec<PortableProperty>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PortableProperty {
    pub key: String,
    pub component_id: String,
    pub schema: Value,
    pub asset_refs: AssetEncoding,
    pub portable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum AssetEncoding {
    Native,
    BrowserV1,
}

/// Reject duplicate object keys even inside arbitrary JSON Schema documents.
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON value with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::from(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::from(value)))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .map(StrictValue)
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value.to_owned())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(value)))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = sequence.next_element::<StrictValue>()? {
                    items.push(item.0);
                }
                Ok(StrictValue(Value::Array(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut result = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, StrictValue>()? {
                    if result.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON field `{key}`")));
                    }
                    result.insert(key, value.0);
                }
                Ok(StrictValue(Value::Object(result)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

pub(super) fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err("template JSON exceeds 1 MiB".into());
    }
    let parsed: StrictValue = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    fn depth(value: &Value, level: usize) -> Result<(), String> {
        if level > MAX_DEPTH {
            return Err("template JSON exceeds nesting depth 32".into());
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    depth(value, level + 1)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    depth(value, level + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    depth(&parsed.0, 0)?;
    serde_json::from_value(parsed.0).map_err(|e| e.to_string())
}

impl Descriptor {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.format != 1 {
            return Err("unsupported template descriptor format".into());
        }
        if self.name.is_empty() || self.name.len() > 256 || self.description.len() > 4096 {
            return Err("invalid template name or description length".into());
        }
        if self.components.len() > 128 {
            return Err("too many template component slots".into());
        }
        if self.composition.authoritative_providers.len() > 64 {
            return Err("too many authoritative provider selections".into());
        }
        let mut slots = HashSet::new();
        for component in &self.components {
            if !valid_slot(&component.slot) || !slots.insert(component.slot.as_str()) {
                return Err(format!(
                    "invalid or duplicate template slot `{}`",
                    component.slot
                ));
            }
        }
        let check_ref = |reference: &Ref| -> Result<(), String> {
            if !slots.contains(reference.slot.as_str()) || reference.component_id.is_empty() {
                return Err(format!(
                    "unknown template component reference `{}`",
                    reference.slot
                ));
            }
            Ok(())
        };
        if let Some(entry) = &self.entry {
            check_ref(entry)?;
        }
        let mut preferred = HashSet::new();
        for selection in &self.composition.preferred_providers {
            check_ref(&selection.provider)?;
            if selection.contract_id.is_empty()
                || selection.contract_version == 0
                || !preferred.insert((&selection.contract_id, selection.contract_version))
            {
                return Err("invalid or duplicate preferred provider selection".into());
            }
        }
        let mut points = HashSet::new();
        for selection in &self.composition.authoritative_providers {
            check_ref(&selection.provider)?;
            if validate_transport_point(&selection.point).is_err()
                || !points.insert(selection.point.as_str())
            {
                return Err("duplicate authoritative point".into());
            }
        }
        let mut grants = HashSet::new();
        for grant in &self.composition.requested_grants {
            if !slots.contains(grant.slot.as_str())
                || grant.component_id.is_empty()
                || !grants.insert((&grant.slot, &grant.component_id, grant.permission))
            {
                return Err("invalid or duplicate requested grant".into());
            }
        }
        if self
            .seed
            .as_ref()
            .is_some_and(|p| p.as_str() != "seed.sqlite")
        {
            return Err("template seed must be seed.sqlite".into());
        }
        let mut paths: HashMap<&str, String> = HashMap::new();
        paths.insert("rtw.toml", "manifest".into());
        paths.insert("template.json", "descriptor".into());
        if self.seed.is_some() {
            paths.insert("seed.sqlite", "seed".into());
        }
        let mut asset_digests = HashMap::new();
        for asset in &self.assets {
            let digest = asset.reference.digest.to_string();
            if let Some(old) =
                asset_digests.insert(digest.clone(), (asset.path.as_str(), asset.reference.size))
                && old != (asset.path.as_str(), asset.reference.size)
            {
                return Err("one asset digest has conflicting path or size".into());
            }
            let kind = format!("asset:{digest}");
            if let Some(old) = paths.insert(asset.path.as_str(), kind.clone())
                && old != kind
            {
                return Err(format!("template entries alias at `{}`", asset.path));
            }
        }
        let mut embedded = HashSet::new();
        for artifact in &self.artifacts {
            if !embedded.insert(&artifact.digest) {
                return Err("duplicate embedded artifact digest".into());
            }
            if paths
                .insert(
                    artifact.path.as_str(),
                    format!("artifact:{}", artifact.digest),
                )
                .is_some()
            {
                return Err(format!("template entries alias at `{}`", artifact.path));
            }
        }
        let mut requirements = HashSet::new();
        for required in &self.host_requirements {
            if rintawa_sdk::manifest::validate_component_target(&required.target).is_err()
                || required.component_id.is_empty()
                || !requirements.insert((
                    &required.artifact,
                    &required.component_id,
                    &required.target,
                ))
            {
                return Err("invalid or duplicate Host requirement".into());
            }
        }
        Ok(())
    }
}

impl Portability {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.format != 1 {
            return Err("unsupported portability format".into());
        }
        let mut keys = HashSet::new();
        for property in &self.properties {
            let key: SchemaKey = property
                .key
                .parse::<SchemaKey>()
                .map_err(|e| e.to_string())?;
            if !keys.insert(key) || property.component_id.is_empty() || !property.portable {
                return Err("invalid or duplicate portable property".into());
            }
        }
        Ok(())
    }
}

fn valid_slot(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
