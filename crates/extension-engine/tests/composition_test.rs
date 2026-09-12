use rintawa_extension_engine::{EngineError, ExtensionEngine, UnresolvedContractReason};
use rintawa_sdk::prelude::*;

struct ContractComponent {
    id: ComponentId,
    definitions: Vec<ContractDefinition>,
    providers: Vec<ContractProvider>,
    consumers: Vec<ContractConsumer>,
}

impl ContractComponent {
    fn new(id: &str) -> Self {
        Self {
            id: ComponentId::new(id),
            definitions: Vec::new(),
            providers: Vec::new(),
            consumers: Vec::new(),
        }
    }

    fn defining(mut self, definition: ContractDefinition) -> Self {
        self.definitions.push(definition);
        self
    }

    fn providing(mut self, provider: ContractProvider) -> Self {
        self.providers.push(provider);
        self
    }

    fn consuming(mut self, consumer: ContractConsumer) -> Self {
        self.consumers.push(consumer);
        self
    }
}

impl Component for ContractComponent {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        for definition in &self.definitions {
            ctx.define_contract(definition.clone())?;
        }
        for provider in &self.providers {
            ctx.provide_contract(provider.clone())?;
        }
        for consumer in &self.consumers {
            ctx.consume_contract(consumer.clone())?;
        }
        Ok(())
    }
}

fn contract(name: &str) -> ContractKey {
    ContractKey::new(name, ContractVersion::new(1))
}

fn manifest(id: &str) -> ExtensionManifest {
    ExtensionManifest {
        id: ExtensionId::new(id),
        name: id.to_string(),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: Vec::new(),
    }
}

#[test]
fn test_single_provider_resolution_is_deterministic_and_overridable() -> anyhow::Result<()> {
    let contract = contract("example.service");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("z-provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition.clone())
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("a-provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;

    for extension in ["z-provider", "a-provider", "consumer"] {
        engine.start_extension(&ExtensionId::new(extension))?;
    }

    let snapshot = engine.composition_snapshot();
    assert_eq!(snapshot.bindings.len(), 1);
    assert_eq!(snapshot.bindings[0].providers.len(), 1);
    assert_eq!(
        snapshot.bindings[0].providers[0],
        ComponentRef::new("a-provider", "runtime")
    );

    engine.set_preferred_contract_provider(
        contract.clone(),
        ComponentRef::new("z-provider", "runtime"),
    );
    let snapshot = engine.composition_snapshot();
    assert_eq!(
        snapshot.bindings[0].providers[0],
        ComponentRef::new("z-provider", "runtime")
    );

    engine.stop_extension(&ExtensionId::new("z-provider"))?;
    let snapshot = engine.composition_snapshot();
    assert!(snapshot.bindings.is_empty());
    assert_eq!(snapshot.unresolved.len(), 1);
    assert_eq!(
        snapshot.unresolved[0].reason,
        UnresolvedContractReason::PreferredProviderUnavailable
    );
    Ok(())
}

#[test]
fn test_multiple_provider_resolution_binds_all_providers_in_stable_order() -> anyhow::Result<()> {
    let contract = contract("example.multiple");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Multiple);
    let mut engine = ExtensionEngine::new();

    for extension in ["z-provider", "a-provider"] {
        engine.register_extension(
            manifest(extension),
            vec![Box::new(
                ContractComponent::new("runtime")
                    .defining(definition.clone())
                    .providing(ContractProvider::new(contract.clone())),
            )],
        )?;
        engine.start_extension(&ExtensionId::new(extension))?;
    }

    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime").consuming(ContractConsumer::new(contract, true)),
        )],
    )?;
    engine.start_extension(&ExtensionId::new("consumer"))?;

    let snapshot = engine.composition_snapshot();
    assert_eq!(snapshot.bindings.len(), 1);
    assert_eq!(
        snapshot.bindings[0].providers,
        vec![
            ComponentRef::new("a-provider", "runtime"),
            ComponentRef::new("z-provider", "runtime"),
        ]
    );
    Ok(())
}

