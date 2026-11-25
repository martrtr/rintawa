# Secret management inside the existing Web UI

Status: accepted by the owner on 2026-11-25 with the instruction to begin
implementation. Replaces the native-only input boundary, not the storage model.
Linux implementation, browser acceptance and removal of native window
infrastructure are recorded below.

Keep shared-secret-storage.md authoritative for entries, bindings, storage,
rotation and revocation, except its native-only input requirements. No platform
portability refactor is required for this Linux Web UI slice.

## Decision and threat boundary

Use an existing shell-owned modal, opened from an extension's secret control,
just like existing permission review. Root owns the form; the requesting settings
iframe never receives the value, global list or management RPC capability. Reuse
the existing authenticated browser/runtime channel and identity binding. Do not
create a second web server, credential origin, GTK helper or new generic UI framework.

The selected administrative shell and web runtime are trusted with entered values:
bytes necessarily exist in their memory and traverse the authenticated channel.
A malicious replacement shell or compromised runtime can steal them. The previous
native design avoided this exposure; do not keep that stronger claim after this
change. Root authority is selected/approved by Host policy, not by claiming to be
Root in JSON. Existing untrusted nested frames retain isolation. Local loopback
only for this slice; LAN/remote credential entry requires authenticated encrypted
transport and a separate accepted remote-client design. Never expose these methods
through an unauthenticated remote endpoint.

## Minimal integration

A settings iframe requests selection for an exact logical slot and sibling backend.
Reuse attested requester identity, manifest checks and the existing secret-prompt
admission semantics; rename/document it as UI-independent if necessary rather
than retaining a native-only promise. The request contains no credential value.
Root opens a modal showing Host-verified target scope/component/artifact and slot.
User may select an existing entry or create one, then explicitly approve its use.
One successful action binds and grants immediately; no restart or second read-grant
prompt. The frame receives only bound/cancelled/unavailable/stale/unknown outcome.

## Host authority: exact enforcement path

Current evidence: rtwKit packages/web-runtime/src/wasm.rs executes queued calls in
its hosted component task, resolves the live owner, selects only that owner's
channels and applies Call::shell_only before dispatch. Core's runtime/wasm/
control_plane.rs uses runtime_permission_owner for administrative capabilities.
The native delegated execution context, not browser-supplied identity, is the
source of authority. Preserve this existing route; no new attestation service.

Add one generic runtime capability `secret-manage`. Grant it explicitly to the
selected baseline shell component, not to the web-runtime provider component,
ordinary Settings contributors or AI backend. Host checks it against the current
attested executing principal for administration begin/status, as defined below. A
manifest request or becoming the selected shell does not grant it. Only baseline
administration is supported in this slice; reject World-scoped administrators.
A user can intentionally trust another baseline administrator through existing
Host grant mechanisms; the capability is not a Core knowledge of “Web UI”.

At the bridge, mark all management calls shell-only and dispatch under that shell's
delegated context. Core owns operations by principal/activation; web runtime binds
each native handle to its admitted browser channel as specified below;
a shell switch retires old browser access. Native capability revocation blocks
future admission/reads and is rechecked at commit. The runtime's provider context
must never substitute for the shell's. Tests must attempt that substitution.
A hostile arbitrary frame with a matching JSON method name fails both route and
native permission checks. Host cannot prove a human clicked a button; an approved
administrator is trusted to collect consent. State this honestly.

Keep read-only `secrets.read` unchanged. Add a separate generic privileged
`secret-administration` Host/WIT interface: begin(request), status(handle),
cancel(handle), release(handle). It exposes metadata/consumer listing, create, rename, replace,
delete, bind and disconnect. There is no value-read operation. Ordinary guests may
link the interface, but begin/status without `secret-manage` fail natively.
Ownership-only cleanup is defined below. WIT availability is not authorization. Reuse existing managed-store methods, revision checks and
live-policy binding transaction; do not create another store or grant system.

### Normative administration lifecycle

| Operation | Native authorization | Result after permission revocation |
| --- | --- | --- |
| begin | Active baseline principal and current secret-manage | Denied; no work admitted |
| status | Original principal/activation and current secret-manage | Denied, including retained metadata/results |
| cancel | Original principal/activation owns handle; no secret-manage check | Allowed while owner can execute; cancels work not yet commit-admitted |
| release | Original principal/activation owns handle; no secret-manage check | Allowed; frees terminal state or marks pending state for cleanup |
| stop/trap cleanup | Internal native lifecycle ownership | Invalidates admission and retires resources without executing as the stopped owner |

