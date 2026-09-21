use anyhow::Result;
use rintawa_extension_engine::ExtensionEngine;
use rintawa_sdk::prelude::*;
use rintawa_sdk::services::ServiceCallError;

#[derive(Clone)]
enum Handler {
    Static(Vec<u8>),
    Forward(ContractKey),
    ExpectCyclic(ContractKey),
}

struct ServiceComponent {
    id: ComponentId,
    definitions: Vec<ContractDefinition>,
    providers: Vec<ContractProvider>,
    consumers: Vec<ContractConsumer>,
    handler: Option<Handler>,
    message_limit: Option<usize>,
    start_call: Option<(ContractKey, Vec<u8>, Vec<u8>)>,
}

impl ServiceComponent {
    fn new(id: &str) -> Self {
        Self {
            id: ComponentId::new(id),
            definitions: Vec::new(),
            providers: Vec::new(),
            consumers: Vec::new(),
            handler: None,
            message_limit: None,
            start_call: None,
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

    fn responding(mut self, response: impl Into<Vec<u8>>) -> Self {
        self.handler = Some(Handler::Static(response.into()));
        self
    }

    fn forwarding(mut self, contract: ContractKey) -> Self {
        self.handler = Some(Handler::Forward(contract));
        self
    }

    fn expecting_cyclic(mut self, contract: ContractKey) -> Self {
        self.handler = Some(Handler::ExpectCyclic(contract));
        self
    }

    fn limiting_messages_to(mut self, bytes: usize) -> Self {
        self.message_limit = Some(bytes);
        self
    }

    fn calling_on_start(
        mut self,
        contract: ContractKey,
        request: impl Into<Vec<u8>>,
        expected: impl Into<Vec<u8>>,
    ) -> Self {
        self.start_call = Some((contract, request.into(), expected.into()));
        self
    }
}
impl Component for ServiceComponent {
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

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        if let Some((contract, request, expected)) = &self.start_call {
            let response = ctx
                .call_service(contract, request)
                .map_err(|error| ExtensionError::Message(error.to_string()))?;
            if &response != expected {
                return Err(ExtensionError::Message(String::from(
                    "unexpected startup service response",
                )));
            }
        }
        Ok(())
    }

    fn service_message_limit(&self) -> Option<usize> {
        self.message_limit
    }

    fn handle_service(
        &mut self,
        ctx: &mut dyn ComponentContext,
        _contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        match self.handler.as_ref() {
            Some(Handler::Static(response)) => Ok(response.clone()),
            Some(Handler::Forward(contract)) => ctx
                .call_service(contract, request)
                .map_err(|error| ExtensionError::Message(error.to_string())),
            Some(Handler::ExpectCyclic(contract)) => {
                let error = ctx.call_service(contract, request).unwrap_err();
                assert_eq!(error, ServiceCallError::CyclicCall);
                Ok(b"cycle-detected".to_vec())
            }
            None => Err(ExtensionError::ServiceHandlerUnavailable(String::from(
                "test",
            ))),
        }
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

fn start(engine: &mut ExtensionEngine, ids: &[&str]) -> Result<()> {
    for id in ids {
        engine.start_extension(&ExtensionId::new(*id))?;
    }
    Ok(())
}
#[test]
fn test_unary_service_route_tracks_provider_lifecycle() -> Result<()> {
    let contract = contract("example.echo");
    let definition =
        ContractDefinition::service(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"pong".to_vec()),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    start(&mut engine, &["provider", "consumer"])?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(engine.call_service(&consumer, &contract, b"ping")?, b"pong");

    engine.stop_extension(&ExtensionId::new("provider"))?;
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping"),
        Err(ServiceCallError::Unavailable)
    );

    engine.start_extension(&ExtensionId::new("provider"))?;
    assert_eq!(engine.call_service(&consumer, &contract, b"ping")?, b"pong");
    Ok(())
}

#[test]
fn test_platform_caller_only_invokes_platform_owned_service_contracts() -> Result<()> {
    let scope = RuntimeScopeId::new("world:platform");
    let platform_service = contract("rintawa.test.platform-system");
    let platform_binding = contract("rintawa.test.platform-binding");
    let extension_service = contract("example.extension-service");
    let provider_instance = ExtensionInstanceId::new("provider-instance");
    let mut engine = ExtensionEngine::new();

    engine.define_platform_service_contract_in_scope(
        scope.clone(),
        platform_service.clone(),
        ContractResolutionPolicy::Single,
    )?;
    engine.define_platform_binding_contract_in_scope(
        scope.clone(),
        platform_binding.clone(),
        ContractResolutionPolicy::Single,
    )?;
    engine.register_extension_instance(
        provider_instance.clone(),
        scope.clone(),
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    extension_service.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(platform_service.clone()))
                .providing(ContractProvider::new(platform_binding.clone()))
                .providing(ContractProvider::new(extension_service.clone()))
                .responding(b"platform-pong".to_vec()),
        )],
    )?;
    engine.start_extension_instance(&provider_instance)?;

    let caller = engine.platform_service_caller(scope.clone());
    assert_eq!(caller.scope_id(), &scope);
    assert_eq!(caller.call(&platform_service, b"ping")?, b"platform-pong");
    assert_eq!(
        caller.call(&platform_binding, b"ping"),
        Err(ServiceCallError::NotServiceContract)
    );
    assert_eq!(
        caller.call(&extension_service, b"ping"),
        Err(ServiceCallError::Unavailable)
    );

    engine.stop_extension_instance(&provider_instance)?;
    assert_eq!(
        caller.call(&platform_service, b"ping"),
        Err(ServiceCallError::Unavailable)
    );
    Ok(())
}

