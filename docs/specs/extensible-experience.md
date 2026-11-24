# A responsive, composable Rintawa experience

Status: the owner authorized the prerequisite freshness, layout/theme and message-action
slices on 2026-11-24. Composer actions, context sources and later product features
remain proposals. Existing Core contracts remain accepted and
unchanged. This document requires owner approval before implementing its new
cross-extension behavior. It does not introduce a Core UI subsystem.

## Purpose

A user opens a World, continues a story, changes settings and installs a useful
extension without manually synchronizing its panels. Extension authors can add
actions, context and their own interfaces without becoming part of another
extension's trusted implementation.

This builds on [live permissions and UI contracts](live-permissions-and-ui-contracts.md)
and [browser layout identity](browser-layout-amendment.md). It specifies the next
behavior, rather than another implementation checklist for the original MVP.

## What already exists

The rtwKit shell uses DockView, stable view restoration, a global theme and
Root-owned isolated nested viewports. Settings sections and Chat composer actions
use sourced transport replies. A replacement Settings renderer can consume the
same sections. A component does not inherit its container's native permissions.

The bridge already exposes publish/subscribe/poll/unsubscribe. These notifications
are transient: the Host may drop old messages for a slow subscriber. The current
UI mixes explicit reads, polling and Refresh buttons. Settings sections and Chat
actions are view contributions; they are not yet a general contextual action API.

These facts are grounded in rtwKit `web/bridge/src/index.ts`,
`web/ui/src/contributions.ts`, `packages/settings-ui/src/Settings.tsx`,
`packages/chat/src/ComposerActions.tsx` and `packages/web-ui/src/App.tsx`.

## 1. Freshness without a durable event system

### Contract

Queries remain authoritative. A notification means only “reread this resource.”
It never carries permission, proves a commit, or supplies authoritative state.
Publish after a successful commit; a failed publish must not turn a successful
mutation into a failed mutation. A caller rereads after its own completed write.

Introduce owner-defined points `rintawa/chat/changed/v1`,
`rintawa/character/changed/v1` and `rintawa/ai/connections-changed/v1` alongside
their existing query APIs. Payload is `{v:1, resource:string|null}`: an item UUID
when applicable, otherwise null for the owner's catalog. UTF-8 JSON is at most
256 bytes. Unknown fields/invalid payloads are ignored. Scope is the native
transport scope, never an authority claim in the body. Consumers coalesce hints;
they do not allocate a cache entry for every received resource string.

The consuming extension subscribes before its initial read. A hint arriving while
a read is in flight marks it dirty and causes one follow-up read. At most one read
is in flight per observed resource; late results from a retired view or different
selection cannot replace current state. Release subscriptions on unmount/stop.

Reconcile on reconnect, focus/visibility return and successful local mutations.
Because a final notification can be lost with no later notification, also reconcile
visible catalogs/transcripts at a configurable interval, initially 5 seconds.
This is a freshness bound while queries succeed, not a network latency guarantee.
Use jitter/backoff on errors. Hidden panels stop periodic reads and reconcile
before becoming current again. Keep the previous data with an explicit stale or
unavailable state after failed reads; do not represent failure as an empty list.

Existing polled transport may require a short receive loop. Share its lifecycle
helper in the browser SDK, but do not build another broker, data store or framework.
Publishers do not wait for subscriber acknowledgements.

Host-owned catalogs and baseline preferences need special care: World-scoped
transport does not magically broadcast baseline changes into other Worlds.
Initially their consumers use the same reconciliation rule. A World AI editor
can hint its own scope after saving; other scopes discover the canonical change
on reconciliation. Do not add cross-scope transport merely to remove this polling.

### Alternatives and acceptance

Polling alone is adequate for small catalogs, but constantly rereading transcripts
is unnecessary. Notifications alone cannot recover a lost final event. A durable
event log adds ordering/replay/storage obligations absent from this requirement.
Hints plus authoritative reads solve the present problem with existing primitives.

Acceptance: two Chat panels update after either writes; an importer updates a
visible character picker; drop the only hint and the other panel still converges;
reorder read completions while switching conversations; disconnect during a write;
stop one World while another remains active; flood hints without unbounded work.
An unknown mutation result still requires reconciliation, never automatic replay.
Remove ordinary Refresh controls once these cases pass; retain Retry for failures.

## 2. Contributions are contracts, not remote DOM patches

Settings remains a selected renderer plus independently owned sections. Provider
selection chooses the renderer; contribution order sorts entries. An extension
cannot become authoritative by declaring a larger priority number.

Keep current view contributions for self-contained interfaces. Add contextual
actions only where required: message actions (TTS, image generation), composer
actions (attachments/tools), and Narrator context sources. Do not unify these
different semantics into a generic interception middleware.

### Contextual actions

The Chat owner defines `rintawa/chat/message-actions/v1`. Discovery uses sourced
queries with `{method:"describe"}`. Each provider returns
`{actionPoint,actions:[{id,label,order}]}` with at most 16 entries. Use the existing contribution
limits for IDs, labels and ordering. IDs are unique within one provider; duplicate
IDs invalidate that provider's response. Entries are keyed by attested provider
identity plus local ID, not by an arbitrary claimed package/view route.

Invocation uses `{method:"invoke",actionId,context}` on `actionPoint`, a valid
transport name pinned to the discovered provider by the existing Host
`authoritative_providers` policy. Installation/composition sets that pin. Without
the pin the action is unavailable. A string containing an instance ID is not a
private endpoint. Verify the provider's current activation and response source;
response verification alone is insufficient because side effects happen before
the response. Never fan out an invocation to discoverable providers. Discovery
remains Multiple; execution has exactly one authority per action point.

