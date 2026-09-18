use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rintawa_extension_engine::{
    ActivationPlanError, ArtifactStoreAccess, CompositionAccess, CompositionActivation,
    EngineError, ExtensionEngine, HostAccessError, HostAccessResult, ImportedArtifact,
    PreferenceAccess, RtwExtensionLoadOutcome, RtwExtensionLoader, UnresolvedContractReason,
};
use rintawa_sdk::{
    contracts::{
        ComponentRef, ContractResolutionPolicy, host_shell_contract_key, ui_layer_contract_key,
    },
    types::{ExtensionInstanceId, RuntimeScopeId},
};

use crate::{
    BootstrapBlockedActivation, BootstrapDeferredActivation, BootstrapRollback, BootstrapStall,
    HOST_SCOPE, HostCleanupFailure, HostCleanupOperation, HostError, HostHome, HostResult,
    HostShutdownFailures,
};

struct BaselineHostAccess {
    home_root: PathBuf,
}

impl BaselineHostAccess {
    fn new(home_root: &Path) -> Self {
        Self {
            home_root: home_root.to_path_buf(),
        }
    }

    fn home(&self) -> HostAccessResult<HostHome> {
        HostHome::open(&self.home_root).map_err(map_host_access_error)
    }
}

impl ArtifactStoreAccess for BaselineHostAccess {
    fn import_rtw(&self, bytes: &[u8]) -> HostAccessResult<ImportedArtifact> {
        let home = self.home()?;
        let imported = home
            .import_rtw_bytes(bytes)
            .map_err(map_artifact_import_error)?;
        let archive = home
            .artifact_store()
            .open_artifact(imported.digest())
            .map_err(|_| HostAccessError::InvalidArtifact)?;
        Ok(ImportedArtifact {
            digest: imported.digest().to_string(),
            content: archive.manifest().content.to_string(),
        })
    }
}

impl PreferenceAccess for BaselineHostAccess {
    fn get(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<Option<String>> {
        self.home()?
            .get_preference(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                key,
            )
            .map_err(map_host_access_error)
    }

    fn set(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
        value: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .set_preference(
                RuntimeScopeId::new(scope_id),
                ComponentRef::new(instance_id, component_id),
                key.to_string(),
                value.to_string(),
            )
            .map_err(map_host_access_error)
    }

    fn delete(
        &self,
        scope_id: &str,
        instance_id: &str,
        component_id: &str,
        key: &str,
    ) -> HostAccessResult<()> {
        self.home()?
            .delete_preference(
                &RuntimeScopeId::new(scope_id),
                &ComponentRef::new(instance_id, component_id),
                key,
            )
            .map_err(map_host_access_error)
    }
}

impl CompositionAccess for BaselineHostAccess {
    fn list_activations(&self) -> HostAccessResult<Vec<CompositionActivation>> {
        self.home()?
            .list_activations()
            .map(|activations| {
                activations
                    .into_iter()
                    .map(to_composition_activation)
                    .collect()
            })
            .map_err(map_host_access_error)
    }

    fn select_artifact(
        &self,
        digest: &str,
        enabled: Option<bool>,
    ) -> HostAccessResult<CompositionActivation> {
        let digest: rintawa_artifacts::ArtifactDigest =
            digest.parse().map_err(|_| HostAccessError::InvalidDigest)?;
        self.home()?
            .select_stored_rtw(&digest, enabled)
            .map(to_composition_activation)
            .map_err(map_host_access_error)
    }

    fn set_enabled(&self, subject: &str, enabled: bool) -> HostAccessResult<()> {
        self.home()?
            .set_enabled(subject, enabled)
            .map_err(map_host_access_error)
    }

