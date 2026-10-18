//! Owner-scoped execution-target hosts used while loading RTW components.

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use rintawa_sdk::{
    contracts::ComponentRef,
    manifest::{ComponentDescriptor, WASM_COMPONENT_TARGET_V1, validate_component_target},
    types::{ComponentId, ComponentTarget, ExtensionInstanceId},
};

use crate::{
    artifact_host::RtwComponentHost,
    errors::{EngineError, EngineResult},
};

#[derive(Clone)]
pub(crate) struct RegisteredExecutionTargetHost {
    pub(crate) owner: ComponentRef,
    pub(crate) host: Arc<dyn RtwComponentHost>,
}

/// One exact execution-target dependency created while an extension is instantiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionTargetDependency {
    pub(crate) component_id: ComponentId,
    pub(crate) target: ComponentTarget,
    pub(crate) provider: ComponentRef,
}

#[derive(Default)]
struct ExecutionTargetRegistryState {
    hosts: BTreeMap<String, RegisteredExecutionTargetHost>,
}

/// Shared owner-scoped registry for non-built-in component execution targets.
#[derive(Clone, Default)]
pub(crate) struct ExecutionTargetRegistry {
    state: Arc<RwLock<ExecutionTargetRegistryState>>,
}

impl ExecutionTargetRegistry {
    pub(crate) fn register(
        &self,
        owner: ComponentRef,
        host: Arc<dyn RtwComponentHost>,
    ) -> EngineResult<()> {
        let target = host.target();
        validate_target(target)?;

        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.hosts.contains_key(target) {
            return Err(EngineError::DuplicateComponentHostTarget(
                target.to_string(),
            ));
        }
        state.hosts.insert(
            target.to_string(),
            RegisteredExecutionTargetHost { owner, host },
        );
        Ok(())
    }

    pub(crate) fn resolve(&self, target: &str) -> Option<RegisteredExecutionTargetHost> {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.hosts.get(target).cloned()
    }

    pub(crate) fn owner(&self, target: &str) -> Option<ComponentRef> {
        self.resolve(target).map(|registered| registered.owner)
    }

    pub(crate) fn revoke_component(&self, owner: &ComponentRef) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state
            .hosts
            .retain(|_, registered| &registered.owner != owner);
    }

    pub(crate) fn revoke_instance(&self, instance_id: &ExtensionInstanceId) {
        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state
            .hosts
            .retain(|_, registered| registered.owner.instance_id != *instance_id);
    }
}

fn validate_target(target: &str) -> EngineResult<()> {
    validate_component_target(target).map_err(|_| EngineError::InvalidComponentHostTarget)?;
    if target == WASM_COMPONENT_TARGET_V1 {
        return Err(EngineError::DuplicateComponentHostTarget(
            target.to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn dependency_from_descriptor(
    descriptor: &ComponentDescriptor,
    provider: ComponentRef,
) -> ExecutionTargetDependency {
    ExecutionTargetDependency {
        component_id: descriptor.id.clone(),
        target: descriptor.target.clone(),
        provider,
    }
}
