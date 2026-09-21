//! Delegated components provided by a WASM execution-target provider.

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contracts::ContractKey,
    errors::{ExtensionError, ExtensionResult},
    traits::Component,
    types::ComponentId,
    ui::UiActionEvent,
};
use tracing::warn;
use wasmtime::component::Resource;

use crate::{
    artifact_host::{
        RtwComponentHost, RtwComponentHostError, RtwComponentHostResult, RtwComponentSource,
    },
    runtime::wasm::{
        WasmComponent, WasmExecutionBudget, WasmInstance, WasmRegistrations, WasmSharedRuntime,
        apply_wasm_registrations,
        target_provider_bindings::{
            TargetProviderPlugin,
            exports::rintawa::engine::target_provider::{
                ComponentDescriptor as WitTargetComponentDescriptor, Error as WitTargetError,
            },
        },
    },
};

#[derive(Clone)]
pub(super) struct WasmTargetProviderEndpoint {
    pub(super) runtime: Arc<Mutex<WasmSharedRuntime>>,
    pub(super) budget: WasmExecutionBudget,
}

pub(super) struct WasmExecutionTargetHost {
    pub(super) target: String,
    pub(super) provider: WasmTargetProviderEndpoint,
}

struct WasmExecutionTargetProxy {
    id: ComponentId,
    handle: u64,
    provider: WasmTargetProviderEndpoint,
}

impl WasmTargetProviderEndpoint {
    fn runtime(&self) -> RtwComponentHostResult<MutexGuard<'_, WasmSharedRuntime>> {
        match self.runtime.try_lock() {
            Ok(runtime) => Ok(runtime),
            Err(TryLockError::WouldBlock) => Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime is busy",
            ))),
            Err(TryLockError::Poisoned(_)) => Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime lock was poisoned",
            ))),
        }
    }

    fn live_instance(runtime: &mut WasmSharedRuntime) -> RtwComponentHostResult<&mut WasmInstance> {
        if runtime.failed_lifecycle_callback.is_some() {
            return Err(RtwComponentHostError::Host(String::from(
                "WASM target-provider runtime is unavailable after lifecycle failure",
            )));
        }
        runtime.instance.as_mut().ok_or_else(|| {
            RtwComponentHostError::Host(String::from("WASM target-provider is not running"))
        })
    }

    pub(super) fn ensure_target_provider(instance: &mut WasmInstance) -> ExtensionResult<()> {
        if instance.target_provider.is_none() {
            let provider = TargetProviderPlugin::new(&mut instance.store, &instance.instance)
                .map_err(|error| {
                    ExtensionError::Message(format!(
                        "WASM component published an execution target without target-provider exports: {error}"
                    ))
                })?;
            instance.target_provider = Some(provider);
        }
        Ok(())
    }

    fn load_component(
        &self,
        target: &str,
        source: &RtwComponentSource<'_>,
        descriptor: &rintawa_sdk::manifest::ComponentDescriptor,
    ) -> RtwComponentHostResult<u64> {
        let owned_source = source.fork_owned()?;
        let mut runtime = self.runtime()?;
        let instance = Self::live_instance(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target load")
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        Self::ensure_target_provider(instance)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        let source_resource = instance
            .store
            .data_mut()
            .resource_table
            .push(owned_source)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()))?;
        let borrowed_source = Resource::new_borrow(source_resource.rep());
        let kind = match descriptor.kind {
            rintawa_sdk::manifest::ComponentKind::Runtime => String::from("runtime"),
            rintawa_sdk::manifest::ComponentKind::Ui => String::from("ui"),
        };
        let descriptor = WitTargetComponentDescriptor {
            id: descriptor.id.to_string(),
            kind,
            target: descriptor.target.to_string(),
            entry: descriptor.entry.clone(),
            required: descriptor.required,
        };
        let provider = instance.target_provider.as_ref().ok_or_else(|| {
            RtwComponentHostError::Host(String::from("target-provider export view is unavailable"))
        })?;
        let result = provider
            .rintawa_engine_target_provider()
            .call_load_component(&mut instance.store, target, &descriptor, borrowed_source)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()));
        let cleanup = instance
            .store
            .data_mut()
            .resource_table
            .delete(source_resource)
            .map_err(|error| RtwComponentHostError::Host(error.to_string()));
        let provider_result = result?;
        cleanup?;
        provider_result.map_err(|error| {
            RtwComponentHostError::Host(format!(
                "WASM target-provider load callback failed: {error:?}"
            ))
        })
    }
}

fn map_wit_target_host_error(operation: &'static str, error: WitTargetError) -> ExtensionError {
    ExtensionError::Message(format!(
        "WASM target-provider `{operation}` callback failed: {error:?}"
    ))
}

