use std::sync::Arc;

use rintawa_artifacts::ArtifactDigest;
use rintawa_extension_engine::{ExtensionEngine, ExtensionState, RtwExtensionLoader};
use rintawa_sdk::types::{ExtensionId, ExtensionInstanceId, RuntimeScopeId};

use crate::{DevError, DevProject, DevResult, PreparedSnapshot, web::DevWebComponentHost};

/// Running local extension snapshot.
pub struct DevSession {
    snapshot: PreparedSnapshot,
    engine: ExtensionEngine,
    instance_id: ExtensionInstanceId,
    extension_id: ExtensionId,
    web_host: Arc<DevWebComponentHost>,
}

impl DevSession {
    /// Prepares and starts a project snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot preparation, loading, registration, or startup fails.
    pub fn start(
        project: &DevProject,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> DevResult<Self> {
        let snapshot = project.prepare_snapshot()?;
        Self::start_snapshot(snapshot, instance_id, scope_id)
    }

    pub(crate) fn start_snapshot(
        snapshot: PreparedSnapshot,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> DevResult<Self> {
        let mut engine = ExtensionEngine::new();
        let web_host = Arc::new(DevWebComponentHost::new());
        let mut loader = RtwExtensionLoader::new();
        loader.register_component_host(web_host.clone())?;
        let extension_id = loader.load_stored_extension(
            &mut engine,
            snapshot.store(),
            snapshot.digest(),
            instance_id.clone(),
            scope_id,
        )?;
        engine.start_extension_instance(&instance_id)?;
        if let Err(setup_error) = web_host
            .attach_layers(&mut engine)
            .and_then(|()| web_host.pump(&mut engine))
        {
            return match cleanup_engine(&mut engine, &instance_id) {
                Ok(()) => Err(setup_error),
                Err(cleanup_error) => Err(DevError::RuntimeSetupCleanupFailed {
                    setup: Box::new(setup_error),
                    cleanup: Box::new(cleanup_error),
                }),
            };
        }

        Ok(Self {
            snapshot,
            engine,
            instance_id,
            extension_id,
            web_host,
        })
    }

    /// Returns the logical extension ID.
    pub fn extension_id(&self) -> &ExtensionId {
        &self.extension_id
    }

    /// Returns the running snapshot digest.
    pub fn digest(&self) -> &ArtifactDigest {
        self.snapshot.digest()
    }

    /// Returns the runtime instance ID.
    pub fn instance_id(&self) -> &ExtensionInstanceId {
        &self.instance_id
    }

    /// Returns the current runtime state.
    pub fn state(&self) -> Option<ExtensionState> {
        self.engine.extension_instance_state(&self.instance_id)
    }

    /// Returns development URLs exposed by running Web bundle components.
    ///
    /// # Errors
    ///
    /// Returns an error when Web host state is unavailable.
    pub fn web_urls(&self) -> DevResult<Vec<String>> {
        self.web_host.urls()
    }

    /// Processes queued Web UI actions and publishes current portable UI state.
    ///
    /// # Errors
    ///
    /// Returns an engine or Web host error.
    pub fn pump(&mut self) -> DevResult<()> {
        self.web_host.pump(&mut self.engine)
    }

    /// Stops and unregisters the extension instance.
    ///
    /// # Errors
    ///
    /// Returns an engine cleanup error. If stop and unregister both fail, both
    /// errors are preserved in [`DevError::ShutdownFailed`].
    pub fn shutdown(self) -> DevResult<()> {
        self.shutdown_into_snapshot().map(drop)
    }

    pub(crate) fn shutdown_into_snapshot(mut self) -> DevResult<PreparedSnapshot> {
        cleanup_engine(&mut self.engine, &self.instance_id)?;
        Ok(self.snapshot)
    }
}

fn cleanup_engine(
    engine: &mut ExtensionEngine,
    instance_id: &ExtensionInstanceId,
) -> DevResult<()> {
    let stop_error = engine.stop_extension_instance(instance_id).err();
    let unregister_error = engine.unregister_extension_instance(instance_id).err();

    match (stop_error, unregister_error) {
        (None, None) => Ok(()),
        (Some(error), None) | (None, Some(error)) => Err(error.into()),
        (Some(stop), Some(unregister)) => Err(DevError::ShutdownFailed {
            stop: Box::new(stop),
            unregister: Box::new(unregister),
        }),
    }
}
