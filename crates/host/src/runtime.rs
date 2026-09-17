use rintawa_extension_engine::{ExtensionEngine, RtwExtensionLoader, UnresolvedContractReason};

use rintawa_sdk::{
    contracts::{ComponentRef, ContractResolutionPolicy, host_shell_contract_key},
    types::RuntimeScopeId,
};

use crate::{HOST_SCOPE, HostError, HostHome, HostResult};

/// Running baseline host composition loaded exclusively from exact local RTW digests.
pub struct HostRuntime {
    engine: ExtensionEngine,
    started_instances: Vec<rintawa_sdk::types::ExtensionInstanceId>,
    host_shell_provider: Option<ComponentRef>,
}

impl HostRuntime {
    /// Loads and starts every enabled activation in the baseline profile.
    pub fn start(home: &HostHome) -> HostResult<Self> {
        let profile = home.load_profile()?;
        let mut engine = ExtensionEngine::new();
        let host_scope = RuntimeScopeId::new(HOST_SCOPE);
        let host_shell_contract = host_shell_contract_key();
        engine.define_platform_binding_contract_in_scope(
            host_scope.clone(),
            host_shell_contract.clone(),
            ContractResolutionPolicy::Single,
        )?;
        let loader = RtwExtensionLoader::new();
        let mut registered = Vec::new();
        let mut started = Vec::new();
        let mut host_shell_provider = None;

        let result: HostResult<()> = (|| {
            let activations: Vec<_> = profile
                .activations
                .iter()
                .filter(|item| item.enabled)
                .cloned()
                .collect();

            // Registration is a separate bootstrap phase. Every baseline
            // extension publishes its topology before any component enters
            // `start()`, so dependency planning can inspect the complete
            // baseline rather than profile iteration side effects.
            for activation in &activations {
                if activation.content.to_string() != "rintawa.extension@1" {
                    return Err(HostError::UnsupportedContent(
                        activation.content.to_string(),
                    ));
                }
                loader.load_stored_extension(
                    &mut engine,
                    home.artifact_store(),
                    &activation.artifact,
                    activation.instance_id.clone(),
                    activation.scope_id.clone(),
                )?;
                registered.push(activation.instance_id.clone());
            }

            for selection in &profile.preferred_providers {
                engine.set_preferred_contract_provider_policy_in_scope(
                    selection.scope_id.clone(),
                    selection.contract(),
                    selection.provider(),
                );
            }

            let requested_instances: Vec<_> = activations
                .iter()
                .map(|activation| activation.instance_id.clone())
                .collect();
            let plan = engine.plan_extension_activation(&requested_instances)?;
            for instance_id in plan.into_ordered_instances() {
                engine.start_extension_instance(&instance_id)?;
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
            Ok(())
        })();

        if let Err(error) = result {
            for instance in started.iter().rev() {
                let _ = engine.stop_extension_instance(instance);
            }
            for instance in registered.iter().rev() {
                let _ = engine.unregister_extension_instance(instance);
            }
            return Err(error);
        }

        Ok(Self {
            engine,
            started_instances: started,
            host_shell_provider,
        })
    }

    /// Returns the selected active provider of the platform Host Shell role.
    ///
    /// `None` is a valid headless composition with no eligible Host Shell provider.
    pub fn host_shell_provider(&self) -> Option<&ComponentRef> {
        self.host_shell_provider.as_ref()
    }

    /// Stops and unregisters every baseline runtime instance in reverse activation order.
    pub fn shutdown(mut self) -> HostResult<()> {
        let mut first_error = None;
        for instance in self.started_instances.iter().rev() {
            if let Err(error) = self.engine.stop_extension_instance(instance)
                && first_error.is_none()
            {
                first_error = Some(HostError::Engine(error));
            }
            if let Err(error) = self.engine.unregister_extension_instance(instance)
                && first_error.is_none()
            {
                first_error = Some(HostError::Engine(error));
            }
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}
