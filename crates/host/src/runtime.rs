use rintawa_extension_engine::{ExtensionEngine, RtwExtensionLoader};

use crate::{HostError, HostHome, HostResult};

/// Running baseline host composition loaded exclusively from exact local RTW digests.
pub struct HostRuntime {
    engine: ExtensionEngine,
    started_instances: Vec<rintawa_sdk::types::ExtensionInstanceId>,
}

impl HostRuntime {
    /// Loads and starts every enabled activation in the baseline profile.
    pub fn start(home: &HostHome) -> HostResult<Self> {
        let profile = home.load_profile()?;
        let mut engine = ExtensionEngine::new();
        let loader = RtwExtensionLoader::new();
        let mut registered = Vec::new();
        let mut started = Vec::new();

        let result: HostResult<()> = (|| {
            let activations: Vec<_> = profile
                .activations
                .into_iter()
                .filter(|item| item.enabled)
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

            let requested_instances: Vec<_> = activations
                .iter()
                .map(|activation| activation.instance_id.clone())
                .collect();
            let plan = engine.plan_extension_activation(&requested_instances)?;
            for instance_id in plan.into_ordered_instances() {
                engine.start_extension_instance(&instance_id)?;
                started.push(instance_id);
            }
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
        })
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
