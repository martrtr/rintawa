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
            target: ComponentTarget::new("native"),
            entry: None,
            required: true,
            permissions: ComponentPermissions {
                secret_read: vec![pattern.clone()],
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
    start(&mut engine, &["secured-provider", "consumer"])?;

    let consumer = ComponentRef::new("consumer", "runtime");
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping"),
        Err(ServiceCallError::Unavailable)
    );

    engine.grant_requested_secret_read(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
        pattern,
    )?;
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping")?,
        b"secret-ready"
    );
    assert_eq!(
        engine.call_service(&consumer, &contract, b"ping")?,
        b"secret-ready"
    );
    engine.secret_manager().revoke_component(
        &ExtensionId::new("secured-provider"),
        &ComponentId::new("runtime"),
    );
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
                .consuming(ContractConsumer::new(b.clone(), true))
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
                .consuming(ContractConsumer::new(a.clone(), true))
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
