# Package and conversation foundation

Status: proposed architecture, 2026-11-25. This document changes the proposed
delivery order before [simulation experiences](narrative-and-companion.md).
It is not an implementation report or authorization to change accepted APIs.

The owner has selected two product behaviors:

- Explicit **Stop saves the validated text already received**.
- A transcript fork appears as **a separate chat in the chat list**. Editing that
  chat must not change the original, including its shared earlier messages.

The owner requested proceeding with implementation after reviewing this proposal.
Delivery starts with F1b: explicit Stop under the product behavior above, using
the existing Chat schema and native HTTP lifecycle. Unresolved alternatives in
other slices remain design decisions, not permission to silently pick a conflicting
contract. Existing accepted streaming/revision contracts remain the baseline.
A stopped World/activation is different from pressing Stop in Chat.

## 1. Current foundation and missing user paths

Core supplies storage, verified artifacts, scopes, capabilities and lifecycle.
Package policy belongs to an administrative extension; transcript semantics to
Chat; generation orchestration to Narrator; inference transport to AI Provider;
authored characters and player personas to Character. No simulation-specific
behavior is added to Core or AI Provider to implement these paths.

| Capability | Existing foundation | Missing complete behavior |
| --- | --- | --- |
| Package management | Immutable CAS, local RTW import, scoped selection/enable/remove, template creation; rtwKit catalog/solver library | Deployable manager, scope-aware UI, inventory/inspection access, safe version/removal workflow |
| Stop | Cumulative text; guarded retention after a known network failure | Explicit owner cancellation, prefix freeze/save, UI and cancellation race checks |
| Edit/delete | Conversation/message revisions and guarded owner writes | Owner operations, history semantics, independent forks, UI |
| Regenerate | Generate after the current user message; retry an empty failure | Replace an existing answer through a captured generation basis and independent fork |
| Message contributions | Directed backend action buttons; isolated nested view mounting | Message slot descriptors, captured context, bounded mounting and specific mutation admission |
| Alternate greetings | Card library parses and round-trips alternatives | Character exposes/persists choices; Chat initializes and switches them |
| Player persona | Character objects and conversation character binding | Authored player preset, selection, revision-aware prompt context and display |

Source map for the companion rtwKit checkout: `crates/package-manager/src/model.rs`
(`validate_registry`, `solve_install_plan`); `packages/chat/src/{model,wasm}.rs`
and `packages/chat/src/main.tsx`; `packages/narrator/src/{wasm,model}.rs`;
`packages/character/src/{wasm,model}.rs`; `web/ui/src/{actions,contributions}.ts`;
`web/bridge/src/index.ts`. The revision-specific audit records exact revisions.

Native entry points are in [Host](../../crates/host/src/lib.rs),
[composition profile](../../crates/host/src/profile.rs),
[template import](../../crates/host/src/templates/mod.rs),
[CAS](../../crates/artifacts/src/store.rs) and
[WIT](../../crates/extension-engine/wit/engine.wit).

## 2. Package manager: finish existing primitives before inventing a platform

### 2.1 Identity and state

Keep these independent dimensions visible to the implementation and understandable
in the UI:

| Value | Meaning |
| --- | --- |
| Artifact digest | Exact immutable RTW bytes; necessary even when a version label exists |
| Package identity/version | Publisher metadata where the content contract defines it; not a security principal |
| Content type | Descriptor format, currently extension or World template; opaque unknown content is not executable |
| Scope/instance | Where an extension is selected and which instance owns its data/permissions |
| Configured selection | Artifact saved for the next applicable activation |
| Effective activation | Artifact actually running now; may differ until restart |
| World template | Installable source for creating independent Worlds |
| World instance | Mutable simulation, managed by existing World lifecycle APIs |

Current outer RTW metadata does not define a universal package ID/semver. Do not
invent one by treating display names as identity. Current extension descriptors
have IDs/versions; template identity can initially be exact digest plus name.
Two templates with the same name remain distinct. A new template version creates
new instances; it does not overwrite an existing World's database.

### 2.2 Initial useful manager

Deliver one Host-scoped package-manager extension with an administrative view and
a narrowly authorized backend. Its first complete path is:

1. Import a local `.rtw` using existing bounded, disk-backed package transfer.
2. Inspect validated content, exact digest, declared identity/version where present,
   requested permissions and selected scopes. Import alone selects or runs nothing.
3. For an extension, choose the target scope, review the actual selection change,
   select it, then activate through existing lifecycle controls. For a template,
   offer creation of a new World through the existing template flow.
4. List available versions/artifacts and configured/effective state. Permit an
   explicit switch to another installed compatible artifact; never silently follow
   a moving `latest`. Show missing dependencies and unsupported content honestly.
5. Disable, remove a scoped selection, or remove an unused template/archive using
   distinct operations described below. Preserve World data by default.

Reuse the existing upload UI/helper rather than transporting a whole archive in a
single JSON request. Source repository build scripts are publishing tools, not
install hooks. Importing arbitrary content must not execute its code or acquire
the importing administrator's authority.