Ownership includes the recorded scope. A restarted activation cannot inherit old
handles. Cancel/release return only cleanup acknowledgement or invalid-handle, not
metadata, values or mutation results. Revocation is not a reason to deny cleanup.
A retired owner cannot invoke guest calls; native lifecycle cleanup covers that
case. Pending resources remain counted until their worker actually terminates.
Runtime retirement cleanup must bypass frontend shell-selection checks ONLY for
its own retained resource map, never by admitting new browser calls on a dead
channel. Core still enforces the original executing owner or performs internal
teardown; runtime-provider identity is never a substitute.

Requests use a strict tagged DTO with only each operation's existing domain fields.
Bind includes exact scope/instance/component/artifact/activation, slot and entry ID;
these identify the target, never the administrator. Native validates target live
membership, manifest coverage and profile selection again at commit. Existing
entry edit/delete requires expected revision; disconnect requires the exact expected
binding so a stale view cannot revoke a replacement. No wildcard binding, foreign
administrator identity, URL, filesystem path or raw profile patch is accepted.
Host returns verified target metadata before the confirmation UI; untrusted intent
labels cannot replace it. Picker binding uses the captured prompt authority below,
not a fresh target reconstructed from frontend JSON.

Use bounded asynchronous storage work outside guest/Engine locks. Maximum 8 retained
operations per administrator activation, including released-pending work; 60 KiB
encoded request/result, at most 32 metadata rows per page, and existing store quotas.
Keep paginated consumer lists within the same result bound. Reject oversize before
side effects. For browser value entry the encoded envelope limit can be lower than
the native CLI's 64 KiB raw UTF-8 limit: show an actionable “use native stdin for
this larger value” error, never truncate or silently enlarge the transport.

Status is pending / completed(result) / failed(code), with explicit durability-unknown
and cleanup-failed cases. Begin acknowledgement means admission, not persistence.
Status and errors never echo request values. Released pending work remains charged
until terminal. Native stop/revocation and delivered cancellation prevent later
commit admission; channel loss schedules that cancellation and is not itself a
synchronous Host fence. An already admitted commit may survive disconnect. Do not force-kill vault I/O and
claim rollback. Do not auto retry an unknown create/update. Reconcile metadata or
report unknown if the outcome cannot be determined. Reuse bounded worker mechanisms
where applicable; document dependencies whose external I/O cannot be interrupted.
No new browser credential server, distributed transaction or durable job queue.

### Separate picker admission (`secret-prompt`)

An ordinary iframe cannot call administration. Its existing bridge request becomes
`secrets.request-selection {componentId, slot}`; there are no requester, scope,
instance, artifact, entry ID or secret-value fields. Web runtime validates the
frame's authenticated channel and executes a new bounded `secret-prompt.begin`
Host import under that frame owner's delegated context. Core derives the requester
from its active execution context and calls the existing
`LivePolicyControl::admit_secret_prompt` using that verified identity. It checks the
actual `secret-prompt` grant, live sibling in the same instance/artifact/scope and
manifest coverage. It retains the resulting `SecretPromptAuthority` natively.

Begin returns an opaque prompt handle referencing that retained authority, not a
serialized proof. Maximum one outstanding prompt per requester activation and
16 per Host; excess requests are busy. Handles expire after 5 minutes of Host
monotonic time and on permission/activation retirement. `secret-prompt.cancel`
and `release` accept only the original attested requester; they require ownership,
not a still-granted prompt permission, so revocation cannot block cleanup. Cancel
invalidates uncommitted use; release invalidates and frees the prompt when no
in-flight administrative operation references it. All retained records count until
freed. This bounded native state is the lifetime of a picker, not another grant store.

Web runtime retains the handle on the requesting channel and forwards a picker
notification to the current shell. Use the existing authenticated shell/runtime
route, not a global Zenoh publication. The iframe receives only admission/outcome,
not the native handle. Notification receipt grants no authority. Shell calls
administration `begin({kind:"inspect-prompt",prompt})` under its own delegated
context and `secret-manage` grant. Host validates the prompt authority and returns
verified requester/target/slot metadata. No browser-supplied requester is trusted.

On confirmation shell uses `begin({kind:"bind-prompt",prompt,entryId})`. Host
revalidates administrator authority AND the retained requester/sibling/permission
lease and uses `SecretPromptAuthority::bind` within the existing persisted/live
binding transaction. A successful bind consumes the prompt; duplicate submission
cannot create another mutation. Bind admitted concurrently with cancellation has
one ordered commit-admission decision; completion may survive later cancellation.
General manager bind remains a separate explicitly privileged operation, not a
fallback when a picker fails validation. Shell cannot repair a stale picker by
reconstructing its target and silently performing a general bind.