impl WasmTargetProviderEndpoint {
    pub(super) fn runtime_for_callback(
        &self,
    ) -> ExtensionResult<MutexGuard<'_, WasmSharedRuntime>> {
        match self.runtime.try_lock() {
            Ok(runtime) => Ok(runtime),
            Err(TryLockError::WouldBlock) => Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime is busy",
            ))),
            Err(TryLockError::Poisoned(_)) => Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime lock was poisoned",
            ))),
        }
    }

    fn live_instance_for_callback(
        runtime: &mut WasmSharedRuntime,
    ) -> ExtensionResult<&mut WasmInstance> {
        if runtime.failed_lifecycle_callback.is_some() {
            return Err(ExtensionError::Message(String::from(
                "WASM target-provider runtime is unavailable after lifecycle failure",
            )));
        }
        runtime.instance.as_mut().ok_or_else(|| {
            ExtensionError::Message(String::from("WASM target-provider is not running"))
        })
    }

    fn register_component(&self, handle: u64) -> ExtensionResult<WasmRegistrations> {
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target register")?;
        instance
            .store
            .data_mut()
            .begin_target_component_registration()?;
        let result = match instance.target_provider.as_ref() {
            Some(provider) => provider
                .rintawa_engine_target_provider()
                .call_register_component(&mut instance.store, handle)
                .map_err(|error| WasmComponent::execution_error("target register", error)),
            None => Err(ExtensionError::Message(String::from(
                "target-provider export view is unavailable",
            ))),
        };
        match result {
            Ok(Ok(())) => instance.store.data_mut().finish_registration(),
            Ok(Err(error)) => {
                instance
                    .store
                    .data_mut()
                    .cancel_target_component_registration();
                Err(map_wit_target_host_error("register", error))
            }
            Err(error) => {
                instance
                    .store
                    .data_mut()
                    .cancel_target_component_registration();
                Err(error)
            }
        }
    }

    fn start_component(&self, handle: u64, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target start")?;
        instance
            .store
            .data_mut()
            .begin_delegated_guest_execution(owner);
        let result = match instance.target_provider.as_ref() {
            Some(provider) => provider
                .rintawa_engine_target_provider()
                .call_start_component(&mut instance.store, handle)
                .map_err(|error| WasmComponent::execution_error("target start", error)),
            None => Err(ExtensionError::Message(String::from(
                "target-provider export view is unavailable",
            ))),
        };
        match result {
            Ok(Ok(())) => match instance.store.data_mut().finish_guest_execution(ctx) {
                Ok(()) => Ok(()),
                Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
            },
            Ok(Err(error)) => {
                let error = map_wit_target_host_error("start", error);
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            }
            Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
        }
    }

    pub(super) fn stop_component(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
    ) -> ExtensionResult<()> {
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let (mut runtime, lock_failure) = match self.runtime.try_lock() {
            Ok(runtime) => (runtime, None),
            Err(TryLockError::WouldBlock) => {
                return Err(ExtensionError::Message(String::from(
                    "WASM target-provider runtime is busy",
                )));
            }
            Err(TryLockError::Poisoned(poisoned)) => (
                poisoned.into_inner(),
                Some(ExtensionError::Message(String::from(
                    "WASM target-provider runtime lock was poisoned",
                ))),
            ),
        };
        let result = if let Some(error) = lock_failure {
            Err(error)
        } else {
            match Self::live_instance_for_callback(&mut runtime) {
                Ok(instance) => match WasmComponent::set_callback_fuel(
                    &mut instance.store,
                    &self.budget,
                    "target stop",
                ) {
                    Ok(()) => match instance.target_provider.as_ref() {
                        Some(provider) => provider
                            .rintawa_engine_target_provider()
                            .call_stop_component(&mut instance.store, handle)
                            .map_err(|error| WasmComponent::execution_error("target stop", error))
                            .and_then(|result| {
                                result.map_err(|error| map_wit_target_host_error("stop", error))
                            }),
                        None => Err(ExtensionError::Message(String::from(
                            "target-provider export view is unavailable",
                        ))),
                    },
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        };
        if let Some(instance) = runtime.instance.as_mut() {
            instance
                .store
                .data_mut()
                .forget_effect_handles_for_owner(&owner);
            instance
                .store
                .data_mut()
                .revoke_runtime_resources_for_owner(&owner);
        }
        result
    }

    fn handle_event(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        if topic.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target event topic",
                actual_bytes: topic.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        if payload.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target event payload",
                actual_bytes: payload.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }

        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target event")?;
        instance
            .store
            .data_mut()
            .begin_delegated_guest_execution(owner);
        let result = match instance.target_provider.as_ref() {
            Some(provider) => provider
                .rintawa_engine_target_provider()
                .call_handle_event(&mut instance.store, handle, topic, payload)
                .map_err(|error| WasmComponent::execution_error("target event", error)),
            None => Err(ExtensionError::Message(String::from(
                "target-provider export view is unavailable",
            ))),
        };

        match result {
            Ok(Ok(())) => match instance.store.data_mut().finish_guest_execution(ctx) {
                Ok(()) => Ok(()),
                Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
            },
            Ok(Err(error)) => {
                let error = map_wit_target_host_error("event", error);
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            }
            Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
        }
    }

    fn handle_ui_action(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        if payload.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target UI action",
                actual_bytes: payload.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_ui_action_fuel(&mut instance.store, &self.budget, "target UI action")?;
        instance
            .store
            .data_mut()
            .begin_delegated_guest_execution(owner);
        let result = match instance.target_provider.as_ref() {
            Some(provider) => provider
                .rintawa_engine_target_provider()
                .call_handle_ui_action(&mut instance.store, handle, payload)
                .map_err(|error| WasmComponent::execution_error("target UI action", error)),
            None => Err(ExtensionError::Message(String::from(
                "target-provider export view is unavailable",
            ))),
        };
        match result {
            Ok(Ok(())) => match instance.store.data_mut().finish_guest_execution(ctx) {
                Ok(()) => Ok(()),
                Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
            },
            Ok(Err(error)) => {
                let error = map_wit_target_host_error("UI action", error);
                Err(instance.store.data_mut().abort_guest_execution(ctx, error))
            }
            Err(error) => Err(instance.store.data_mut().abort_guest_execution(ctx, error)),
        }
    }

    fn handle_service(
        &self,
        handle: u64,
        ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        if request.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target service request",
                actual_bytes: request.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        let owner = rintawa_sdk::contracts::ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target service")?;
        instance
            .store
            .data_mut()
            .begin_delegated_service_execution(owner);
        let result = match instance.target_provider.as_ref() {
            Some(provider) => provider
                .rintawa_engine_target_provider()
                .call_handle_service(
                    &mut instance.store,
                    handle,
                    contract.id.as_str(),
                    contract.version.major(),
                    request,
                )
                .map_err(|error| WasmComponent::execution_error("target service", error)),
            None => Err(ExtensionError::Message(String::from(
                "target-provider export view is unavailable",
            ))),
        };
        instance.store.data_mut().finish_service_execution();
        let response = result?.map_err(|error| map_wit_target_host_error("service", error))?;
        if response.len() > self.budget.max_host_message_bytes {
            return Err(ExtensionError::HostMessageTooLarge {
                operation: "target service response",
                actual_bytes: response.len(),
                maximum_bytes: self.budget.max_host_message_bytes,
            });
        }
        Ok(response)
    }

    fn drop_component(&self, handle: u64) -> ExtensionResult<()> {
        let mut runtime = self.runtime_for_callback()?;
        let instance = Self::live_instance_for_callback(&mut runtime)?;
        WasmComponent::set_callback_fuel(&mut instance.store, &self.budget, "target drop")?;
        instance
            .target_provider
            .as_ref()
            .ok_or_else(|| {
                ExtensionError::Message(String::from("target-provider export view is unavailable"))
            })?
            .rintawa_engine_target_provider()
            .call_drop_component(&mut instance.store, handle)
            .map_err(|error| WasmComponent::execution_error("target drop", error))?
            .map_err(|error| map_wit_target_host_error("drop", error))
    }
}