Existing `composition`/`scoped-composition` WIT handles selection. There is no
complete browser composition facade or bounded generic guest artifact catalog.
Use the manager's permitted backend for existing Host APIs. Add only the missing
bounded artifact list/descriptor inspection, guarded composition application and
the chosen removal operation at the native boundary; do not expose filesystem paths or broad Host authority to
every nested view. A native inventory page returns canonical digest hints, content, bounded
advisory metadata and explicit invalid entries, not a fully expanded archive.
The manual delivery boundary below defines its verification semantics.
Scope inspections distinguish configured selections from live instances.

List pagination must tolerate concurrent imports/removals without claiming a
snapshot; selection/removal revalidate exact identities at mutation time. Existing
native `digests()` enumeration is not evidence of a scalable guest catalog API.

### Initial manual delivery boundary

The first local manager uses individual existing composition operations and reports
configured state as effective on the next restart. It does not advertise resolved
or atomic dependency installation. Native bounded `artifact-store.list-rtw` exposes
advisory metadata pages only to a Host-scoped caller with composition-read. Each
row has an opaque key, optional canonical digest/content/name/version/subject and
a row-level error. Malformed names, unsafe files and unreadable archive headers
remain explicit rows. Catalog reads inspect bounded metadata without hashing whole
payloads; labels are unverified hints. Permission inspection and selection still
verify the exact bytes through their existing APIs, never trust a catalog claim. Initial inventory
caps one page at 32 artifacts and directory enumeration at 10,000 entries, rejecting
larger catalogs explicitly. This is an initial measured-work bound, not a permanent
platform capacity promise. It adds no catalog database. Declared identity is a hint until verified selection.
Synchronous exact inspection/selection retains its current transport deadline; a
large or slow artifact can produce an unconfirmed outcome requiring a fresh read.
This slice does not promise large-archive selection latency. Large ZIP directories
also have bounded-but-nonconstant catalog cost under the existing archive limits;
no hard 500 ms inventory latency is promised. Failure to refresh inventory must
not disable an otherwise admitted import path or erase a confirmed import result.

The manager is an optional separate RTW with an administrative backend and isolated
UI. Only its exact sibling UI may invoke mutations. Browser upload uses its backend's
existing disk-backed transfer capability; no child obtains the shell's channel.
Uninstall initially means removing a scoped selection while retaining bytes/data.
Physical archive deletion and guarded dependency batches remain separate increments.
World scope mutations require that World to be stopped; baseline changes are pending
Host restart. Read/mutation errors and unknown outcomes remain visible without replay.

### 2.3 Versions, dependencies and activation

The existing Rust package-manager library is reusable, not a finished installer.
Its registry currently accepts extension content only, its solver receives installed
semvers rather than exact digests, and it solves the requested dependency closure.
Do not infer that it validates every reverse dependency in the selected scope.

For a change plan, capture target scope, current selections, exact candidate
digests, metadata and dependency constraints. Check both the candidate's closure
and dependents whose requirements the change would break. A same-version artifact
with different bytes is a distinct candidate, never an automatic no-op or trusted
replacement. Catalog claims must agree with the imported artifact's descriptor.
Without verified version/dependency metadata, label a local artifact as such;
do not promise a resolved semver installation.

Current selectors atomically save an individual profile change; a dependency batch
is not automatically atomic and a read followed by selection is not a stale-plan
guard. There are two viable scopes: an initial manual configuration UI that reports
partial changes honestly, or a guarded installation plan. The latter is recommended
for version/dependency management because undoing sequential mutations in the
extension duplicates concurrency and recovery machinery.

The narrow native amendment is compare-and-apply for one composition scope:
the manager supplies the expected scoped selection state and bounded exact-digest
changes. Under the existing composition exclusion, Host compares the relevant
state, validates the resulting profile and saves it once, or changes nothing.
This must cover the complete selection set on which dependency checks depended;
checking only the root package misses concurrently added reverse dependents.
It is not a semver solver in Core, a global version service or a transaction across
World databases. Preserve current instance/grant filtering rules. Import all bytes
before applying the plan; failed application may leave cached bytes, not a partly
selected dependency set. Cross-scope changes are separate results. A later explicit
permission approval is separate from the selection commit and does not make an
unapproved package executable. Without this amendment, the manual UI must not claim
atomic plans or protection from concurrent administrators.

Initial switching of World extensions is a stopped-World operation. Baseline
changes may require Host restart. Show pending activation explicitly. Host selection
does not unload running code; configuration removal is not live teardown.
Default-World recipes affect future Worlds, not existing pinned Worlds.

Replacing the same subject preserves its instance in current selection code;
removing and reinstalling is not a substitute for replacement. Property ownership
and protected schema data must remain readable by the intended instance. Current
permission selection keeps still-requested grants and filters obsolete ones. Do
not silently redefine that accepted policy as granting every new request or
revoking every existing approval merely because the digest changed.

Code rollback is not database rollback. An artifact with incompatible schemas or
domain semantics cannot be safely selected just because its semver is older/newer.
Before shipping Chat's new history schema, specify its owner conversion and which
artifact versions can read the converted World. A pre-upgrade template snapshot
is a separate recoverable World, not an invisible reversal of live state. Do not
build a universal migration framework into the first manager.

