//! Native worker boundary for Web secret administration. No value-read operation exists.
use crate::runtime::LocalHostAccess;

#[cfg(test)]
mod tests;
use crate::{SecretReadGrant, SecretStorageError};
use rintawa_extension_engine::{
    HostAccessError, HostAccessResult, LivePolicyTarget, SecretAccess, SecretPromptAuthority,
};
use rintawa_sdk::secrets::{SecretPath, SecretValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct Operations(Mutex<Book>);
#[derive(Default)]
struct Book {
    next: u64,
    entries: HashMap<u64, Operation>,
}
struct Operation {
    owner: LivePolicyTarget,
    cancelled: Arc<AtomicBool>,
    released: bool,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
    prompt: Option<Arc<Prompt>>,
}
struct Prompt {
    authority: SecretPromptAuthority,
    expires: Instant,
    cancelled: Arc<AtomicBool>,
    binding: AtomicBool,
    outcome: Mutex<Option<&'static str>>,
}
impl Prompt {
    fn check_lifetime(&self) -> HostAccessResult<()> {
        if Instant::now() >= self.expires {
            self.cancelled.store(true, Ordering::Release);
        }
        if self.cancelled.load(Ordering::Acquire) {
            return Err(HostAccessError::Rejected);
        }
        Ok(())
    }
    fn check(&self) -> HostAccessResult<()> {
        self.check_lifetime()?;
        self.authority.validate()
    }
}
#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    List {
        offset: u32,
    },
    Consumers {
        offset: u32,
    },
    Create {
        label: String,
        value: String,
    },
    Rename {
        id: Uuid,
        revision: u64,
        label: String,
    },
    Replace {
        id: Uuid,
        revision: u64,
        value: String,
    },
    Delete {
        id: Uuid,
        revision: u64,
    },
    Bind {
        target: LivePolicyTarget,
        slot: SecretPath,
        id: Uuid,
    },
    Disconnect {
        grant: SecretReadGrant,
    },
    InspectPrompt {
        prompt: String,
    },
    BindPrompt {
        prompt: String,
        id: Uuid,
    },
}
fn fail(e: HostAccessError) -> String {
    match e {
        HostAccessError::Rejected => "stale",
        HostAccessError::NotFound => "not-found",
        _ => "unavailable",
    }
    .into()
}
fn storage(e: SecretStorageError) -> String {
    match e {
        SecretStorageError::InvalidInput => "invalid-request",
        SecretStorageError::NotFound => "not-found",
        SecretStorageError::RevisionConflict => "revision-conflict",
        SecretStorageError::QuotaExceeded => "quota-exceeded",
        SecretStorageError::DurabilityUnknown => "durability-unknown",
        SecretStorageError::CleanupFailed => "cleanup-failed",
        _ => "unavailable",
    }
    .into()
}
fn page<T: serde::Serialize>(items: Vec<T>, offset: u32) -> Result<Value, String> {
    let total = items.len();
    let offset = offset as usize;
    if offset > total {
        return Err("invalid-request".into());
    }
    let mut rows = Vec::new();
    for item in items.into_iter().skip(offset).take(32) {
        let row = serde_json::to_value(item).map_err(|_| "unavailable")?;
        rows.push(row);
        if serde_json::to_vec(&rows).map_err(|_| "unavailable")?.len() > 56 * 1024 {
            rows.pop();
            break;
        }
    }
    Ok(json!({"items":rows,"nextOffset":offset+rows.len(),"total":total}))
}
impl Operations {
    pub(super) fn shutdown(&self) {
        if let Ok(book) = self.0.lock() {
            for op in book.entries.values() {
                op.cancelled.store(true, Ordering::Release);
            }
        }
    }
}
impl SecretAccess for LocalHostAccess {
    fn begin(
        self: Arc<Self>,
        owner: &LivePolicyTarget,
        request: &str,
        prompt: Option<SecretPromptAuthority>,
        cancelled: Arc<AtomicBool>,
    ) -> HostAccessResult<u64> {
        if request.len() > 60 * 1024 {
            return Err(HostAccessError::MessageTooLarge);
        }
        let parsed = if prompt.is_none() {
            Some(
                serde_json::from_str::<Request>(request)
                    .map_err(|_| HostAccessError::InvalidRequest)?,
            )
        } else {
            None
        };
        // Bound input validation happens before worker admission and before any storage change.
        if let Some(Request::Create { label, value }) = &parsed
            && (label.is_empty() || label.len() > 128 || value.is_empty() || value.len() > 65536)
        {
            return Err(HostAccessError::InvalidRequest);
        }
        if let Some(Request::Replace { value, .. }) = &parsed
            && (value.is_empty() || value.len() > 65536)
        {
            return Err(HostAccessError::InvalidRequest);
        }
        let result = Arc::new(Mutex::new(None));
        let mut book = self
            .secret_operations
            .0
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        book.entries.retain(|_, op| {
            !op.released
                || op.prompt.as_ref().map_or_else(
                    || !op.result.lock().is_ok_and(|r| r.is_some()),
                    |p| Arc::strong_count(p) > 1,
                )
        });
        if prompt.is_some() {
            if book
                .entries
                .values()
                .filter(|op| op.prompt.is_some())
                .count()
                >= 16
                || book
                    .entries
                    .values()
                    .any(|op| op.prompt.is_some() && op.owner == *owner)
            {
                return Err(HostAccessError::QueueFull);
            }
        } else if book
            .entries
            .values()
            .filter(|op| op.prompt.is_none() && op.owner == *owner)
            .count()
            >= 8
        {
            return Err(HostAccessError::QueueFull);
        }
        book.next = book
            .next
            .checked_add(1)
            .ok_or(HostAccessError::Unavailable)?;
        let handle = book.next;
        let prompt = prompt.map(|authority| {
            Arc::new(Prompt {
                authority,
                expires: Instant::now() + Duration::from_secs(300),
                cancelled: cancelled.clone(),
                binding: AtomicBool::new(false),
                outcome: Mutex::new(None),
            })
        });
        book.entries.insert(
            handle,
            Operation {
                owner: owner.clone(),
                cancelled: cancelled.clone(),
                released: false,
                result: result.clone(),
                prompt,
            },
        );
        drop(book);
        if let Some(request) = parsed {
            let access = self.clone();
            let owner = owner.clone();
            if std::thread::Builder::new()
                .name("secret-operation".into())
                .spawn(move || {
                    let outcome = if cancelled.load(Ordering::Acquire) {
                        Err("cancelled".into())
                    } else {
                        access.perform_secret(&owner, request, &cancelled)
                    };
                    if let Ok(mut slot) = result.lock() {
                        *slot = Some(outcome);
                    }
                })
                .is_err()
            {
                self.secret_operations
                    .0
                    .lock()
                    .map_err(|_| HostAccessError::Unavailable)?
                    .entries
                    .remove(&handle);
                return Err(HostAccessError::Unavailable);
            }
        }
        Ok(handle)
    }
    fn status(&self, owner: &LivePolicyTarget, handle: u64) -> HostAccessResult<String> {
        let book = self
            .secret_operations
            .0
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let op = book
            .entries
            .get(&handle)
            .filter(|op| op.owner == *owner && !op.released)
            .ok_or(HostAccessError::NotFound)?;
        let value = if let Some(prompt) = &op.prompt {
            let outcome = *prompt
                .outcome
                .lock()
                .map_err(|_| HostAccessError::Unavailable)?;
            if let Some(outcome) = outcome {
                json!({"status":"completed","result":{"outcome":outcome}})
            } else {
                match prompt.check() {
                    Ok(()) => json!({"status":"pending"}),
                    Err(HostAccessError::Rejected) => {
                        json!({"status":"completed","result":{"outcome":"stale"}})
                    }
                    Err(error) => return Err(error),
                }
            }
        } else {
            match &*op.result.lock().map_err(|_| HostAccessError::Unavailable)? {
                None => json!({"status":"pending"}),
                Some(Ok(result)) => json!({"status":"completed","result":result}),
                Some(Err(code)) => json!({"status":"failed","code":code}),
            }
        };
        let encoded = serde_json::to_string(&value).map_err(|_| HostAccessError::Unavailable)?;
        if encoded.len() > 60 * 1024 {
            return Err(HostAccessError::MessageTooLarge);
        }
        Ok(encoded)
    }
    fn close(&self, owner: &LivePolicyTarget, handle: u64, release: bool) -> HostAccessResult<()> {
        let mut book = self
            .secret_operations
            .0
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?;
        let op = book
            .entries
            .get_mut(&handle)
            .filter(|op| op.owner == *owner)
            .ok_or(HostAccessError::NotFound)?;
        op.cancelled.store(true, Ordering::Release);
        op.released |= release;
        if release
            && (op
                .prompt
                .as_ref()
                .is_some_and(|p| Arc::strong_count(p) == 1)
                || op.result.lock().is_ok_and(|r| r.is_some()))
        {
            book.entries.remove(&handle);
        }
        Ok(())
    }
}
impl LocalHostAccess {
    fn retained_prompt(&self, handle: &str) -> HostAccessResult<Arc<Prompt>> {
        let id = handle
            .parse::<u64>()
            .map_err(|_| HostAccessError::InvalidRequest)?;
        self.secret_operations
            .0
            .lock()
            .map_err(|_| HostAccessError::Unavailable)?
            .entries
            .get(&id)
            .filter(|op| !op.released)
            .and_then(|op| op.prompt.clone())
            .ok_or(HostAccessError::NotFound)
    }
    fn perform_secret(
        &self,
        owner: &LivePolicyTarget,
        request: Request,
        cancelled: &AtomicBool,
    ) -> Result<Value, String> {
        // This is native commit admission: revalidate the actual admin activation and live grant.
        self.policy_control()
            .map_err(fail)?
            .admit_secret_management(owner, cancelled)
            .map_err(fail)?;
        let store = self.secrets.as_ref().ok_or("unavailable")?;
        let value = match request {
            Request::List { offset } => {
                let mut value = page(store.list().map_err(storage)?, offset)?;
                value["backend"] = json!(store.backend_name());
                value
            }
            Request::Consumers { offset } => page(
                self.home()
                    .map_err(fail)?
                    .list_secret_consumers()
                    .map_err(|_| "unavailable")?,
                offset,
            )?,
            Request::Create { label, value } => json!(
                store
                    .create(&label, &SecretValue::new(value))
                    .map_err(storage)?
            ),
            Request::Rename {
                id,
                revision,
                label,
            } => {
                store
                    .edit(id, revision, Some(&label), None)
                    .map_err(storage)?;
                json!({})
            }
            Request::Replace {
                id,
                revision,
                value,
            } => {
                store
                    .edit(id, revision, None, Some(&SecretValue::new(value)))
                    .map_err(storage)?;
                json!({})
            }
            Request::Delete { id, revision } => {
                store.remove(id, revision).map_err(storage)?;
                json!({})
            }
            Request::Bind { target, slot, id } => {
                self.bind_secret_selection(&target, slot, id, None, Some(cancelled))
                    .map_err(fail)?;
                json!({})
            }
            Request::Disconnect { grant } => {
                self.disconnect_secret_consumer(&grant, cancelled)
                    .map_err(fail)?;
                json!({})
            }
            Request::InspectPrompt { prompt } => {
                let prompt = self.retained_prompt(&prompt).map_err(fail)?;
                prompt.check().map_err(fail)?;
                let target = prompt.authority.target();
                let requester = prompt.authority.requester_target();
                let manifest = self
                    .home()
                    .map_err(fail)?
                    .extension_manifest(&requester.artifact.parse().map_err(|_| "stale")?)
                    .map_err(|_| "unavailable")?;
                json!({"target":target,"requester":requester,"slot":prompt.authority.slot(),"packageId":manifest.id,"packageName":manifest.name})
            }
            Request::BindPrompt { prompt, id } => {
                let prompt = self.retained_prompt(&prompt).map_err(fail)?;
                prompt.check_lifetime().map_err(fail)?;
                if prompt.binding.swap(true, Ordering::AcqRel) {
                    return Err("stale".into());
                }
                let result = self.bind_secret_selection(
                    prompt.authority.target(),
                    prompt.authority.slot().clone(),
                    id,
                    Some(&prompt.authority),
                    Some(&prompt.cancelled),
                );
                match result {
                    Ok(()) => {
                        *prompt.outcome.lock().map_err(|_| "unavailable")? = Some("bound");
                        json!({})
                    }
                    Err(e) => {
                        prompt.binding.store(false, Ordering::Release);
                        return Err(fail(e));
                    }
                }
            }
        };
        Ok(value)
    }
}
