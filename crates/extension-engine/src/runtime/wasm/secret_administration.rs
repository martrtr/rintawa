//! Secret imports derive authority from the current execution, including delegated tasks.
use crate::runtime::wasm::{
    WasmHostState,
    bindings::rintawa::engine::{secret_administration as admin, secret_prompt as prompt},
};
use crate::{
    HostAccessError, LivePolicyControl, LivePolicyTarget, SecretAccess,
    runtime_permissions::RuntimeCancellation,
};
use rintawa_sdk::{
    runtime_permissions::RuntimePermission, secrets::SecretPath, types::ComponentId,
};
#[cfg(test)]
mod tests;

use serde::Deserialize;
use std::sync::Arc;

pub(super) struct OwnedOperation {
    pub(super) owner: LivePolicyTarget,
    handle: u64,
    prompt: bool,
    access: Arc<dyn SecretAccess>,
    policy: Arc<RuntimeCancellation>,
}
impl Drop for OwnedOperation {
    fn drop(&mut self) {
        self.policy
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.access.close(&self.owner, self.handle, true);
    }
}
fn error(e: HostAccessError) -> admin::Error {
    match e {
        HostAccessError::QueueFull => admin::Error::Busy,
        HostAccessError::Rejected => admin::Error::Denied,
        HostAccessError::NotFound => admin::Error::NotFound,
        HostAccessError::InvalidRequest => admin::Error::InvalidRequest,
        HostAccessError::MessageTooLarge => admin::Error::MessageTooLarge,
        _ => admin::Error::Unavailable,
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    component_id: String,
    slot: SecretPath,
}
impl WasmHostState {
    fn secret_owner(&self) -> Result<LivePolicyTarget, admin::Error> {
        if !self.host_access_active {
            return Err(admin::Error::Denied);
        }
        let ctx = self.active_execution_context().map_err(|e| match e {
            crate::runtime::wasm::RuntimeContextError::Unavailable => admin::Error::Unavailable,
            _ => admin::Error::Denied,
        })?;
        let (verified_ctx, verified) = self
            .transport
            .contexts
            .policy_identity(&ctx.principal.component())
            .map_err(|e| match e {
                crate::transport::TransportError::Unavailable => admin::Error::Unavailable,
                _ => admin::Error::Denied,
            })?;
        if verified_ctx != *ctx {
            return Err(admin::Error::Denied);
        }
        Ok(LivePolicyTarget {
            scope_id: ctx.scope.to_string(),
            instance_id: ctx.principal.instance_id.to_string(),
            component_id: ctx.principal.component_id.to_string(),
            activation: ctx.principal.activation.to_string(),
            artifact: verified.digest.to_string(),
        })
    }

    fn secret_permission(&self, is_prompt: bool) -> Result<(), admin::Error> {
        self.runtime_permission_owner(if is_prompt {
            RuntimePermission::SecretPrompt
        } else {
            RuntimePermission::SecretManage
        })
        .map_err(|e| match e {
            crate::runtime::wasm::RuntimePermissionCheck::Unavailable => admin::Error::Unavailable,
            _ => admin::Error::Denied,
        })?;
        if !is_prompt
            && self
                .active_execution_context()
                .map_err(|_| admin::Error::Denied)?
                .scope
                .as_str()
                != "host"
        {
            return Err(admin::Error::Denied);
        }
        Ok(())
    }
    fn secret_begin(&mut self, request: String, is_prompt: bool) -> Result<u64, admin::Error> {
        if request.len() > 60 * 1024 {
            return Err(admin::Error::MessageTooLarge);
        }
        let owner = self.secret_owner()?;
        self.secret_permission(is_prompt)?;
        let access = self
            .host_access
            .secret_access
            .clone()
            .ok_or(admin::Error::Unavailable)?;
        let authority = if is_prompt {
            let intent: Intent =
                serde_json::from_str(&request).map_err(|_| admin::Error::InvalidRequest)?;
            Some(
                LivePolicyControl::new(
                    self.transport.contexts.clone(),
                    self.runtime_permissions.clone(),
                    self.secrets.clone(),
                )
                .admit_secret_prompt(&owner, &ComponentId::new(intent.component_id), intent.slot)
                .map_err(error)?,
            )
        } else {
            None
        };
        let component = self.current_execution_owner().ok_or(admin::Error::Denied)?;
        let activation = self
            .active_execution_context()
            .map_err(|_| admin::Error::Denied)?
            .principal
            .activation;
        let policy = self
            .runtime_permissions
            .register_resource(
                component,
                activation,
                if is_prompt {
                    RuntimePermission::SecretPrompt
                } else {
                    RuntimePermission::SecretManage
                },
                None,
            )
            .map_err(|_| admin::Error::Denied)?;
        let handle = access
            .clone()
            .begin(&owner, &request, authority, policy.cancelled.clone())
            .map_err(error)?;
        self.secret_operations.insert(
            handle,
            OwnedOperation {
                owner,
                handle,
                prompt: is_prompt,
                access,
                policy,
            },
        );
        Ok(handle)
    }
    fn secret_status(&self, handle: u64, is_prompt: bool) -> Result<String, admin::Error> {
        let owner = self.secret_owner()?;
        self.secret_permission(is_prompt)?;
        let op = self
            .secret_operations
            .get(&handle)
            .filter(|op| op.owner == owner && op.prompt == is_prompt)
            .ok_or(admin::Error::NotFound)?;
        let value = op.access.status(&owner, handle).map_err(error)?;
        if value.len() > 60 * 1024 {
            return Err(admin::Error::MessageTooLarge);
        }
        Ok(value)
    }
    fn secret_close(
        &mut self,
        handle: u64,
        is_prompt: bool,
        release: bool,
    ) -> Result<(), admin::Error> {
        let owner = self.secret_owner()?;
        let op = self
            .secret_operations
            .get(&handle)
            .filter(|op| op.owner == owner && op.prompt == is_prompt)
            .ok_or(admin::Error::NotFound)?;
        op.access.close(&owner, handle, release).map_err(error)?;
        if release {
            self.secret_operations.remove(&handle);
        }
        Ok(())
    }
}
impl admin::Host for WasmHostState {
    fn begin(&mut self, request: String) -> Result<u64, admin::Error> {
        self.secret_begin(request, false)
    }
    fn status(&mut self, handle: u64) -> Result<String, admin::Error> {
        self.secret_status(handle, false)
    }
    fn cancel(&mut self, handle: u64) -> Result<(), admin::Error> {
        self.secret_close(handle, false, false)
    }
    fn release(&mut self, handle: u64) -> Result<(), admin::Error> {
        self.secret_close(handle, false, true)
    }
}
impl prompt::Host for WasmHostState {
    fn begin(&mut self, request: String) -> Result<u64, admin::Error> {
        self.secret_begin(request, true)
    }
    fn status(&mut self, handle: u64) -> Result<String, admin::Error> {
        self.secret_status(handle, true)
    }
    fn cancel(&mut self, handle: u64) -> Result<(), admin::Error> {
        self.secret_close(handle, true, false)
    }
    fn release(&mut self, handle: u64) -> Result<(), admin::Error> {
        self.secret_close(handle, true, true)
    }
}