#[test]
fn test_platform_caller_can_pin_provider_extension_owner() -> Result<()> {
    let contract = contract("rintawa.test.owner-pinned-platform-service");
    let scope = RuntimeScopeId::new("world:owner-pinned");
    let hijacker_instance = ExtensionInstanceId::new("a-hijacker");
    let owner_instance = ExtensionInstanceId::new("z-owner");
    let mut engine = ExtensionEngine::new();

    engine.define_platform_service_contract_in_scope(
        scope.clone(),
        contract.clone(),
        ContractResolutionPolicy::Single,
    )?;
    engine.register_extension_instance(
        hijacker_instance.clone(),
        scope.clone(),
        manifest("hijacker"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"hijacked".to_vec()),
        )],
    )?;
    engine.register_extension_instance(
        owner_instance.clone(),
        scope.clone(),
        manifest("schema-owner"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"owner".to_vec()),
        )],
    )?;
    engine.start_extension_instance(&hijacker_instance)?;
    engine.start_extension_instance(&owner_instance)?;

    assert_eq!(
        engine
            .platform_service_caller(scope.clone())
            .call(&contract, b"ping")?,
        b"hijacked"
    );

    let caller = engine
        .platform_service_caller_for_extension(scope.clone(), ExtensionId::new("schema-owner"));
    assert_eq!(
        caller.provider_extension_id(),
        Some(&ExtensionId::new("schema-owner"))
    );
    assert_eq!(caller.call(&contract, b"ping")?, b"owner");

    let missing =
        engine.platform_service_caller_for_extension(scope, ExtensionId::new("missing-owner"));
    assert_eq!(
        missing.call(&contract, b"ping"),
        Err(ServiceCallError::Unavailable)
    );
    Ok(())
}