### 2.4 Removal: concrete semantics and the remaining native gap

- **Disable:** retain instance/configuration/data; stop using it after the explicit
  lifecycle transition. Suitable for temporarily turning off an extension.
- **Remove from scope:** remove its composition selection and associated grants as
  current Host does; preserve domain data and cached bytes. Show dependencies and
  ownership consequences. Existing authoritative point pins remain fail-closed;
  do not silently select another provider. Rebinding is an explicit action.
- **Delete World:** use existing World lifecycle deletion, separately from packages.
- **Delete cached artifact/template:** remove these exact bytes only when safety can
  be established. No recursive dependency deletion and no deletion of World data.
- **Purge extension data:** outside initial scope; requires its owner's semantic
  cleanup, not blind deletion of protected properties or cascading foreign data.

There is currently no artifact-delete API. Template listing scans CAS, so hiding
a row in manager preferences would leave it installed in the shell's template
picker. That is not an implemented uninstall.

Recommended minimal amendment: an explicit native removal operation for an exact
unused artifact. Recheck under the same exclusion as composition/start/import:
all saved scopes (including stopped Worlds/default recipes), active/pending uses,
and retained known packages' declared exact dependencies. Reject `in-use` with
bounded reasons; reject uncertain reachability rather than infer no references.
Do not count a mere provenance label as a runtime dependency. Do not physically
remove dependencies merely because their parent template is removed. The caller
must see when bytes could not be deleted.

Prove the reference rules against actual template/profile descriptors before this
amendment is accepted. The initial supported removable set can be unused World
templates; extension cache reclamation can remain unavailable until all retention
roots are known. This does not block scoped extension uninstall. Unknown custom
content must not enable unsafe reclamation. There is no automatic cache GC.

Alternative: a separate installed-library membership catalog would permit removal
while retaining bytes, but changes template-list semantics and adds authoritative
durable state. Do not introduce that second source of truth just for a hidden row.
If chosen later, it must replace the CAS-derived catalog everywhere, not coexist
as conflicting manager-only preferences. Archive removal and this alternative are
mutually exclusive first-slice designs, not two required layers.

### 2.5 Deferred distribution and custom content

Repository fetching, convenient updates, trust policy and custom content handlers
are later increments. Preserve exact selection and inspectable provenance now.
Reuse catalog/solver code when adding distribution; explicitly amend its
extension-only content validation when supporting template catalogs.

A future theme/preset package does not become a runnable extension merely because
it is packaged. A handler belongs to an installed, explicitly admitted extension;
the manager passes a verified digest through a bounded owner API. The handler owns
format validation, resulting domain data and uninstall semantics. Unknown types
can remain stored/inspectable without installation side effects. No Core registry
of themes, lore, voices, personas or package-kind scripts is required now.

## 3. Explicit Stop with retained text

Current UI AbortController/release detach observation; pending release continues
work and holds its quota. They are not Stop. Keep these meanings separate.

Add an owner-bound Narrator stop operation and a generic AI cancellation operation.
The following are semantic requirements, not already existing wire methods:

1. Chat addresses the exact Narrator operation and captured caller/activation. The
   request contains no replacement text. Wrong caller or retired operation fails.
2. Narrator serializes Stop against its own progress. At acceptance it freezes its
   last validated cumulative prefix and stops admitting later provider text.
   This may include validated text not yet displayed by the next browser poll.
3. Cancel/retire provider work; save a nonempty frozen prefix through ordinary
   guarded Chat append with the already allocated message identity and captured
   conversation/input revisions. Whitespace-only/empty output creates no message.
4. Show `stopping`/`saving` until the result is known. Only Chat COMMIT is saved
   history. Stale context remains a conflict, not permission to refresh revisions
   and force the old answer into a different history.
5. If append was already submitted, reconcile that operation/known identity. Do
   not submit a second append or pretend to retract an admitted transaction.
   Stop after completion returns the completed result. Repeated Stop is idempotent
   while the operation is retained; after release, use canonical Chat reads.

Before the first prefix, Stop means no assistant message. During provider failure,
the existing validated-prefix retention and Stop converge on one save decision.
Malformed/retracted snapshots remain invalid, even if the user presses Stop.
Unknown begin/write admission is reconciled without replay. Closing a panel,
switching chats or stopping a World is not a new command to save a prefix.
Retired activation work cannot initiate writes. Already admitted writes are
reported by their actual outcome. None of this creates durable generation jobs.

AI Provider owns cancellation of inference transport, not persistence of prose.
It returns a terminal cancelled result and stops publishing new text. Narrator
owns the explicit decision to retain its frozen prefix. Partial structured output
never becomes an executable action. Future speech/image callers may choose a
different interruption policy without changing AI Provider.