#[test]
fn test_provider_grants_control_binding_eligibility() -> anyhow::Result<()> {
    let contract = contract("example.secured");
    let secret_pattern = SecretPathPattern::parse("service.keys.example")?;
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    let provider_manifest = ExtensionManifest {
        id: ExtensionId::new("secured-provider"),
        name: String::from("Secured Provider"),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: vec![ComponentDescriptor {
            id: ComponentId::new("runtime"),
            kind: ComponentKind::Runtime,
            target: ComponentTarget::new("native"),
            entry: None,
            required: true,
            permissions: ComponentPermissions {
                secret_read: vec![secret_pattern.clone()],
            },
        }],
    };

    let required_grant = ContractGrantRequirement::SecretRead {
        pattern: secret_pattern.clone(),
    };
    engine.register_extension(
        provider_manifest,
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition.clone())
                .providing(ContractProvider::new(contract.clone()).requiring(required_grant)),
        )],
    )?;

    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.start_extension(&ExtensionId::new("secured-provider"))?;
    engine.start_extension(&ExtensionId::new("consumer"))?;

    let snapshot = engine.composition_snapshot();
    assert!(snapshot.bindings.is_empty());
    assert_eq!(snapshot.unresolved.len(), 1);
    assert_eq!(
        snapshot.unresolved[0].reason,
        UnresolvedContractReason::NoEligibleProvider
    );

    engine.grant_requested_secret_read(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
        secret_pattern,
    )?;
    let snapshot = engine.composition_snapshot();
    assert_eq!(snapshot.bindings.len(), 1);
    assert!(snapshot.unresolved.is_empty());

    engine.secret_manager().revoke_component(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
    );
    let snapshot = engine.composition_snapshot();
    assert!(snapshot.bindings.is_empty());
    assert_eq!(
        snapshot.unresolved[0].reason,
        UnresolvedContractReason::NoEligibleProvider
    );
    Ok(())
}

#[test]
fn test_conflicting_contract_definitions_are_rejected() -> anyhow::Result<()> {
    let contract = contract("example.conflict");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("first"),
        vec![Box::new(ContractComponent::new("runtime").defining(
            ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single),
        ))],
    )?;

    let error = engine
        .register_extension(
            manifest("second"),
            vec![Box::new(ContractComponent::new("runtime").defining(
                ContractDefinition::new(contract, ContractResolutionPolicy::Multiple),
            ))],
        )
        .unwrap_err();

    assert!(matches!(
        error,
        EngineError::ContractDefinitionConflict { .. }
    ));
    Ok(())
}

#[test]
fn test_consumer_grant_controls_binding_eligibility() -> anyhow::Result<()> {
    let contract = contract("example.secured-consumer");
    let secret_pattern = SecretPathPattern::parse("consumer.keys.example")?;
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition.clone())
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;

    let consumer_manifest = ExtensionManifest {
        id: ExtensionId::new("consumer"),
        name: String::from("Consumer"),
        version: String::from("0.0.1"),
        sdk: String::from("^0.0"),
        components: vec![ComponentDescriptor {
            id: ComponentId::new("runtime"),
            kind: ComponentKind::Runtime,
            target: ComponentTarget::new("native"),
            entry: None,
            required: true,
            permissions: ComponentPermissions {
                secret_read: vec![secret_pattern.clone()],
            },
        }],
    };
    let requirement = ContractGrantRequirement::SecretRead {
        pattern: secret_pattern.clone(),
    };
    engine.register_extension(
        consumer_manifest,
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .consuming(ContractConsumer::new(contract.clone(), true).requiring(requirement)),
        )],
    )?;

    engine.start_extension(&ExtensionId::new("provider"))?;
    engine.start_extension(&ExtensionId::new("consumer"))?;
    let snapshot = engine.composition_snapshot();
    assert_eq!(
        snapshot.unresolved[0].reason,
        UnresolvedContractReason::ConsumerIneligible
    );

    engine.grant_requested_secret_read(
        &ExtensionId::new("consumer"),
        &ComponentId::new("runtime"),
        secret_pattern,
    )?;
    let snapshot = engine.composition_snapshot();
    assert_eq!(snapshot.bindings.len(), 1);
    assert!(snapshot.unresolved.is_empty());
    Ok(())
}

#[test]
fn test_unresolved_consumer_preserves_required_flag() -> anyhow::Result<()> {
    let contract = contract("example.missing-provider");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .consuming(ContractConsumer::new(contract, false)),
        )],
    )?;
    engine.start_extension(&ExtensionId::new("consumer"))?;

    let snapshot = engine.composition_snapshot();
    assert!(snapshot.bindings.is_empty());
    assert_eq!(snapshot.unresolved.len(), 1);
    assert!(!snapshot.unresolved[0].required);
    assert_eq!(
        snapshot.unresolved[0].reason,
        UnresolvedContractReason::NoProvider
    );
    Ok(())
}

#[test]
fn test_conflicting_contract_protocols_are_rejected() -> anyhow::Result<()> {
    let contract = contract("example.protocol-conflict");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("binding-definition"),
        vec![Box::new(ContractComponent::new("runtime").defining(
            ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single),
        ))],
    )?;

    let error = engine
        .register_extension(
            manifest("service-definition"),
            vec![Box::new(ContractComponent::new("runtime").defining(
                ContractDefinition::service(contract, ContractResolutionPolicy::Single),
            ))],
        )
        .unwrap_err();

    assert!(matches!(
        error,
        EngineError::ContractDefinitionConflict { .. }
    ));
    Ok(())
}