#[test]
fn test_platform_caller_isolates_identical_contracts_by_scope() -> Result<()> {
    let contract = contract("rintawa.test.scoped-platform-service");
    let scope_a = RuntimeScopeId::new("world:a");
    let scope_b = RuntimeScopeId::new("world:b");
    let instance_a = ExtensionInstanceId::new("provider-a");
    let instance_b = ExtensionInstanceId::new("provider-b");
    let mut engine = ExtensionEngine::new();

    for scope in [&scope_a, &scope_b] {
        engine.define_platform_service_contract_in_scope(
            scope.clone(),
            contract.clone(),
            ContractResolutionPolicy::Single,
        )?;
    }
    for (instance, scope, response) in [
        (&instance_a, &scope_a, b"a".to_vec()),
        (&instance_b, &scope_b, b"b".to_vec()),
    ] {
        engine.register_extension_instance(
            instance.clone(),
            scope.clone(),
            manifest(instance.as_str()),
            vec![Box::new(
                ServiceComponent::new("runtime")
                    .providing(ContractProvider::new(contract.clone()))
                    .responding(response),
            )],
        )?;
        engine.start_extension_instance(instance)?;
    }

    assert_eq!(
        engine
            .platform_service_caller(scope_a)
            .call(&contract, b"ping")?,
        b"a"
    );
    assert_eq!(
        engine
            .platform_service_caller(scope_b)
            .call(&contract, b"ping")?,
        b"b"
    );
    Ok(())
}

#[test]
fn test_bound_caller_uses_platform_service_contract_and_tracks_lifecycle() -> Result<()> {
    let scope = RuntimeScopeId::new("world:test");
    let contract = contract("rintawa.test.platform-service");
    let provider_instance = ExtensionInstanceId::new("provider-instance");
    let consumer_instance = ExtensionInstanceId::new("consumer-instance");
    let mut engine = ExtensionEngine::new();

    engine.define_platform_service_contract_in_scope(
        scope.clone(),
        contract.clone(),
        ContractResolutionPolicy::Single,
    )?;
    engine.register_extension_instance(
        provider_instance.clone(),
        scope.clone(),
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"pong".to_vec()),
        )],
    )?;
    engine.register_extension_instance(
        consumer_instance.clone(),
        scope,
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.start_extension_instance(&provider_instance)?;
    engine.start_extension_instance(&consumer_instance)?;

    let caller =
        engine.bound_service_caller(ComponentRef::new(consumer_instance.clone(), "runtime"));
    assert_eq!(
        caller.consumer(),
        &ComponentRef::new(consumer_instance, "runtime")
    );
    let threaded_caller = caller.clone();
    let threaded_contract = contract.clone();
    let threaded_response =
        std::thread::spawn(move || threaded_caller.call(&threaded_contract, b"ping"))
            .join()
            .expect("bound service caller thread must not panic")?;
    assert_eq!(threaded_response, b"pong");

    let undeclared =
        engine.bound_service_caller(ComponentRef::new(provider_instance.clone(), "runtime"));
    assert_eq!(
        undeclared.call(&contract, b"ping"),
        Err(ServiceCallError::NotConsumer)
    );

    engine.stop_extension_instance(&provider_instance)?;
    assert_eq!(
        caller.call(&contract, b"ping"),
        Err(ServiceCallError::Unavailable)
    );
    Ok(())
}

#[test]
fn test_preferred_provider_invalidates_cached_route() -> Result<()> {
    let contract = contract("example.choice");
    let definition =
        ContractDefinition::service(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    for (id, response) in [("a-provider", b"a".to_vec()), ("z-provider", b"z".to_vec())] {
        engine.register_extension(
            manifest(id),
            vec![Box::new(
                ServiceComponent::new("runtime")
                    .defining(definition.clone())
                    .providing(ContractProvider::new(contract.clone()))
                    .responding(response),
            )],
        )?;
    }
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    start(&mut engine, &["a-provider", "z-provider", "consumer"])?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(engine.call_service(&consumer, &contract, b"")?, b"a");

    engine.set_preferred_contract_provider(
        contract.clone(),
        ComponentRef::new("z-provider", "runtime"),
    );
    assert_eq!(engine.call_service(&consumer, &contract, b"")?, b"z");

    engine.unregister_extension(&ExtensionId::new("z-provider"))?;
    assert_eq!(
        engine.call_service(&consumer, &contract, b""),
        Err(ServiceCallError::Unavailable)
    );

    engine.register_extension(
        manifest("z-provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"z".to_vec()),
        )],
    )?;
    engine.start_extension(&ExtensionId::new("z-provider"))?;
    assert_eq!(engine.call_service(&consumer, &contract, b"")?, b"z");

    engine.clear_preferred_contract_provider(&contract);
    assert_eq!(engine.call_service(&consumer, &contract, b"")?, b"a");
    Ok(())
}