    fn remove_activation(&self, subject: &str) -> HostAccessResult<()> {
        self.home()?
            .remove_activation(subject)
            .map_err(map_host_access_error)
    }
}

fn to_composition_activation(activation: crate::InstalledActivation) -> CompositionActivation {
    CompositionActivation {
        subject: activation.subject,
        content: activation.content.to_string(),
        name: activation.name,
        version: activation.version,
        digest: activation.digest.to_string(),
        instance_id: activation.instance_id.to_string(),
        scope_id: activation.scope_id.to_string(),
        enabled: activation.enabled,
    }
}

fn map_artifact_import_error(error: HostError) -> HostAccessError {
    match error {
        HostError::Artifact(_) | HostError::UnsupportedContent(_) => {
            HostAccessError::InvalidArtifact
        }
        HostError::Io(_)
        | HostError::ProfileDecode(_)
        | HostError::ProfileEncode(_)
        | HostError::UnsupportedProfileSchema(_) => HostAccessError::Unavailable,
        _ => HostAccessError::Rejected,
    }
}

fn map_host_access_error(error: HostError) -> HostAccessError {
    match error {
        HostError::ActivationNotFound(_) => HostAccessError::NotFound,
        HostError::UnsupportedContent(_) => HostAccessError::UnsupportedContent,
        HostError::Io(_)
        | HostError::ProfileDecode(_)
        | HostError::ProfileEncode(_)
        | HostError::UnsupportedProfileSchema(_) => HostAccessError::Unavailable,
        HostError::InvalidPreference(_) => HostAccessError::InvalidPreference,
        HostError::PreferenceQuotaExceeded => HostAccessError::PreferenceQuotaExceeded,
        HostError::Artifact(_) => HostAccessError::Rejected,
        _ => HostAccessError::Rejected,
    }
}

/// Running baseline host composition loaded exclusively from exact local RTW digests.
pub struct HostRuntime {
    engine: ExtensionEngine,
    started_instances: Vec<ExtensionInstanceId>,
    host_shell_provider: Option<ComponentRef>,
    ui_layer_provider: Option<ComponentRef>,
}

impl HostRuntime {
    /// Loads and starts every enabled activation in the baseline profile.
    ///
    /// Bootstrap is staged to a fixed point. Artifacts whose required execution
    /// targets do not exist yet are deferred without side effects. Before the full
    /// topology is available, only target-provider-capable root WASM activations
    /// and their resolvable required contract dependency closure may start. Once
    /// every artifact is registered, the remaining instances use the ordinary
    /// deterministic full-topology activation plan.
    pub fn start(home: &HostHome) -> HostResult<Self> {
        let profile = home.load_profile()?;
        let access = Arc::new(BaselineHostAccess::new(home.root()));
        let artifact_store_access: Arc<dyn ArtifactStoreAccess> = access.clone();
        let composition_access: Arc<dyn CompositionAccess> = access.clone();
        let preference_access: Arc<dyn PreferenceAccess> = access;
        let mut engine = ExtensionEngine::with_host_access(
            artifact_store_access,
            composition_access,
            preference_access,
        );
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        let host_shell_contract = host_shell_contract_key();
        let ui_layer_contract = ui_layer_contract_key();
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            host_shell_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            ui_layer_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;

        let loader = RtwExtensionLoader::new();
        let mut registered = Vec::new();
        let mut started = Vec::new();
        let mut host_shell_provider = None;
        let mut ui_layer_provider = None;

        let result: HostResult<()> = (|| {
            let activations: Vec<_> = profile
                .activations
                .iter()
                .filter(|item| item.enabled)
                .cloned()
                .collect();
            for activation in &activations {
                if activation.content.to_string() != "rintawa.extension@1" {
                    return Err(HostError::UnsupportedContent(
                        activation.content.to_string(),
                    ));
                }
            }

            // Provider policy must already be visible while early bootstrap
            // instances are evaluated. A preferred provider may itself still be
            // deferred; in that case required consumers remain blocked until it loads.
            for selection in &profile.preferred_providers {
                engine.set_preferred_contract_provider_policy_in_scope(
                    selection.scope_id.clone(),
                    selection.contract(),
                    selection.provider(),
                );
            }

            let mut pending = activations.clone();
            let mut registered_set = HashSet::new();
            let mut started_set = HashSet::new();
            let mut bootstrap_instances = HashSet::new();

            while !pending.is_empty() {
                let mut made_progress = false;
                let mut next_pending = Vec::new();
                let mut deferred = Vec::new();

                for activation in pending {
                    match loader.try_load_stored_extension(
                        &mut engine,
                        home.artifact_store(),
                        &activation.artifact,
                        activation.instance_id.clone(),
                        activation.scope_id.clone(),
                    )? {
                        RtwExtensionLoadOutcome::Loaded(loaded) => {
                            // Registration is already a completed lifecycle transition.
                            // Record it before applying any fallible host policy so startup
                            // rollback always unregisters this instance explicitly.
                            registered_set.insert(activation.instance_id.clone());
                            registered.push(activation.instance_id.clone());
                            if loaded.can_publish_execution_targets {
                                bootstrap_instances.insert(activation.instance_id.clone());
                            }
                            for grant in profile.runtime_permissions.iter().filter(|grant| {
                                grant.scope_id == activation.scope_id
                                    && grant.instance_id == activation.instance_id
                            }) {
                                engine.grant_requested_runtime_permission_for_instance(
                                    &activation.instance_id,
                                    &grant.component_id,
                                    grant.permission,
                                )?;
                            }
                            made_progress = true;
                        }
                        RtwExtensionLoadOutcome::Deferred(waiting) => {
                            deferred.push(BootstrapDeferredActivation {
                                subject: activation.subject.clone(),
                                instance_id: activation.instance_id.clone(),
                                extension_id: waiting.extension_id.to_string(),
                                missing_required_targets: waiting
                                    .missing_required_targets()
                                    .cloned()
                                    .collect(),
                            });
                            next_pending.push(activation);
                        }
                    }
                }
                pending = next_pending;

                if pending.is_empty() {
                    break;
                }

                // Only runtime-provider-capable roots and their required contract
                // dependency closure may start before the complete baseline is loaded.
                // Ordinary consumers wait for the final full-topology activation plan.
                let mut blocked = Vec::new();
                loop {
                    blocked.clear();
                    let mut started_one = false;
                    let mut expanded_dependencies = false;
                    for activation in &activations {
                        if !bootstrap_instances.contains(&activation.instance_id)
                            || !registered_set.contains(&activation.instance_id)
                            || started_set.contains(&activation.instance_id)
                        {
                            continue;
                        }
                        match engine.start_extension_instance(&activation.instance_id) {
                            Ok(()) => {
                                started_set.insert(activation.instance_id.clone());
                                started.push(activation.instance_id.clone());
                                made_progress = true;
                                started_one = true;
                                break;
                            }
                            Err(EngineError::ActivationPlan(reason)) => {
                                expanded_dependencies |= expand_bootstrap_dependencies(
                                    &engine,
                                    &activation.scope_id,
                                    &activation.instance_id,
                                    &reason,
                                    &registered_set,
                                    &mut bootstrap_instances,
                                );
                                blocked.push(BootstrapBlockedActivation {
                                    instance_id: activation.instance_id.clone(),
                                    reason,
                                });
                            }
                            Err(error) => return Err(HostError::Engine(error)),
                        }
                    }
                    if started_one {
                        continue;
                    }
                    if expanded_dependencies {
                        continue;
                    }
                    break;
                }

                if made_progress {
                    continue;
                }

                let blocked_instances: Vec<_> = activations
                    .iter()
                    .filter(|activation| {
                        bootstrap_instances.contains(&activation.instance_id)
                            && registered_set.contains(&activation.instance_id)
                            && !started_set.contains(&activation.instance_id)
                    })
                    .map(|activation| activation.instance_id.clone())
                    .collect();
                let batch_error = bootstrap_batch_error(&engine, &blocked_instances)?;
                return Err(HostError::BootstrapStalled(Box::new(BootstrapStall {
                    deferred,
                    blocked,
                    batch_error,
                })));
            }

            // Once every enabled artifact is registered, resolve the complete
            // contract topology and start every remaining instance using the
            // ordinary deterministic provider-before-consumer activation plan.
            let remaining_instances: Vec<_> = activations
                .iter()
                .filter(|activation| {
                    registered_set.contains(&activation.instance_id)
                        && !started_set.contains(&activation.instance_id)
                })
                .map(|activation| activation.instance_id.clone())
                .collect();
            let plan = engine.plan_extension_activation(&remaining_instances)?;
            for instance_id in plan.into_ordered_instances() {
                engine.start_extension_instance(&instance_id)?;
                started_set.insert(instance_id.clone());
                started.push(instance_id);
            }

            let has_explicit_shell_selection =
                profile.preferred_providers.iter().any(|selection| {
                    selection.scope_id == host_scope && selection.contract() == host_shell_contract
                });
            host_shell_provider = match engine
                .resolve_active_contract_providers_in_scope(&host_scope, &host_shell_contract)
            {
                Ok(providers) => providers.into_iter().next(),
                Err(UnresolvedContractReason::NoProvider) if !has_explicit_shell_selection => None,
                Err(reason) => {
                    return Err(HostError::ContractRoleUnavailable {
                        scope_id: host_scope.to_string(),
                        contract: host_shell_contract.to_string(),
                        reason,
                    });
                }
            };

            let has_explicit_ui_layer_selection =
                profile.preferred_providers.iter().any(|selection| {
                    selection.scope_id == host_scope && selection.contract() == ui_layer_contract
                });
            ui_layer_provider = match engine
                .resolve_active_contract_providers_in_scope(&host_scope, &ui_layer_contract)
            {
                Ok(providers) => providers.into_iter().next(),
                Err(UnresolvedContractReason::NoProvider) if !has_explicit_ui_layer_selection => {
                    None
                }
                Err(reason) => {
                    return Err(HostError::ContractRoleUnavailable {
                        scope_id: host_scope.to_string(),
                        contract: ui_layer_contract.to_string(),
                        reason,
                    });
                }
            };
            if let Some(provider) = ui_layer_provider.clone() {
                engine.attach_registered_ui_layer(provider)?;
            }
            Ok(())
        })();

        if let Err(error) = result {
            let cleanup_failures = cleanup_instances(&mut engine, &started, &registered);
            if cleanup_failures.is_empty() {
                return Err(error);
            }
            return Err(HostError::BootstrapRollback(Box::new(BootstrapRollback {
                primary: Box::new(error),
                cleanup_failures,
            })));
        }

        Ok(Self {
            engine,
            started_instances: started,
            host_shell_provider,
            ui_layer_provider,
        })
    }