Dropping the current HTTP handle sets a flag, but a silent blocking response read
may retain a native worker until data/timeout. Saving text must not wait indefinitely
for that socket. Still charge retiring resources to the relevant native quota;
release cannot allow unbounded abandoned workers. Test silent upstream explicitly.
A measured bounded cancellation fix belongs in shared HTTP resource lifecycle,
not in each narrator. Do not promise immediate remote cancellation or refunded billing.

## 4. Independent chats, edits, deletion and regeneration

### 4.1 Observable history rules

A chat is a durable transcript identity listed independently, with its own title,
revision, head/history selection, character/persona binding and optional origin
reference. A fork is created atomically as a visible chat, not merely as a browser
tab or an alternate head hidden in the original chat's mutable metadata.

| Operation | Required result |
| --- | --- |
| Fork at a message | New chat containing the prefix through that message; original and its continuation remain available |
| Edit an old message | New chat containing the prefix before the target and a replacement message; old suffix stays in the original; no automatic reuse of causally stale replies |
| Regenerate an assistant reply | New chat based on the prefix before that answer and the captured input; generated answer belongs only to that new chat |
| Select another greeting/answer | Select or create the corresponding separate chat and its own continuation; never attach the old answer's suffix to the new answer |
| Edit an unsent draft | Local composer change; creates no chat or persisted message |
| Delete a sent message | Remove only the chosen entry from the resulting transcript; preserve a separate pre-change chat; later retained messages are not silently regenerated |
| Delete from here | Explicitly separate action: fork to the prefix before the target; preserve the original continuation |

Thus deleting one message and discarding a continuation are not synonyms. The UI
must distinguish them. Single-message deletion is a transcript correction and can
leave causally awkward later replies; it must say that they are retained. A user
can choose the explicit truncation action instead. These recommended deletion
semantics, unlike fork independence, have not yet been separately owner-approved.

For editing even a tail message, use the same independent-history rule rather than
mutating a node that may be shared with another chat. UI may group related chats
or display a variant counter, but every persistent branch remains individually
listed/selectable. Repeated read/preview/arrow discovery creates no chats.
The first actual choice creates a new branch once; revisiting a recorded variant
selects its existing chat. Concurrent choices use known target IDs and revisions
so retries cannot silently create duplicate chats.

An explicit fork copies the prefix, not the source's future appends. Fork metadata
is a non-owning reference: deleting the source chat must not delete children or
make their prefix unreadable. New titles need not be unique. Stable IDs identify
chats, messages and variants; a card array index or translated title does not.

### 4.2 F2 implementation boundary: independent memberships

For the current MVP use the existing immutable `rintawa.chat.message@1` bodies
and give each new chat fresh `rintawa.chat.entry@1` relations. Preserve all three
existing property schemas exactly. No Core changes, conversation migration,
predecessor index, balanced history tree, or durable preparation records are needed.
This chooses the eager independent membership alternative after source review;
the predecessor design above was a proposal, not an accepted storage contract.

A fork shares message bodies, never mutable membership or conversation metadata.
Editing creates a new body with the target's role. Every later edit also forks;
there is no in-place text mutation, including at the tail. New entry IDs are sorted
before assignment so existing UUID-ordered page/recent reads remain authoritative.
Deleting the source conversation cannot cascade through its relations to the
shared message objects. Initial retention keeps bodies; there is no message GC or
promise of physical erasure. Transcript changes never roll back World state.

Add an owner-UI-only `begin-fork` operation with caller-known result conversation
ID (and replacement message ID for editing), source revision/head, exact target
entry/message/revision, title and action (`fork`, `edit`, `delete`, `truncate`,
`regenerate`). Preparation reads and allocates fresh IDs in bounded batches between
runtime ticks, without persistent writes. The authoritative System rereads the
source on its pinned snapshot, validates the captured context and exact target
membership, and builds the entire new chat atomically. The source is never changed.
A regenerate target must be an assistant answer immediately preceded by a user
message; publish the prefix first, then invoke the existing guarded Narrator path.
Failed/empty inference leaves the new prefix available for an explicit retry.

Fork work is O(number of retained entries), without copying message text. Derive
the preparation entry budget from serialized membership bytes and the existing
1 MiB transaction output budget, reserving 64 KiB for bounded metadata/replacement.
Check the actual encoded transaction again. `history-too-large`, malformed data,
ID collisions and stale context publish nothing. This is a bounded MVP workload,
not a claim of unbounded cheap forks. Measure the supported limit in real WASM;
a persistent tree is a later option only if actual accepted workloads need it.
Preparation is transient: World stop before submission leaves no new chat; already
submitted writes use ordinary known-ID reconciliation, with no automatic replay.

New membership-sensitive reads can supply `entryId`: load that exact tagged
relation and verify conversation/message endpoints. Existing `get-message` requests
without it retain their behavior, with paginated rather than first-32 incoming
lookup; that older lookup is linear in the number of referencing chats and has the
normal transport deadline. The new fork commands always use direct membership.
No action-context format is changed in F2; contribution improvements remain F3.

Acceptance: source/fork independence after shared-prefix editing, all five actions,
assistant regeneration and Stop in the new chat, stale target/source rejection,
forged/non-member IDs, ID collision, overflow with no visible partial chat,
more than 32 referencing chats, existing saved @1 chats, paging, restart and
portable World roundtrip. Preserve raw commands, outcomes and artifact hashes.

