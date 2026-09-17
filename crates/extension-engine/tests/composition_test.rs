use rintawa_extension_engine::{
    ActivationPlanError, EngineError, ExtensionEngine, ExtensionState, UnresolvedContractReason,
};
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
fn test_registered_topology_is_visible_before_runtime_activation() -> anyhow::Result<()> {
    let contract = contract("example.bootstrap-topology");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
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

    let active = engine.composition_snapshot();
    assert!(active.bindings.is_empty());
    assert!(active.unresolved.is_empty());

    let topology = engine.composition_topology_snapshot();
    assert_eq!(topology.bindings.len(), 1);
    assert!(topology.unresolved.is_empty());
    assert_eq!(
        topology.bindings[0].providers,
        vec![ComponentRef::new("provider", "runtime")]
    );

    assert!(topology.bindings[0].required);
    let error = engine
        .start_extension(&ExtensionId::new("consumer"))
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::UndefinedContract,
            ..
        })
    ));
    assert_eq!(
        engine.extension_state(&ExtensionId::new("consumer")),
        Some(ExtensionState::Registered)
    );

    engine.start_extension(&ExtensionId::new("provider"))?;
    engine.start_extension(&ExtensionId::new("consumer"))?;
    let active = engine.composition_snapshot();
    assert_eq!(active.bindings.len(), 1);
    assert!(active.bindings[0].required);
    assert!(active.unresolved.is_empty());
    Ok(())
}

#[test]
fn test_should_order_required_provider_before_consumer_and_preserve_ties() -> anyhow::Result<()> {
    let contract = contract("example.activation-order");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(manifest("independent"), Vec::new())?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract)),
        )],
    )?;

    let requested = vec![
        ExtensionInstanceId::new("independent"),
        ExtensionInstanceId::new("consumer"),
        ExtensionInstanceId::new("provider"),
    ];
    let plan = engine.plan_extension_activation(&requested)?;
    assert_eq!(
        plan.ordered_instances(),
        &[
            ExtensionInstanceId::new("independent"),
            ExtensionInstanceId::new("provider"),
            ExtensionInstanceId::new("consumer"),
        ]
    );

    for instance_id in plan.ordered_instances() {
        engine.start_extension_instance(instance_id)?;
    }
    for instance_id in requested {
        assert_eq!(
            engine.extension_instance_state(&instance_id),
            Some(ExtensionState::Active)
        );
    }
    Ok(())
}

#[test]
fn test_should_order_separate_contract_definition_owner_before_consumer() -> anyhow::Result<()> {
    let contract = contract("example.separate-definition");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ContractComponent::new("runtime").providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.register_extension(
        manifest("definition"),
        vec![Box::new(ContractComponent::new("runtime").defining(
            ContractDefinition::new(contract, ContractResolutionPolicy::Single),
        ))],
    )?;

    let requested = vec![
        ExtensionInstanceId::new("provider"),
        ExtensionInstanceId::new("consumer"),
        ExtensionInstanceId::new("definition"),
    ];
    let plan = engine.plan_extension_activation(&requested)?;
    assert_eq!(
        plan.ordered_instances(),
        &[
            ExtensionInstanceId::new("provider"),
            ExtensionInstanceId::new("definition"),
            ExtensionInstanceId::new("consumer"),
        ]
    );
    for instance_id in plan.ordered_instances() {
        engine.start_extension_instance(instance_id)?;
    }
    assert_eq!(
        engine.extension_instance_state(&ExtensionInstanceId::new("consumer")),
        Some(ExtensionState::Active)
    );
    Ok(())
}

#[test]
fn test_should_reject_unresolved_required_consumer_activation() -> anyhow::Result<()> {
    let contract = contract("example.required-missing");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .consuming(ContractConsumer::new(contract, true)),
        )],
    )?;

    let error = engine
        .plan_extension_activation(&[ExtensionInstanceId::new("consumer")])
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::NoProvider,
            ..
        })
    ));
    assert!(matches!(
        engine
            .start_extension(&ExtensionId::new("consumer"))
            .unwrap_err(),
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::NoProvider,
            ..
        })
    ));
    Ok(())
}

