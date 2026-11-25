//! Strict template descriptor and portability metadata regressions.
use serde_json::{Value, json};

use super::{Descriptor, Portability, parse};

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn descriptor() -> Value {
    json!({
        "format": 1,
        "name": "Roleplay",
        "components": [{"slot":"chat","artifact":digest('a'),"enabled":true}],
        "composition": {
            "preferredProviders":[{"contractId":"rintawa.chat","contractVersion":1,"provider":{"slot":"chat","componentId":"backend"}}],
            "authoritativeProviders":[{"point":"rintawa/chat/v1","provider":{"slot":"chat","componentId":"backend"}}],
            "requestedGrants":[{"slot":"chat","componentId":"backend","permission":"background-task"}]
        },
        "entry":{"slot":"chat","componentId":"ui"},
        "assets":[{"reference":{"digest":digest('b'),"size":5,"media_type":"image/png"},"path":"assets/image.png"}],
        "artifacts":[{"digest":digest('a'),"path":"extensions/chat.rtw"}]
    })
}

fn check(value: &Value) -> Result<(), String> {
    parse::<Descriptor>(&serde_json::to_vec(value).unwrap())?.validate()
}

#[test]
fn accepts_exact_template_and_rejects_unknown_or_duplicate_fields() {
    check(&descriptor()).unwrap();
    let mut unknown = descriptor();
    unknown["composition"]["unexpected"] = json!(1);
    assert!(check(&unknown).is_err());
    let duplicate = br#"{"format":1,"name":"x","name":"y","components":[],"composition":{}}"#;
    assert!(parse::<Descriptor>(duplicate).is_err());
    let nested = br#"{"format":1,"name":"x","components":[],"composition":{"preferredProviders":[],"preferredProviders":[]}}"#;
    assert!(parse::<Descriptor>(nested).is_err());
    let schema = br#"{"format":1,"properties":[{"key":"story.card@1","componentId":"backend","schema":{"properties":{"name":{"type":"string","type":"number"}}},"assetRefs":"browser-v1","portable":true}]}"#;
    assert!(parse::<Portability>(schema).is_err());
}

#[test]
fn rejects_depth_and_byte_budget_before_typed_decode() {
    let deeply_nested = format!(
        "{{\"format\":1,\"properties\":[{{\"key\":\"story.card@1\",\"componentId\":\"backend\",\"schema\":{}0{},\"assetRefs\":\"native\",\"portable\":true}}]}}",
        "[".repeat(34),
        "]".repeat(34)
    );
    assert!(parse::<Portability>(deeply_nested.as_bytes()).is_err());
    assert!(parse::<Descriptor>(&vec![b' '; 1024 * 1024 + 1]).is_err());
}

#[test]
fn validates_slots_references_paths_and_unique_embedded_bytes() {
    let mut value = descriptor();
    value["components"][0]["slot"] = json!("../chat");
    assert!(check(&value).is_err());
    let mut value = descriptor();
    let duplicate = value["components"][0].clone();
    value["components"].as_array_mut().unwrap().push(duplicate);
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["entry"]["slot"] = json!("other");
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["seed"] = json!("../seed.sqlite");
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["seed"] = json!("seed.sqlite");
    value["assets"][0]["path"] = json!("seed.sqlite");
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["artifacts"][0]["path"] = json!("assets/image.png");
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["artifacts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"digest":digest('a'),"path":"extensions/duplicate.rtw"}));
    assert!(check(&value).is_err());
    let mut value = descriptor();
    value["assets"].as_array_mut().unwrap().push(json!({"reference":{"digest":digest('b'),"size":5,"media_type":"application/octet-stream"},"path":"assets/image.png"}));
    check(&value).unwrap(); // Distinct media metadata may share exact bytes.
    value["assets"][1]["reference"]["size"] = json!(7);
    assert!(check(&value).is_err());
}

#[test]
fn portability_requires_exact_schema_keys_and_opted_in_encoding() {
    let valid = json!({"format":1,"properties":[{"key":"story.card@1","componentId":"backend","schema":{"type":"object"},"assetRefs":"browser-v1","portable":true}]});
    parse::<Portability>(&serde_json::to_vec(&valid).unwrap())
        .unwrap()
        .validate()
        .unwrap();
    let mut invalid = valid.clone();
    invalid["properties"][0]["portable"] = json!(false);
    assert!(
        parse::<Portability>(&serde_json::to_vec(&invalid).unwrap())
            .unwrap()
            .validate()
            .is_err()
    );
    let mut invalid = valid.clone();
    invalid["properties"]
        .as_array_mut()
        .unwrap()
        .push(valid["properties"][0].clone());
    assert!(
        parse::<Portability>(&serde_json::to_vec(&invalid).unwrap())
            .unwrap()
            .validate()
            .is_err()
    );
    let mut invalid = valid;
    invalid["properties"][0]["assetRefs"] = json!("any");
    assert!(parse::<Portability>(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn portability_accepts_two_versions_of_one_property_from_the_same_owner() {
    let metadata = json!({"format":1,"properties":[
        {"key":"story.record@1","componentId":"backend","schema":{"type":"object"},"assetRefs":"native","portable":true},
        {"key":"story.record@2","componentId":"backend","schema":{"type":"object","properties":{"new":{"type":"string"}}},"assetRefs":"native","portable":true}
    ]});
    parse::<Portability>(&serde_json::to_vec(&metadata).unwrap())
        .unwrap()
        .validate()
        .unwrap();
}