Context is inert `{conversationId,messageId,expectedRevision}` for message actions.
`expectedRevision` is the Chat message revision defined by the narrative contract.
The recipient resolves authorized data itself or requests a bounded public excerpt
from Chat. It receives neither a native capability nor general Chat write access.
Invocation accepts only the selected Chat UI principal; providers that support
other callers must explicitly define their policy. Forged IDs, stale revisions and
cross-World references reject before work starts.

Use the existing admission/status/release convention for asynchronous work. A
provider may return a completed result immediately; opening a confirmation/editor
uses its own view and inert context via existing navigation/mounting. A UI timeout
does not prove an action was not admitted. Do not silently repeat synthesis,
generation, purchase, file writes or other side effects.

The initial message-action consumer uses the existing domain response envelope
`{ok:true,value:...}` or `{ok:false,error:{code:...}}`. `invoke` returns
`{status:"completed",message?:string}` or `{status:"pending",operation:string}`;
`status` takes that same opaque 43-character operation token and may also return
`{status:"failed",reason:string}`. Tokens never change during observation.
The optional completion message is bounded to 1024 UTF-8 bytes, failure reason
to 2048. Cleanup uses `{method:"release",operation}`; releasing a pending result
does not cancel accepted work or let its provider exceed its admission bound.
The initial Chat observes for up to 30 seconds, polls status every 80 ms, and
releases retained results when observation ends or selection changes. An expired
observation leaves the effect unknown and never retries `invoke`. This is a UI
observation budget, not a guarantee that provider work stops at that time.
The owner-approved Native admission boundary is defined in
[contextual-action-admission.md](contextual-action-admission.md).

Composer actions use the same identity rules, but their context is conversation
and draft revision. Providers propose a result to the composer; Chat validates it
and asks the user before replacing a changed draft or sending a message. The
existing view-only composer point stays valid; contextual actions are a separate
contract, not a silent reinterpretation of its descriptor.

A TTS contribution can display its own controls inside a Chat-owned slot through
`mountView`. The Root remains responsible for viewport isolation, clipping and
cleanup. Never pass a DOM node, React component, native channel or executable
string across an extension boundary. A contributor may itself consume another
explicit point under the same rules.

Acceptance: replace Settings without changing section providers; mount a nested
contributor inside another contributor; stop either parent/provider during mount;
reject duplicate/spoofed descriptors; invoke Speak on the correct message while
switching chats; a contributor cannot use Chat's authority to modify another item.

## 3. Workspaces, resizing and appearance

DockView owns workspaces that need movable tabs, splits and independently restored
views. Both baseline and World views can participate, retaining exact scope in
their restoration keys. Never make World switching an implicit World shutdown.

Inside a view, use an ordinary accessible split layout for a chat list, inspector
or navigation sidebar. The shared UI package supplies the resize handle, keyboard
interaction, minimum sizes, collapse/restore and token styling. An extension may
use nested DockView when its contents really are rearrangeable documents; it is
not compulsory for every separator. These are browser libraries, not Core APIs.

Default story layout shows its content without redundant single-tab chrome. Tabs
and close controls appear where multiple documents actually need them. Every
hidden header still has a keyboard/context-menu route for switching, closing and
restoring panels. Closing a view does not delete its data or stop its World.

The shell owns outer layout. Each extension owns its internal pane sizes in its
existing preferences. Store logical pane IDs and bounded ratios, never DOM paths,
pixels tied to one monitor, or nested ephemeral view IDs. Clamp restored sizes to
the current viewport. Resize emits no World mutation. Dragging through isolated
frames must not leave a stuck pointer capture or expose another frame's contents.

Theme tokens cover text, muted text, surfaces, selection, hover, borders, focus,
disabled controls and accent contrast. The same token contract reaches nested
frames and DockView. Remove literal dark palette values from semantic UI states;
artwork may keep authored colors. No second theme store in each extension.

Acceptance: light/dark themes across shell, Chat, Settings, dialogs and nested
views; readable selected/disabled/error/focus states; narrow screen; keyboard
resize; refresh/restart at a different viewport; one hidden-header panel and two
tabbed panels; stopped/uninstalled extension leaves a recoverable placeholder.
Visual implementation and theme corrections belong to Sol, not a new Core design.

## 4. Permissions that survive restart

The accepted live policy contract already requires durable and effective success.
Keep one setup summary for requested application rights, with details expandable.
Do not ask again for unchanged approved rights. Missing permissions are explained
at the affected feature, with one action to review the missing set. A saved AI
configuration and permission to use its endpoint/key remain distinct internally.

Restarting the same home, World and installed packages must preserve decisions.
Copying/importing a World does not copy local trust. A fresh preview installation
is not a restart. Artifact update/trust policy is a separate security decision;
do not weaken it to hide an apparent persistence regression.

Before changing this model, distinguish persisted grants, effective grants,
provider readiness and the displayed UI state. Capture only scope/component IDs,
artifact digests and permission names, never key values. Compare before shutdown
and after launching `rintawa --home <same-directory> run`. No reinstall script in
this check. Test both allow and revoke, baseline and World, endpoint and secret
access, and a deliberately denied capability. Existing setup state must converge
without browser/World restart after a live permission change.

## Boundaries retained

No UI schemas, themes, Chat events, DockView state or action IDs enter Core.
No new World table, generic event store, registry or cross-world bus is required.
Notifications do not confer read access. A compact shared client helper is useful
because race handling and cleanup should not be reimplemented by every extension.
Public compatibility changes need explicit acceptance; layout/CSS fixes within
the current contracts can proceed independently.