#[test]
fn test_secret_policy_revision_invalidates_cached_route() -> Result<()> {
    let contract = contract("example.secured-service");
    let pattern = SecretPathPattern::parse("service.keys.example")?;
    let definition =
        ContractDefinition::service(contract.clone(), ContractResolutionPolicy::Single);
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
                secret_read: vec![pattern.clone()],
                runtime: Vec::new(),
            },
        }],
    };
    let required_grant = ContractGrantRequirement::SecretRead {
        pattern: pattern.clone(),
    };
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        provider_manifest,
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone()).requiring(required_grant))
                .responding(b"secret-ready".to_vec()),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.start_extension(&ExtensionId::new("secured-provider"))?;
    engine.grant_requested_secret_read(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
        pattern,
    )?;
    engine.start_extension(&ExtensionId::new("consumer"))?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping")?,
        b"secret-ready"
    );
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping")?,
        b"secret-ready"
    );
    engine
        .secret_manager()
        .revoke_component(&ComponentRef::new("secured-provider", "runtime"));
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping"),
        Err(ServiceCallError::Unavailable)
    );
    Ok(())
}

#[test]
fn test_provider_message_limit_is_reported_by_router() -> Result<()> {
    let request_contract = contract("example.small-request");
    let response_contract = contract("example.small-response");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("request-provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    request_contract.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(request_contract.clone()))
                .responding(b"ok".to_vec())
                .limiting_messages_to(4),
        )],
    )?;
    engine.register_extension(
        manifest("response-provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    response_contract.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(response_contract.clone()))
                .responding(b"12345".to_vec())
                .limiting_messages_to(4),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(request_contract.clone(), true))
                .consuming(ContractConsumer::new(response_contract.clone(), true)),
        )],
    )?;
    start(
        &mut engine,
        &["request-provider", "response-provider", "consumer"],
    )?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &request_contract, b"12345"),
        Err(ServiceCallError::RequestTooLarge)
    );
    assert_eq!(
        engine.call_service(&consumer, &response_contract, b"ok"),
        Err(ServiceCallError::ResponseTooLarge)
    );
    Ok(())
}

#[test]
fn test_nested_service_call_routes_through_provider_context() -> Result<()> {
    let inner = contract("example.inner");
    let outer = contract("example.outer");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("inner-provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    inner.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(inner.clone()))
                .responding(b"inner-response".to_vec()),
        )],
    )?;
    engine.register_extension(
        manifest("outer-provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    outer.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(outer.clone()))
                .consuming(ContractConsumer::new(inner.clone(), true))
                .forwarding(inner.clone()),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime").consuming(ContractConsumer::new(outer.clone(), true)),
        )],
    )?;
    start(
        &mut engine,
        &["inner-provider", "outer-provider", "consumer"],
    )?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &outer, b"request")?,
        b"inner-response"
    );
    Ok(())
}

#[test]
fn test_start_callback_can_call_active_service_without_activating_external_caller() -> Result<()> {
    let contract = contract("example.startup");
    let definition =
        ContractDefinition::service(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"ready".to_vec()),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true))
                .calling_on_start(contract.clone(), b"ping".to_vec(), b"ready".to_vec()),
        )],
    )?;

    engine.start_extension(&ExtensionId::new("provider"))?;
    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping"),
        Err(ServiceCallError::NotConsumer)
    );

    engine.start_extension(&ExtensionId::new("consumer"))?;
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping")?,
        b"ready"
    );
    Ok(())
}