#### Measured System budget amendment — owner accepted

Real Host evidence: 64 retained entries committed (278 ms); 128 failed (365 ms)
with `all fuel consumed by WebAssembly`. All 128 source appends had committed;
the failed fork changed no source and published no chat. Native unit construction
at 3,233 entries fits the byte cap, but is not evidence of the current WASM compute
capacity. Evidence: `/tmp/rtw-f2-evidence/budget/{result.json,run.log,context.txt}`.

`WasmWorldSystemEndpoint::evaluate` currently uses the 10,000,000 lifecycle callback
fuel allowance. Owner accepted the narrow amendment: use the already existing bounded request
allowance `fuel_per_service_request` (default 500,000,000) for command evaluation.
Do not change registration/start/stop fuel, runtime-task fuel, memory, property
ownership, atomicity, request size or transport deadlines. No new Core API or DB
schema. Candidate diff: `/tmp/rtw-f2-evidence/world-system-budget-proposal.patch`.
This permits more compute while holding the World's serialized writer; it does
not promise unlimited commands or a latency guarantee. Acceptance must measure
large actual WASM commands plus out-of-fuel atomic rollback and a subsequent
small successful command with deliberately small configured fuel. The implementation is verified on the default Host budget. A final-package
3,233-entry prefix fork committed in 1,956 ms; a 3,234-entry fork returned
`history-too-large` in 347 ms, with unchanged source and no new chat. Both complete
histories were read through 102 forward pages and compared by body IDs/order;
new entry IDs were unique, and foreign entry membership was rejected. These are
local measurements, not a latency guarantee. The 3,233 copied-entry bound comes
from 304 serialized bytes per membership pair plus the 64 KiB overhead reserve;
it does not limit conversation length or prevent branching a short early prefix.

Final Core and rtwKit repository checks passed. Browser acceptance passed 11 Chat
and 13 streaming checks, covering independent editing, deletion/truncation,
regeneration, explicit Generate reply after a user edit, Stop, more than 32 shared
references, isolation and restart. An exported template preserved the two-chat
graph, all item/property/schema rows and its asset exactly. Existing @1 data was
also reopened under the final package without migration. Durable private evidence:
`.vscode/research/chat-history-2026-11-25/`. Source-chat physical deletion and
contribution/persona increments are not implemented by F2.

### 4.3 Command admission and generation basis

Each F2 history transformation binds to exact conversation ID/revision, target message identity/
revision, intended prefix/head and caller-known result IDs. It validates inside
Chat's authoritative transaction. Stale input rejects with no partial visible
branch; operation tokens remain transient, and lost outcomes are reconciled via
known IDs without repeating billable work. A guessed ID is not membership proof.

Chat remains the sole owner of transcript mutations. Narrator obtains an immutable
generation basis: target chat, validated history boundary, triggering input,
character/persona revisions and future assistant identity. Existing Narrator's
`current_user` check assumes the head is a user message; calling its existing
generate method on a stored assistant is not regeneration. Fork to the correct
prefix, then use a validated generation path. Do not remove the head check and
allow an arbitrary caller-supplied prompt to become a Chat write.

Commit the new user-visible chat/prefix before starting inference; show generation
in that chat. Failed/empty inference leaves that prefix and an explicit retry,
while the original answer remains untouched. A retained Stop prefix belongs to
the new chat. Current one-pending-generation-per-conversation admission still
applies; no global lock across independent chats is needed.

History writes must not race a pending generation unnoticed. Reject incompatible
mutations as busy or retire/stop work explicitly and then perform the change;
choose and document one UI flow. Revision guards remain authoritative even if UI
buttons are disabled. Actions already admitted report actual outcomes.

### 4.4 Compatibility and simulation boundary

Current strict Chat schema/DTOs and membership relations do not implement this
model. Do not modify an immutable registered schema in place. Introduce the
necessary owner schema version and explicit conversion of deployed MVP data,
including old IDs/revision meaning and action callers; validate conversion,
restart and template export/import before switching readers. This is conversion
of the active product format, not compatibility for retired experimental systems.

A chat fork is **not a fork of World state**. It does not duplicate actors, rewind
inventory/clocks, reroll actions or retract sounds. Branch history can describe
another imagined continuation while all chats still address the same simulation.
Full independent simulation uses a separate World snapshot/instance. UI must not
present transcript branching as world rollback.

Memory/source adapters bind evidence to conversation identity and history version.
A new chat does not silently inherit the old chat's processed checkpoint, private
access or derived recollections. Shared text can be reused as input only after the
new perspective/source rules allow it. Background extraction should be scoped to
selected sources, not indiscriminately treat every alternative in the chat list
as something that happened in the same World. A later explicit memory bootstrap
can reuse a verified shared prefix; it is not required for initial Chat forks.

## 5. Message contributions and greeting choices

Keep three distinct contracts:

1. Existing message action buttons: directed backend commands with captured
   conversation/message/revision and native provider identity.