impl RtwComponentHost for WasmExecutionTargetHost {
    fn target(&self) -> &str {
        &self.target
    }

    fn load_component(
        &self,
        source: &mut RtwComponentSource<'_>,
        descriptor: &rintawa_sdk::manifest::ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        let handle = self
            .provider
            .load_component(&self.target, source, descriptor)?;
        Ok(Box::new(WasmExecutionTargetProxy {
            id: descriptor.id.clone(),
            handle,
            provider: self.provider.clone(),
        }))
    }
}

impl Component for WasmExecutionTargetProxy {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        let registrations = self.provider.register_component(self.handle)?;
        apply_wasm_registrations(registrations, ctx)
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.provider.start_component(self.handle, ctx)
    }

    fn stop(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.provider.stop_component(self.handle, ctx)
    }

    fn handle_event(
        &mut self,
        ctx: &mut dyn ComponentContext,
        topic: &str,
        payload: &[u8],
    ) -> ExtensionResult<()> {
        self.provider.handle_event(self.handle, ctx, topic, payload)
    }

    fn handle_ui_action(
        &mut self,
        ctx: &mut dyn ComponentContext,
        event: &UiActionEvent,
    ) -> ExtensionResult<()> {
        let payload = serde_json::to_vec(event).map_err(|error| {
            ExtensionError::Message(format!("could not encode target UI action: {error}"))
        })?;
        self.provider.handle_ui_action(self.handle, ctx, &payload)
    }

    fn service_message_limit(&self) -> Option<usize> {
        Some(self.provider.budget.max_host_message_bytes)
    }

    fn handle_service(
        &mut self,
        ctx: &mut dyn ComponentContext,
        contract: &ContractKey,
        request: &[u8],
    ) -> ExtensionResult<Vec<u8>> {
        self.provider
            .handle_service(self.handle, ctx, contract, request)
    }
}

impl Drop for WasmExecutionTargetProxy {
    fn drop(&mut self) {
        if let Err(error) = self.provider.drop_component(self.handle) {
            warn!(component = %self.id, error = %error, "WASM target-provider drop callback failed");
        }
    }
}
