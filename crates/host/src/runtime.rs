use std::sync::Arc;

use rintawa_extension_engine::{ExtensionEngine, RtwExtensionLoader};
use rintawa_web_host::WebComponentHost;

use crate::{HostError, HostHome, HostResult};

/// Running baseline host composition loaded exclusively from exact local RTW digests.
pub struct HostRuntime {
    engine: ExtensionEngine,
    web_host: Arc<WebComponentHost>,
    started_instances: Vec<rintawa_sdk::types::ExtensionInstanceId>,
}

impl HostRuntime {
    /// Loads and starts every enabled activation in the baseline profile.
    pub fn start(home: &HostHome) -> HostResult<Self> {
        let profile = home.load_profile()?;
        let mut engine = ExtensionEngine::new();
        let web_host = Arc::new(WebComponentHost::new());
        let mut loader = RtwExtensionLoader::new();
        loader.register_component_host(web_host.clone())?;
        let mut registered = Vec::new();
        let mut started = Vec::new();

        let result: HostResult<()> = (|| {
            for activation in profile.activations.into_iter().filter(|item| item.enabled) {
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
                    activation.scope_id,
                )?;
                registered.push(activation.instance_id.clone());
                engine.start_extension_instance(&activation.instance_id)?;
                started.push(activation.instance_id);
            }
            web_host.attach_layers(&mut engine)?;
            web_host.pump(&mut engine)?;
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
            web_host,
            started_instances: started,
        })
    }

    /// Processes queued Web renderer actions and publishes current Portable UI state.
    pub fn pump(&mut self) -> HostResult<()> {
        Ok(self.web_host.pump(&mut self.engine)?)
    }

    /// Returns local URLs exposed by active Web bundle components.
    pub fn web_urls(&self) -> HostResult<Vec<String>> {
        Ok(self.web_host.urls()?)
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