If creation succeeds but binding fails, the created entry remains and the user can
select it in a fresh picker. No automatic replay or implicit rollback of creation.

### Channel ownership and retirement

Core sees attested principal/activation, never authoritative browser channel IDs.
Web runtime stores `(connection, channel, owner activation) -> native handles` in
its existing per-channel resources. Browser-facing references are channel-local
and resolved only through that map; a handle string from another channel is rejected
before native dispatch, even when both channels belong to the same component.
Native status/cancel/release independently enforce the operation's original
principal/activation. Prompt handles additionally retain their original requester
and its policy lease. No string named `channel` in guest payload changes ownership.

On disconnect, frame close, shell replacement or expiry, runtime retires the channel,
rejects queued/new calls and schedules native cancel then release under the original
owner's delegated task. Maintain a bounded cleanup list until dispatched; do not
drop handle accounting when cancellation is pending. The existing owner stop/trap
path must directly invalidate owned prompts/operations if that execution context
can no longer run. Do not impersonate a stopped owner with runtime-provider rights.
If the runtime itself stops/traps, hosted-owner teardown performs the same native
cleanup. Tests must prove this path, not rely on a final browser message.

Closing the requesting channel cancels its prompt; closing the shell channel
cancels its administrative operations and the modal. The runtime also cancels the
associated requester prompt through its owner task. No secret-management authority
is needed merely to close an owned resource. Permission revocation invalidates new
commits directly in native policy even if runtime cleanup is delayed.

Channel retirement is observed by runtime, so a native operation can commit before
its cancellation reaches Host. Do not promise instantaneous rollback at socket
closure. Cancellation linearizes at the native commit-admission check: operations
not yet admitted there are cancelled; admitted writes finish and are reconciled if
the reply is lost. `cancel` is idempotent while the handle exists; `release` discards
results, retains pending resource accounting, and never erases a committed write.

### Picker and administration

Root owns one open picker/manager modal; the ordinary frame only requests selection.
Create and bind are separate commits: after successful creation bind that returned
entry; if binding fails, retain the entry and let the user retry selection. Never
recreate the secret automatically. Replace warns about existing readers. Deleting
an entry leaves visibly missing bindings under the existing storage contract.

Values are write-only from the browser's perspective: no readback API. The form
holds only the new value in short-lived component memory. Never use localStorage,
sessionStorage, URL parameters, shared preference state, caches, analytics or error
serialization for values. Clear on dismissal/submit; do not promise browser-memory
or OS-clipboard erasure. Render labels as text, use existing restrictive CSP and
plain masked input with optional user reveal/multiline support. Metadata/errors
and returned operation records must not contain submitted bytes. Raw transport
payload logging must be disabled/redacted for these operations at every boundary.

Expose Manage Secrets from shell settings as an ordinary administrative section.
Extension contributions can request a picker but cannot replace the trusted form
or use their own DOM as the credential input. A replacement shell can provide its
own implementation only by assuming the same explicit administrative trust.

## Delete and retain

Delete production GTK/Python input code, its private JSON pipe protocol, process
watchers, GUI smoke and runtime package requirements after browser acceptance
passes. Remove native-window-specific Host orchestration/types/entrypoints that
have no remaining consumer; preserve generic target fencing, binding atomicity and
vault methods. Do not retain two GUI implementations or compatibility shims merely
because the discarded implementation was tested. Preserve useful regression cases
by exercising the new boundary, not retaining obsolete APIs for tests.

Keep keyring crate, SecretVault, exact bound grants, persisted local policy and
Linux safe file operations. These implement real requirements. Keep explicit
local-file selection and no plaintext fallback. Do not create encryption ourselves.
Reuse existing React modal/input components and bridge rather than another GUI library.

## Portability and completion

Keep this delivery Linux-scoped. No Windows/macOS backend work or broad cfg cleanup
is required to complete the Web UI flow. Common APIs must contain no GTK/Wayland
assumptions, but do not add speculative platform adapters. Describe existing support
accurately; a browser frontend does not prove cross-platform storage support.