2. New message view slots: isolated extension-owned UI mounted in explicit Chat
   regions, initially a bounded accessory area. Arbitrary UI inside a slot is
   allowed; arbitrary access to Chat DOM or React objects is not.
3. Chat history operations: authoritative mutation APIs, never implicit privilege
   obtained from displaying a widget near a message.

A view contribution descriptor contains a stable contribution ID, label/order,
supported slot and a discovered native view identity. It contains neither script
nor trusted arbitrary iframe URL. Capture conversation/message/revision/history
context as inert initial state; exclude full private message text from discovery.
The view requests permitted data from its owner/Chat using its own identity.
Replacing or stopping the provider invalidates the binding; no fallback to another
same-named provider without rediscovery/admission.

Reuse `Client.mountView` and existing root-owned isolated sibling iframe handling.
For the first slice, remount when captured context changes rather than inventing a
generic live context bridge. Unmount on recycled row, provider stop and view close;
late results cannot affect a different row. Bound mounted views to visible/expanded
rows and enforce descriptor/payload/mount budgets. Test long histories, nested
layout, scrolling/clipping, keyboard focus and cleanup. One permanently mounted
iframe per historical message is not an acceptable implementation.

Existing composer contributions open views; they do not yet provide a revisioned
draft/result protocol. Do not call that protocol implemented or make arbitrary
message widgets depend on it. Add a separate bounded composer amendment only for
a concrete insert-into-draft interaction.

### F3a concrete message-view contract — owner accepted

This amendment implements the first message-view usage scenario: an independent
extension displays controls/details beneath a particular transcript entry. No Core,
bridge, database or existing message-action contract changes are proposed.

- Discovery point: `rintawa/chat/message-views/v1`. Request `{"method":"describe"}`;
  reply is the existing strict view descriptor `{id,label,order}`. Resolve the view
  through `viewContributions`: exact native responder instance/component/activation,
  exactly one admitted same-World view. No descriptor URL, DOM, channel or arbitrary
  target identity. This point has one accessory region beneath message text;
  content inside the isolated region is extension-owned arbitrary UI.
- Initial state: five string fields `conversationId`, `entryId`, `messageId`,
  `expectedRevision`, `conversationRevision`, within existing ViewState limits.
  The widget reads `client.initialState`, not persisted `ui.state`. Initial state
  is caller-provided context, never authorization. No message text is disclosed
  through discovery or automatic mounting. A widget can explicitly read Chat using
  `get-message {conversationId,entryId,messageId,expectedRevision}` under its own
  identity; the exact tagged relation establishes current membership.
- Chat displays inert contribution buttons. Mount only after an explicit click;
  at most four expanded widgets across visible rows per Chat panel. At capacity,
  ask the user to close one; do not silently evict another widget. This is a local
  UI resource budget, not a restriction on extension content or a new Host API.
  Reuse the existing bounded descriptor validator; report malformed/ambiguous
  contributions, and show discovery/read errors without clearing confirmed history.
- Each expansion captures the complete context. A changed transcript selection or
  revision, a removed/recycled/offscreen row, explicit close, provider replacement
  or retirement closes the slot. Reopening is explicit. Provider replacement never
  remounts a same-labelled view automatically. Mount completion after close must
  immediately release the late binding. Separate failures stay local to that widget.
- Use `Client.mountView` and its root-owned sibling iframe, clipping and ancestor
  validation. Chat supplies a resizable accessory container and a visible Close
  control. The child gets neither parent DOM/channel nor Chat mutation authority.
  Persistent widget preferences belong to the extension; captured message context
  must never be restored from those preferences for a different row.
- Existing `message-actions/v1` remains unchanged. Its older three-field context
  continues to work; F3a widgets use the direct entryId read path. Greeting/variant
  controls and any contributor-initiated history mutation need their own explicit
  domain amendment after this slice. A widget near a message acquires no write grant.

Acceptance uses an ordinary separately packaged fixture view: exact captured entry
and revision; explicit body fetch; same body in two chats; forged entry rejected;
collapse/scroll/revision/selection teardown; late mount acknowledgement; provider
stop/replacement; maximum four mounts; keyboard focus and clipped layout at narrow
width; restart does not auto-mount old message context. Existing Chat/streaming
acceptance must continue to pass. Save command, source/artifact hashes and raw logs.

#### Verified inner-scroll clipping gap — amendment owner accepted

F3a implementation and repository checks are present. The initial real browser fixture confirmed
that existing mountView clips against Root-owned frame ancestors, but cannot see
Chat's internal transcript overflow. With Chat frame top 48 and transcript local
top 58, the transcript starts at Root y=106; the mounted child starts at y=60.546875,
covering 45.453125 px of the Chat header while its message row is still visible.
Raw evidence: `/tmp/rtwkit-message-views-YiDOaN/clipping-geometry.json` and
`clipping-probe.png`. Hiding widgets whenever any part crosses an edge would be
an inconvenient workaround and would not implement ordinary scroll clipping.

Owner accepted the correction at the existing shared web boundary:

1. Keep the public `Client.mountView(view, element, state, onClosed)` signature.
   The Client measures the intersection of viewport and clipping DOM ancestors of
   element. Preserve the full intended element rectangle, so partial visibility
   does not resize/reflow child content. Observe relevant ancestor sizes as well
   as the element, coalescing existing scroll/resize updates per animation frame.
   This also updates sibling positions when a preceding accessory is resized.
2. Extend the Root-owned JS `ui.slot.mount/update` wire with optional `clip`, using
   the same strict local-coordinate rectangle shape `{x,y,width,height}` and existing
   numeric bounds. Existing callers omitting clip retain current behavior. New SDK
   sends it automatically; widget authors need no new API or authority.
3. Root intersects that additional rectangle with its existing trusted frame,
   viewport and ancestor clipping. The untrusted caller can narrow visibility only;
   it cannot expand the Root boundary or move content over privileged consent.
   Keep the iframe's full intended size and offset within the clipped outer box.
   Native Core/WIT, database, provider identity, channels and permissions do not change.
4. Package the updated web-ui Root alongside Chat. An older strict Root rejects the
   new field explicitly; do not silently fall back to drawing over the Chat header.
   No compatibility framework or migration is proposed.

Acceptance: the captured 45 px overlap becomes zero; partial clipping preserves
child content offset and full viewport dimensions; clip cannot escape the parent
or consent; resizing one accessory moves subsequent ones correctly. Preserve old
callers without clip, strict malformed-field rejection, clipping at 400 px width,
late close/retirement and all F3a plus existing Chat/streaming regressions. Acceptance includes this gap and the final cleanup regression; final evidence is
recorded in `.vscode/research/message-views-2026-11-25/`.

F3a final verification: complete rtwKit checks passed; 17 real-browser message-view
checks passed, plus three repeated cleanup cycles with no remaining Root slots.
A controlled delay of a real Native ui.mount response verified close-before-ACK
cleanup. Existing Chat (11) and streaming (13) regressions passed on final packages.
The shared SDK also has a deterministic red/green regression for transient close
admission failure with concurrent callers. No RPC quotas were raised and no domain
mutation was replayed. Final packages and exact commands/source hashes are recorded
in `.vscode/research/message-views-2026-11-25/README.md`. Original intermittent quota
observations are retained without claiming that one cause explains all of them.

### Greeting and variant example

Character exposes a bounded ordered list of greeting candidates with stable IDs,
captured card revision and text validation result. The library already preserves
Tavern alternatives, but the runtime currently persists only the primary greeting.
Persist enough normalized candidate data to survive restart/re-export; do not
silently depend on reparsing a missing import file. A card edit creates a new
candidate revision; it never rewrites a greeting already stored in a chat.

Chat can initially render generic previous/next variant controls using that data.
Resolve macros against the captured character and player persona, and store the
result as normal text. Before a chat is created, changing a greeting is a draft
choice. Once persisted, selecting another greeting follows the independent-chat
rules above. If that variant already has a continuation, select its chat intact.
An old card revision missing from current metadata cannot silently substitute a
different current array element.

A custom Character widget is a different principal from the selected Chat UI.
Current Chat mutation admission cannot simply be inherited by embedding it.
Recommended minimal integration is Chat-owned controls calling Chat-owned commands,
with Character supplying options. If a custom widget must apply a choice directly,
add explicit narrow contributor admission: exact configured principal, exact
conversation/revision and candidate ID/card revision, resolved and validated by
Chat against Character. Arbitrary text injection, broad Chat write access or
sharing the parent UI channel are not acceptable shortcuts. A click may perform
the admitted action directly; a second ceremonial confirmation is not required.

## 6. Player preset without a second character system

Treat the player persona as authored fictional character data owned by Character,
independent of operating-system users, API credentials and authenticated clients.
Start with stable ID, editable display name, bounded description and optional
portrait AssetRef, with a record revision. Reuse the existing character data model
where it can represent an authored record without a mandatory imported card.
Do not require a new RTW, user table, role-specific Core API or AI-provider preset.

Chat stores its selected persona reference; the default is an explicit no-persona
state with current `You` behavior. A fork captures its own selection, so switching
selection in the new chat does not alter the original. If two chats select the
same editable persona, an edit affects future uses of that authored record in both;
provide Duplicate preset for independently editable personas. This is separate
from immutable transcript sharing and must not rewrite stored messages.

Narrator captures persona ID/revision alongside character and input context, reads
the bounded authored description and resolves `{{user}}`. The admitted generation
uses that immutable snapshot; editing the shared Character record affects future
requests, not an already captured prompt. Changing the selected persona in Chat
increments Chat's revision and invalidates an old append through existing guards.
This distinction avoids pretending that separate owner reads form a cross-owner
transaction. A caller may explicitly Stop before applying an edit to a new request.
Never combine two persona revisions into one prompt. Keep a bounded speaker label
snapshot on new messages if historical labels must remain stable; a mutable display
name is not message identity. Stored macro-expanded text never changes retroactively.
Deleting a referenced preset leaves old transcript text and admitted snapshots
intact; future generation requires an explicit replacement/no-persona choice, not
a hidden fallback to another record.