#[test]
fn test_should_allow_unresolved_optional_consumer_activation() -> anyhow::Result<()> {
    let contract = contract("example.optional-missing");
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

    let consumer = ExtensionInstanceId::new("consumer");
    let plan = engine.plan_extension_activation(std::slice::from_ref(&consumer))?;
    assert_eq!(plan.ordered_instances(), std::slice::from_ref(&consumer));
    engine.start_extension_instance(&consumer)?;
    assert_eq!(
        engine.extension_instance_state(&consumer),
        Some(ExtensionState::Active)
    );
    Ok(())
}

#[test]
fn test_should_accept_already_active_required_provider() -> anyhow::Result<()> {
    let contract = contract("example.active-provider");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime").consuming(ContractConsumer::new(contract, true)),
        )],
    )?;

    let consumer = ExtensionInstanceId::new("consumer");
    assert!(matches!(
        engine
            .plan_extension_activation(std::slice::from_ref(&consumer))
            .unwrap_err(),
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::UndefinedContract,
            ..
        })
    ));

    engine.start_extension(&ExtensionId::new("provider"))?;
    let plan = engine.plan_extension_activation(std::slice::from_ref(&consumer))?;
    assert_eq!(plan.ordered_instances(), std::slice::from_ref(&consumer));
    engine.start_extension_instance(&consumer)?;
    Ok(())
}

#[test]
fn test_should_ignore_unscheduled_provider_when_planning_activation() -> anyhow::Result<()> {
    let contract = contract("example.provider-eligibility");
    let definition = ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("a-registered-provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition.clone())
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("z-active-provider"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone())),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ContractComponent::new("runtime").consuming(ContractConsumer::new(contract, true)),
        )],
    )?;

    engine.start_extension(&ExtensionId::new("z-active-provider"))?;
    let consumer = ExtensionInstanceId::new("consumer");
    let plan = engine.plan_extension_activation(std::slice::from_ref(&consumer))?;
    assert_eq!(plan.ordered_instances(), std::slice::from_ref(&consumer));
    engine.start_extension_instance(&consumer)?;
    Ok(())
}

#[test]
fn test_should_reject_duplicate_instance_in_activation_batch() -> anyhow::Result<()> {
    let mut engine = ExtensionEngine::new();
    engine.register_extension(manifest("duplicate"), Vec::new())?;
    let instance = ExtensionInstanceId::new("duplicate");

    assert!(matches!(
        engine
            .plan_extension_activation(&[instance.clone(), instance])
            .unwrap_err(),
        EngineError::ActivationPlan(ActivationPlanError::DuplicateInstance { .. })
    ));
    Ok(())
}

#[test]
fn test_should_reject_required_activation_dependency_cycle() -> anyhow::Result<()> {
    let first_contract = contract("example.cycle.first");
    let second_contract = contract("example.cycle.second");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("first"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(ContractDefinition::new(
                    first_contract.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(first_contract.clone()))
                .consuming(ContractConsumer::new(second_contract.clone(), true)),
        )],
    )?;
    engine.register_extension(
        manifest("second"),
        vec![Box::new(
            ContractComponent::new("runtime")
                .defining(ContractDefinition::new(
                    second_contract.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(second_contract))
                .consuming(ContractConsumer::new(first_contract, true)),
        )],
    )?;

    let error = engine
        .plan_extension_activation(&[
            ExtensionInstanceId::new("first"),
            ExtensionInstanceId::new("second"),
        ])
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "activation dependency cycle detected: first -> second -> first"
    );
    let EngineError::ActivationPlan(ActivationPlanError::DependencyCycle { instances }) = error
    else {
        anyhow::bail!("expected activation dependency cycle")
    };
    assert_eq!(instances.first(), instances.last());
    assert!(instances.contains(&ExtensionInstanceId::new("first")));
    assert!(instances.contains(&ExtensionInstanceId::new("second")));
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
            target: ComponentTarget::new("example.runtime.native@1"),
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
    let error = engine
        .start_extension(&ExtensionId::new("consumer"))
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::NoEligibleProvider,
            ..
        })
    ));

    engine.grant_requested_secret_read(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
        secret_pattern,
    )?;
    engine.start_extension(&ExtensionId::new("consumer"))?;
    let snapshot = engine.composition_snapshot();
    assert_eq!(snapshot.bindings.len(), 1);
    assert!(snapshot.unresolved.is_empty());

    engine
        .secret_manager()
        .revoke_component(&ComponentRef::new("secured-provider", "runtime"));
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
            target: ComponentTarget::new("example.runtime.native@1"),
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
    let error = engine
        .start_extension(&ExtensionId::new("consumer"))
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::ActivationPlan(ActivationPlanError::RequiredContractUnresolved {
            reason: UnresolvedContractReason::ConsumerIneligible,
            ..
        })
    ));

    engine.grant_requested_secret_read(
        &ExtensionId::new("consumer"),
        &ComponentId::new("runtime"),
        secret_pattern,
    )?;
    engine.start_extension(&ExtensionId::new("consumer"))?;
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