First finish Linux end-to-end. Acceptance must include: AI Settings → create/select
in shell modal → bound backend can use sentinel → restart → rotation shared by two
consumers → revoke one → other still works → clone/export has no values/grants.
Test forged iframe management calls, cross-World/stale picker target, lost reply,
cancel, locked vault, partial durable failure, blocked frame teardown and output
size limits. Verify sentinel absent from ordinary iframe messages, browser local/session storage,
World DB, preferences, logs and exports. It MUST be persisted in the chosen secret
vault (plaintext in explicitly selected local-file), and is expected in trusted
submission buffers and an authorized backend read. Do not prohibit the intended
vault persistence or mislabel full-home backups as secret-free.
No real user keys needed. Preserve source hashes/commands/raw logs. Existing native
Wayland smoke is historical evidence, not acceptance of this new browser workflow.


## Discriminating authorization checks

Prove ordinary frame denial even when frontend checks are bypassed; selected shell
without the capability is denied; approved shell succeeds; runtime-provider identity
cannot borrow shell authority; foreign/stale activation cannot read operation status;
revoke while vault work is queued prevents a later unadmitted commit; switch shell
or detach picker before submission rejects its old context. Test arbitrary target
JSON, wrong sibling, undeclared slot, stopped World and changed artifact. Exercise
both real hosted WASM and browser routing, not only native mock methods.

## Why this is the minimal change

Keeping GTK preserves unwanted product complexity. Putting the form inside the
ordinary extension iframe exposes the user's shared secret to that extension's UI.
A shell-owned form reuses the existing UI and administrative bridge, adding only
a privileged generic administration boundary. Reusing runtime-policy-write for
credential CRUD would silently widen an existing permission; one explicit
secret-manage capability keeps its authority understandable. Existing entry/binding
storage remains; no new global registry, event bus or portability framework.

Additional acceptance: iframe with secret-prompt but no secret-manage can request a
picker, but cannot list/create/replace directly; iframe without secret-prompt is
denied before notification. Forge requester JSON, swap prompt IDs, copy operation
references across two channels of the same shell, close requester before confirm,
revoke/regrant prompt permission, stop either owner/runtime and lose cleanup messages.
Verify no stale bind, no orphaned unbounded records and no lost accounting; explicitly
exercise a commit winning the cancellation race without claiming rollback.

Lifecycle acceptance: revoke secret-manage with pending and terminal operations;
begin/status fail while original-owner cancel/release succeed and disclose no
result. Foreign/restarted owners cannot clean up someone else's handles. Stop/trap
cleans up without invoking the retired owner. Check runtime-owned cleanup after
shell deselection independently from ordinary browser shell-only dispatch.

## Implemented wire and validation — 2026-11-25

`Client.requestSecret(componentId, slot)` uses `secrets.request-selection` and
receives only its terminal outcome. The runtime retains the native prompt on its
requester channel and notifies that connection's shell with an opaque reference.
Administrative begin takes a strict JSON object tagged by `operation`:
`list`/`consumers` with `offset`; `create` with `label,value`; `rename` with
`id,revision,label`; `replace` with `id,revision,value`; `delete` with
`id,revision`; `bind` with `target,slot,id`; `disconnect` with the exact displayed
`grant`; `inspect-prompt` with `prompt`; `bind-prompt` with `prompt,id`.
List results contain `items,nextOffset,total`; entry metadata is `id,label,revision`.
Prompt references are translated only in their owning shell connection/channel.
There is no value-read operation. Errors are sanitized kebab-case codes.

Cancellation, write-only results, retention during blocked I/O, permission and
activation checks are tested in Core. Native metadata contention remains pending
or unavailable; it is never misclassified as stale. Workers may wait for a busy
binding fence only before the persistence callback; a callback that ran is never
replayed. The UI retries only safe reads, clears submitted values, and requires
state reconciliation after unknown mutation outcomes.

Both repository check scripts passed; final commands, revisions, source hashes
and raw logs are under `/tmp/rintawa-web-secrets-20261125/`. The real Chromium
acceptance script is `rtwKit/scripts/shared-secrets-smoke.mjs`; its final run passed
eight scenarios, with no browser exceptions/resource failures, using synthetic
secrets. A separate real-RTW template test covers World lifecycle/export/fork and
exact Host provider identity. Production GTK/Python code and native-window APIs
were removed after the initial six-scenario browser pass. No portability parity
or real-user system-vault mutation is claimed.

Final raw browser evidence: `/tmp/rtwkit-secrets-WXAG7V/`; orchestration log
`/tmp/rintawa-web-secrets-20261125/browser-release.log`. The oversized-input case
asserts no native begin occurred and shows the required stdin guidance. The
completed generic picker is titled Select credential; API key wording belongs
only to AI Provider. Legacy direct approvals are labelled separately from missing
managed entries in the global consumer list.