Player description is narration context, not mechanically authoritative inventory,
traits or abilities. Scenario owns those when introduced. Later distributable
persona packages use owner import/export; custom package handlers are not needed
to create/select a persona in a World now.

## 7. Delivery order and acceptance

These are small deliverable boundaries, not a mandatory sequence of new frameworks:

| Slice | Useful outcome | Dependency |
| --- | --- | --- |
| F0 | Freeze current contracts, product decisions and active-data upgrade plan | This audit; no product rewrite |
| F1a | Local package manager: inventory, import, scoped selection/version switch, disable/remove and honest lifecycle | Existing Host primitives; accepted inventory/guarded-selection/removal amendments |
| F1b | Stop saves one validated prefix; silent transport cleanup measured/fixed if necessary | Chat/Narrator/AI cancellation contract; independent of F1a and branches |
| F2 | Independent listed chats, edit/delete/fork, regeneration and stable variant navigation | Chat history/admission/schema acceptance; F1b for generation interruption |
| F3 | Player persona and alternate greetings; message slots with a realistic external widget | Captured context; F2 for changing a persisted greeting |
| F4 | Typed World examples/small authoring tools; then Memory/Lore/Psychology | Examples can start beside F1; transcript-derived memory waits for stable Chat source semantics |
| Later | Repository updates, custom package types, advanced media, autonomous simulation | Separate concrete requirements and measured boundaries |

Do not block Stop on package repositories, personas on a universal preset manager,
or location/actor examples on a complete branching UI. Do block automatic
transcript-derived memory on an explicit branch/source policy. Basic dialogue
work does not require the full private multi-actor observation framework first.

| Check | Required evidence before declaring that slice complete |
| --- | --- |
| P01 | Local extension and template import; invalid/corrupt archive; bounded transfer; import executes nothing |
| P02 | Two installed versions, exact digest switch, same-version/different-bytes, default recipe versus existing World, configured versus running state |
| P03 | Dependency conflict including reverse dependents; guarded plan failure changes no selections; changed scope invalidates stale plan; bytes imported before failure remain only cached |
| P04 | Remove selection keeps World data; owner/pin consequences visible; shared/in-use artifact deletion rejected; removed unused template disappears from the actual picker |
| P05 | New permissions remain unapproved; retained grants follow existing policy; foreign/nested UI cannot gain manager authority |
| S01 | Stop before text, after text, during provider failure and after completion; exactly one or zero canonical messages as specified |
| S02 | Stop/append race, stale input, lost begin/status, repeated Stop, wrong caller, panel close and World stop; no replay or forced stale write |
| S03 | Silent upstream with no helper heartbeat; measured UI stop and native resource retirement/quota behavior |
| H01 | Fork appears in list/restart; edit shared early prefix only changes the new chat; original and its later continuation remain byte-for-byte readable |
| H02 | Middle delete versus delete-from-here; original preservation; switch variants with existing suffixes; no accidental suffix mixing |
| H03 | Regenerate persisted assistant from correct captured input; failure/Stop cannot overwrite original; concurrent generation bounded per chat |
| H04 | Cross-chat cursor/ID misuse, stale revisions, source chat deletion, long prefix paging/fork/delete limits and no partially visible history |
| H05 | Active MVP data conversion, old caller rejection/version handling, restart and template round-trip preserve every branch and persona binding |
| U01 | External message widget, provider replacement/stop, forged descriptor/caller, recycled row, private data admission and bounded iframe count |
| U02 | Multiple greetings survive import/restart; stable candidate identity; arrow changes select correct independent chat; current card edits do not rewrite old text |
| U03 | Two personas, duplicate/selection independence, immutable in-flight persona snapshot versus stale Chat selection, preset deletion and plain no-persona path |

Source inspection and helper unit tests establish existing building blocks only.
These acceptance scenarios require actual native/WASM/browser integration evidence
when implemented. Do not report the 74-feature roadmap or these new paths as
delivered because their primitives or descriptions exist.

## UI refinement: launcher-style local management

The owner requested a Prism Launcher-like interface after inspecting the first
manual UI. Packages becomes a launchable Host app in the ordinary Home app bar,
with scope navigation, compact selectable rows, Installed/Library tabs, search and
a single details/actions panel. Scope labels use Home, New World defaults and
actual World names. Exact digest and permission details belong in the details or
review, not repeated as full-width prose on every row. Same-version/different-byte
artifacts remain separately selectable in the version control.

Host app hosting reuses native-attested launchable views and existing isolated
Surface mounting; there is no package-specific Core shell or new registry. Switching
Home apps closes the old view under existing cleanup semantics. Package permissions
and backend mutation rules are unchanged.

A live launchable Host app may request existing `ui.create-world` with an optional
canonical `template` digest. This opens the Root-owned chooser only. Root validates
that the template is installed; creation and capability consent still require the
existing user's submit. World views cannot use this intent. Existing selected Home
renderer create/open intents keep their behavior; only Home renderer may open a
World directly. This UI intent grants no native lifecycle or policy capability.