    /// Returns the selected active provider of the platform Host Shell role.
    ///
    /// `None` is a valid headless composition with no eligible Host Shell provider.
    pub fn host_shell_provider(&self) -> Option<&ComponentRef> {
        self.host_shell_provider.as_ref()
    }

    /// Returns the selected active provider of the portable UI Layer role.
    ///
    /// `None` is valid for a headless composition or a shell that does not use
    /// the portable UI presentation protocol.
    pub fn ui_layer_provider(&self) -> Option<&ComponentRef> {
        self.ui_layer_provider.as_ref()
    }

    /// Executes one cooperative runtime pump for active baseline components.
    ///
    /// The returned duration is the earliest requested next wake-up. `None` means
    /// no active component currently owns scheduled cooperative work.
    pub fn poll_runtime(&mut self) -> HostResult<Option<Duration>> {
        Ok(self.engine.poll_runtime()?)
    }

    /// Stops and unregisters every baseline runtime instance in reverse activation order.
    pub fn shutdown(mut self) -> HostResult<()> {
        let cleanup_failures = cleanup_instances(
            &mut self.engine,
            &self.started_instances,
            &self.started_instances,
        );
        if cleanup_failures.is_empty() {
            Ok(())
        } else {
            Err(HostError::ShutdownFailed(Box::new(HostShutdownFailures {
                cleanup_failures,
            })))
        }
    }
}

fn cleanup_instances(
    engine: &mut ExtensionEngine,
    started_instances: &[ExtensionInstanceId],
    registered_instances: &[ExtensionInstanceId],
) -> Vec<HostCleanupFailure> {
    let started: HashSet<_> = started_instances.iter().cloned().collect();
    let mut failures = Vec::new();

    // Registered-but-never-started dependents can still hold execution-target
    // dependencies that prevent their provider from stopping. Remove those first.
    for instance_id in registered_instances
        .iter()
        .rev()
        .filter(|instance_id| !started.contains(*instance_id))
    {
        record_unregister_failure(engine, instance_id, &mut failures);
    }

    // Started instances are recorded in provider-before-consumer order. Reverse
    // that order and unregister each dependent immediately after stop so provider
    // lifetime guards never observe a dangling registered dependent.
    for instance_id in started_instances.iter().rev() {
        if let Err(error) = engine.stop_extension_instance(instance_id) {
            failures.push(HostCleanupFailure {
                instance_id: instance_id.clone(),
                operation: HostCleanupOperation::Stop,
                error,
            });
        }
        record_unregister_failure(engine, instance_id, &mut failures);
    }
    failures
}

fn record_unregister_failure(
    engine: &mut ExtensionEngine,
    instance_id: &ExtensionInstanceId,
    failures: &mut Vec<HostCleanupFailure>,
) {
    if let Err(error) = engine.unregister_extension_instance(instance_id) {
        failures.push(HostCleanupFailure {
            instance_id: instance_id.clone(),
            operation: HostCleanupOperation::Unregister,
            error,
        });
    }
}

fn expand_bootstrap_dependencies(
    engine: &ExtensionEngine,
    scope_id: &RuntimeScopeId,
    activation_instance_id: &ExtensionInstanceId,
    reason: &ActivationPlanError,
    registered_instances: &HashSet<ExtensionInstanceId>,
    bootstrap_instances: &mut HashSet<ExtensionInstanceId>,
) -> bool {
    let ActivationPlanError::RequiredContractUnresolved {
        instance_id,
        component_id,
        contract,
        ..
    } = reason
    else {
        return false;
    };
    if instance_id != activation_instance_id {
        return false;
    }

    let consumer = ComponentRef::new(instance_id.clone(), component_id.clone());
    let topology = engine.composition_topology_snapshot_for_scope(scope_id);
    let Some(binding) = topology.bindings.iter().find(|binding| {
        binding.required && binding.consumer == consumer && binding.contract == *contract
    }) else {
        return false;
    };

    let mut expanded = false;
    for provider in &binding.providers {
        if provider.instance_id != *instance_id
            && registered_instances.contains(&provider.instance_id)
        {
            expanded |= bootstrap_instances.insert(provider.instance_id.clone());
        }
    }
    if let Some(definition_owner) = engine.contract_definition_owner_in_scope(scope_id, contract)
        && definition_owner.instance_id != *instance_id
        && registered_instances.contains(&definition_owner.instance_id)
    {
        expanded |= bootstrap_instances.insert(definition_owner.instance_id);
    }
    expanded
}

fn bootstrap_batch_error(
    engine: &ExtensionEngine,
    blocked_instances: &[ExtensionInstanceId],
) -> HostResult<Option<ActivationPlanError>> {
    if blocked_instances.is_empty() {
        return Ok(None);
    }
    match engine.plan_extension_activation(blocked_instances) {
        Ok(_) => Ok(None),
        Err(EngineError::ActivationPlan(reason)) => Ok(Some(reason)),
        Err(error) => Err(HostError::Engine(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rintawa_sdk::{
        context::RegistrationContext,
        contracts::{
            ContractConsumer, ContractDefinition, ContractKey, ContractProvider, ContractVersion,
        },
        errors::{ExtensionError, ExtensionResult},
        manifest::ExtensionManifest,
        prelude::{Component, ComponentId, ContractResolutionPolicy, ExtensionId},
    };

    struct ContractComponent {
        id: ComponentId,
        definition: Option<ContractDefinition>,
        provider: Option<ContractProvider>,
        consumer: Option<ContractConsumer>,
    }

    impl ContractComponent {
        fn new(id: &str) -> Self {
            Self {
                id: ComponentId::new(id),
                definition: None,
                provider: None,
                consumer: None,
            }
        }
    }

    impl Component for ContractComponent {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            if let Some(definition) = &self.definition {
                ctx.define_contract(definition.clone())?;
            }
            if let Some(provider) = &self.provider {
                ctx.provide_contract(provider.clone())?;
            }
            if let Some(consumer) = &self.consumer {
                ctx.consume_contract(consumer.clone())?;
            }
            Ok(())
        }
    }

    fn test_manifest(id: &str) -> ExtensionManifest {
        ExtensionManifest {
            id: ExtensionId::new(id),
            name: id.to_string(),
            version: String::from("0.0.1"),
            sdk: String::from("^0.0"),
            components: Vec::new(),
        }
    }

    #[test]
    fn test_should_expand_bootstrap_root_to_required_contract_dependencies() -> anyhow::Result<()> {
        let scope = RuntimeScopeId::new("host");
        let contract = ContractKey::new("example.bootstrap-service", ContractVersion::new(1));
        let root = ExtensionInstanceId::new("runtime-provider-root");
        let provider = ExtensionInstanceId::new("service-provider");
        let definition = ExtensionInstanceId::new("service-definition");
        let unrelated = ExtensionInstanceId::new("unrelated");
        let mut engine = ExtensionEngine::new();

        let mut root_component = ContractComponent::new("runtime");
        root_component.consumer = Some(ContractConsumer::new(contract.clone(), true));
        engine.register_extension_instance(
            root.clone(),
            scope.clone(),
            test_manifest("runtime-provider-root"),
            vec![Box::new(root_component)],
        )?;

        let mut provider_component = ContractComponent::new("runtime");
        provider_component.provider = Some(ContractProvider::new(contract.clone()));
        engine.register_extension_instance(
            provider.clone(),
            scope.clone(),
            test_manifest("service-provider"),
            vec![Box::new(provider_component)],
        )?;

        let mut definition_component = ContractComponent::new("runtime");
        definition_component.definition = Some(ContractDefinition::new(
            contract,
            ContractResolutionPolicy::Single,
        ));
        engine.register_extension_instance(
            definition.clone(),
            scope.clone(),
            test_manifest("service-definition"),
            vec![Box::new(definition_component)],
        )?;
        engine.register_extension_instance(
            unrelated.clone(),
            scope.clone(),
            test_manifest("unrelated"),
            Vec::new(),
        )?;

        let error = engine
            .plan_extension_activation(std::slice::from_ref(&root))
            .expect_err("root alone must not resolve its required contract");
        let EngineError::ActivationPlan(reason) = error else {
            anyhow::bail!("expected activation-plan error");
        };
        let registered = HashSet::from([
            root.clone(),
            provider.clone(),
            definition.clone(),
            unrelated.clone(),
        ]);
        let mut bootstrap = HashSet::from([root.clone()]);

        assert!(expand_bootstrap_dependencies(
            &engine,
            &scope,
            &root,
            &reason,
            &registered,
            &mut bootstrap,
        ));
        assert!(bootstrap.contains(&provider));
        assert!(bootstrap.contains(&definition));
        assert!(!bootstrap.contains(&unrelated));
        Ok(())
    }

    struct FailingStopComponent {
        id: ComponentId,
    }

    impl Component for FailingStopComponent {
        fn id(&self) -> &ComponentId {
            &self.id
        }

        fn stop(
            &mut self,
            _ctx: &mut dyn rintawa_sdk::context::ComponentContext,
        ) -> ExtensionResult<()> {
            Err(ExtensionError::Message(String::from("test stop failure")))
        }
    }

    #[test]
    fn test_should_aggregate_all_host_cleanup_failures() -> anyhow::Result<()> {
        let scope = RuntimeScopeId::new("host");
        let first = ExtensionInstanceId::new("first");
        let second = ExtensionInstanceId::new("second");
        let mut engine = ExtensionEngine::new();

        engine.register_extension_instance(
            first.clone(),
            scope.clone(),
            test_manifest("first"),
            vec![Box::new(FailingStopComponent {
                id: ComponentId::new("runtime"),
            })],
        )?;
        engine.register_extension_instance(
            second.clone(),
            scope,
            test_manifest("second"),
            vec![Box::new(FailingStopComponent {
                id: ComponentId::new("runtime"),
            })],
        )?;
        engine.start_extension_instance(&first)?;
        engine.start_extension_instance(&second)?;

        let failures = cleanup_instances(
            &mut engine,
            &[first.clone(), second.clone()],
            &[first.clone(), second.clone()],
        );

        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].instance_id, second);
        assert_eq!(failures[0].operation, HostCleanupOperation::Stop);
        assert_eq!(failures[1].instance_id, first);
        assert_eq!(failures[1].operation, HostCleanupOperation::Stop);
        Ok(())
    }
}
