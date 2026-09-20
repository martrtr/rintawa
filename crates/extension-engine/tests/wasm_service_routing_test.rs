//! Cross-extension WASM service-routing integration coverage.

use anyhow::Result;
use rintawa_extension_engine::ExtensionEngine;
use rintawa_sdk::{
    contracts::{ComponentRef, ContractKey, ContractVersion},
    manifest::{
        ComponentDescriptor, ComponentKind, ComponentPermissions, ExtensionManifest,
        WASM_COMPONENT_TARGET_V1,
    },
    types::{ComponentId, ComponentTarget, ExtensionId},
};

const SERVICE_PROVIDER_COMPONENT: &[u8] = include_bytes!("fixtures/service-routing/provider.wasm");
const SERVICE_CONSUMER_COMPONENT: &[u8] = include_bytes!("fixtures/service-routing/consumer.wasm");

fn wasm_manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_owned(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: vec![ComponentDescriptor {
            id: ComponentId::new("runtime"),
            kind: ComponentKind::Runtime,
            target: ComponentTarget::new(WASM_COMPONENT_TARGET_V1),
            entry: None,
            required: true,
            permissions: ComponentPermissions {
                secret_read: Vec::new(),
                runtime: Vec::new(),
            },
        }],
    }
}

#[test]
fn test_should_route_service_from_wasm_consumer_to_wasm_provider() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let provider_id = ExtensionId::new("service-provider");
    let consumer_id = ExtensionId::new("service-consumer");

    let provider = engine
        .wasm_runtime_engine()?
        .load_component_from_bytes(ComponentId::new("runtime"), SERVICE_PROVIDER_COMPONENT)?;
    let consumer = engine
        .wasm_runtime_engine()?
        .load_component_from_bytes(ComponentId::new("runtime"), SERVICE_CONSUMER_COMPONENT)?;

    engine.register_extension(
        wasm_manifest(provider_id.as_str()),
        vec![Box::new(provider)],
    )?;
    engine.register_extension(
        wasm_manifest(consumer_id.as_str()),
        vec![Box::new(consumer)],
    )?;

    engine.start_extension(&provider_id)?;
    // The consumer fixture calls example.echo@1 from inside its WASM start callback
    // and traps if the route or response is wrong.
    engine.start_extension(&consumer_id)?;

    let response = engine.call_service(
        &ComponentRef::new(consumer_id.as_str(), "runtime"),
        &ContractKey::new("example.echo", ContractVersion::new(1)),
        b"ping",
    )?;
    assert_eq!(response, b"pong");

    Ok(())
}