#[test]
fn test_should_resolve_platform_owned_binding_provider_with_persisted_policy_semantics()
-> anyhow::Result<()> {
    let scope = RuntimeScopeId::new("host");
    let contract = host_shell_contract_key();
    let mut engine = ExtensionEngine::new();
    engine.define_platform_binding_contract_in_scope(
        scope.clone(),
        contract.clone(),
        ContractResolutionPolicy::Single,
    )?;

    for instance in ["a-shell", "z-shell"] {
        engine.register_extension_instance(
            ExtensionInstanceId::new(instance),
            scope.clone(),
            manifest(instance),
            vec![Box::new(
                ContractComponent::new("shell").providing(ContractProvider::new(contract.clone())),
            )],
        )?;
        engine.start_extension_instance(&ExtensionInstanceId::new(instance))?;
    }

    assert_eq!(
        engine
            .resolve_active_contract_providers_in_scope(&scope, &contract)
            .map_err(|reason| anyhow::anyhow!(reason.to_string()))?,
        vec![ComponentRef::new("a-shell", "shell")]
    );

    let selected = ComponentRef::new("z-shell", "shell");
    engine.set_preferred_contract_provider_policy_in_scope(
        scope.clone(),
        contract.clone(),
        selected.clone(),
    );
    assert_eq!(
        engine
            .resolve_active_contract_providers_in_scope(&scope, &contract)
            .map_err(|reason| anyhow::anyhow!(reason.to_string()))?,
        vec![selected]
    );

    let selected_instance = ExtensionInstanceId::new("z-shell");
    engine.stop_extension_instance(&selected_instance)?;
    engine.unregister_extension_instance(&selected_instance)?;
    assert_eq!(
        engine.resolve_active_contract_providers_in_scope(&scope, &contract),
        Err(UnresolvedContractReason::PreferredProviderUnavailable)
    );
    Ok(())
}

#[test]
fn test_should_reject_extension_definition_of_platform_owned_contract() -> anyhow::Result<()> {
    let scope = RuntimeScopeId::new("host");
    let contract = host_shell_contract_key();
    let mut engine = ExtensionEngine::new();
    engine.define_platform_binding_contract_in_scope(
        scope.clone(),
        contract.clone(),
        ContractResolutionPolicy::Single,
    )?;

    let error = engine
        .register_extension_instance(
            ExtensionInstanceId::new("shell"),
            scope,
            manifest("shell"),
            vec![Box::new(ContractComponent::new("shell").defining(
                ContractDefinition::new(contract, ContractResolutionPolicy::Single),
            ))],
        )
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::PlatformContractDefinitionReserved { .. }
    ));
    Ok(())
}

#[test]
fn test_should_reject_platform_reservation_after_extension_definition() -> anyhow::Result<()> {
    let scope = RuntimeScopeId::new("host");
    let contract = host_shell_contract_key();
    let mut engine = ExtensionEngine::new();
    engine.register_extension_instance(
        ExtensionInstanceId::new("legacy-owner"),
        scope.clone(),
        manifest("legacy-owner"),
        vec![Box::new(ContractComponent::new("runtime").defining(
            ContractDefinition::new(contract.clone(), ContractResolutionPolicy::Single),
        ))],
    )?;

    let error = engine
        .define_platform_binding_contract_in_scope(
            scope,
            contract,
            ContractResolutionPolicy::Single,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::PlatformContractReservationConflict { .. }
    ));
    Ok(())
}