#[test]
fn test_binding_contract_cannot_be_called_as_service() -> Result<()> {
    let contract = contract("example.binding-only");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::new(
                    contract.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"should-not-run".to_vec()),
        )],
    )?;
    engine.register_extension(
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    start(&mut engine, &["provider", "consumer"])?;
    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping"),
        Err(ServiceCallError::NotServiceContract)
    );
    Ok(())
}

#[test]
fn test_nested_service_cycle_is_detected_before_provider_locking() -> Result<()> {
    let a = contract("example.cycle.a");
    let b = contract("example.cycle.b");
    let mut engine = ExtensionEngine::new();

    engine.register_extension(
        manifest("a"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    a.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(a.clone()))
                .consuming(ContractConsumer::new(b.clone(), false))
                .forwarding(b.clone()),
        )],
    )?;
    engine.register_extension(
        manifest("b"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(ContractDefinition::service(
                    b.clone(),
                    ContractResolutionPolicy::Single,
                ))
                .providing(ContractProvider::new(b.clone()))
                .consuming(ContractConsumer::new(a.clone(), false))
                .expecting_cyclic(a.clone()),
        )],
    )?;
    engine.register_extension(
        manifest("root"),
        vec![Box::new(
            ServiceComponent::new("runtime").consuming(ContractConsumer::new(a.clone(), true)),
        )],
    )?;
    start(&mut engine, &["a", "b", "root"])?;

    let caller = ComponentRef::new("root", "runtime");
    assert_eq!(
        engine.call_service(&caller, &a, b"ping")?,
        b"cycle-detected"
    );
    Ok(())
}

#[test]
fn test_service_routes_are_isolated_by_runtime_scope() -> Result<()> {
    let contract = contract("example.scoped-service");
    let definition =
        ContractDefinition::service(contract.clone(), ContractResolutionPolicy::Single);
    let mut engine = ExtensionEngine::new();

    let provider_a = ExtensionInstanceId::new("provider@world-a");
    let provider_b = ExtensionInstanceId::new("provider@world-b");
    let consumer_a = ExtensionInstanceId::new("consumer@world-a");
    let consumer_b = ExtensionInstanceId::new("consumer@world-b");
    let scope_a = RuntimeScopeId::new("world-a");
    let scope_b = RuntimeScopeId::new("world-b");

    engine.register_extension_instance(
        provider_a.clone(),
        scope_a.clone(),
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition.clone())
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"world-a".to_vec()),
        )],
    )?;
    engine.register_extension_instance(
        provider_b.clone(),
        scope_b.clone(),
        manifest("provider"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .defining(definition)
                .providing(ContractProvider::new(contract.clone()))
                .responding(b"world-b".to_vec()),
        )],
    )?;
    engine.register_extension_instance(
        consumer_a.clone(),
        scope_a,
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;
    engine.register_extension_instance(
        consumer_b.clone(),
        scope_b,
        manifest("consumer"),
        vec![Box::new(
            ServiceComponent::new("runtime")
                .consuming(ContractConsumer::new(contract.clone(), true)),
        )],
    )?;

    for instance in [&provider_a, &provider_b, &consumer_a, &consumer_b] {
        engine.start_extension_instance(instance)?;
    }

    assert_eq!(
        engine.call_service(
            &ComponentRef::new(consumer_a.clone(), "runtime"),
            &contract,
            b"ping",
        )?,
        b"world-a"
    );
    assert_eq!(
        engine.call_service(
            &ComponentRef::new(consumer_b.clone(), "runtime"),
            &contract,
            b"ping",
        )?,
        b"world-b"
    );

    engine.stop_extension_instance(&provider_a)?;
    assert_eq!(
        engine.call_service(
            &ComponentRef::new(consumer_a, "runtime"),
            &contract,
            b"ping",
        ),
        Err(ServiceCallError::Unavailable)
    );
    assert_eq!(
        engine.call_service(
            &ComponentRef::new(consumer_b, "runtime"),
            &contract,
            b"ping",
        )?,
        b"world-b"
    );

    Ok(())
}
