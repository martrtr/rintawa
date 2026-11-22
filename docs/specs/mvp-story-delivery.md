# Completing the story MVP

Status: owner-directed delivery plan, updated 2026-11-22. The owner rejected
exclusive Host replacement and selected concurrent Worlds with per-World exclusion.
The boundaries below guide implementation; record exact new wire schemas before
implementing them. This is not a report of completed functionality.
The template format is defined in [world-template-packages.md](world-template-packages.md).
Earlier P01–P10 checkpoints are evidence, not a declaration that this product flow
already exists. Implement sequential slices below; each must leave a usable result.

## Product boundary and ownership

Deliver: New World -> installed template OR template file -> approve requirements
-> welcome/import Character -> linked conversation -> AI reply -> restart/resume
-> export -> instantiate exported file in a clean home.

A World is an independent story/save, not a Core Chat type. Extra conversations are
ordinary optional scenes, accessed through a drawer. No privileged first chat.
Current scene is presentation state keyed to World and persisted without credentials;
missing selection prompts a choice, never guesses by title. Default navigation is
collapsed and the active conversation occupies the workspace.

Chat Roleplay is a separately versioned `rintawa.world-template@1` RTW, authored by
rtwKit, not owned or registered by a running Chat/Character extension. Installed
verified template artifacts are the catalog source. A package-manager/export handler
interprets their content; no provider must run just to list a template. Installed
bytes, enabled activations and user selection are distinct concepts.

Ship a maintained starter World/recipe as that template: Character, Chat, Narrator,
AI provider, roleplay workflow/UI and their exact selections/default configuration.
A seed database is optional; an empty story still has meaningful composition/config.
The seed contains no publisher API key, active jobs or session tokens. Publishing a
new template version does not update stories already created from the old version.

Default running environment is the web-runtime plus web-ui (the latter needs its
execution-target provider); no Character/Chat/Narrator instance runs in baseline.
Their artifacts may already be installed without activation. Do not make Character
or Chat generate a template as a startup side effect. An extension distribution
may recommend/bundle a template dependency, but lifecycle never owns the template.

## Export and file selection are part of this MVP

The create dialog offers installed Chat Roleplay and 'From file (.rtw)'. An exported
World is a template too: selection creates a NEW World with the captured story,
not a merge, overwrite or automatic reset to an empty story.

Default export is self-contained: snapshot of committed DB, exact World composition,
required extension RTWs (including required host providers), portable settings,
source cards, portraits and all declared resource dependencies. Use template slots
for references to provider prerequisites; resolve approved local providers explicitly
before activation. Do not copy the complete Host baseline or unrelated CAS.

Excluded: secret values, tokens, in-flight AI work, channels, local paths, unrelated
Host preferences and arbitrary files maintained outside World by other applications.
Model ID, sampling settings and safe endpoint configuration may travel as domain
settings; secret bindings require local configuration. Reject/redact embedded
credentials in provider URLs/config. A share preview states that chats/memories are
included; this is not automatic privacy sanitization.

Export requires orderly stop/drain; busy refuses export while retaining ownership.
A live generation not yet submitted may be abandoned with explicit user notice.
The saved World is unchanged. Produce the package before presenting download; a lost
browser connection cannot corrupt the source or leave a published partial archive.
Import verifies all bytes before activation and asks for local approval of requested
capabilities. Modpack installation never approves itself.

Large RTWs do not fit the 16 MiB user-resource contract. Add a separate privileged,
bounded, disk-backed package upload/download path using existing artifact validation
and RtwLimits. It is shell/admin only, cancellable, cleans temporary files, provides
an explicit status and never executes uploaded content during upload. No arbitrary
filesystem paths from browser and no whole-archive WASM buffering. Before coding this
public wire boundary, record its exact methods/errors/quotas in the browser contract
before implementation; do not silently reuse Character upload with raised limits. CLI file
acceptance is the first implementation checkpoint, browser file selection is required
before declaring the requested MVP complete.

## Welcome/import/Chat coordination

A selected roleplay workflow owns the experience; shell only mounts the template's
entry. Do not hard-code Character methods or Chat empty-state rules into Core/shell.
Reuse ordinary transport and existing selected-provider identities.

Narrow proposed domain authority amendment: define one World-scoped single-provider
contract `rintawa.roleplay.workflow@1`. Character permits allocate/begin-import to its
own UI OR the exact live selected workflow principal in the same World. Chat permits
allocate/begin-create to its own UI OR that selected workflow. Status/release remain
bound to the initiating activation. No grant to arbitrary same-World callers and no
cross-World authority. The workflow has approved resource-upload/read/render rights
only where its actual operations require them. Existing independent UIs still work.

The workflow preallocates Character/conversation IDs, imports, waits for COMMIT, then
creates the linked conversation and shows it. These are distinct owner transactions.
On second-step failure retain Character and offer Continue. On lost reply reread known
IDs; never auto-replay. On restart discover existing conversations/Characters first;
missing conversation offers completion, not re-import. No durable general jobs service.
Expose bounded view navigation/current-selection within rtwKit; any MessagePort view
parameters remain inert validated domain IDs, never an owner/scope override.

## AI path: concrete minimum, no durable scheduler

Narrator is a World extension; AI provider performs network work through existing
approved HTTP/secrets imports. Chat remains usable without them. Configure credentials
locally; missing credentials produce an actionable state, never a half-saved reply.
Reuse pure inference codecs where compatible. Legacy 256 KiB inference envelopes do
NOT fit the 64 KiB transport contract: size the new runtime requests/results to the
existing boundary; do not silently increase transport limits.

After the user message COMMIT, the UI invokes the selected Narrator with conversation
ID and committed user-message ID. Narrator verifies them through Chat read methods,
loads Character source through explicit read permission and constructs a bounded
prompt using documented field mapping. Card text is prompt data, not executable
Host policy; no automatic remote asset fetching or tools in this MVP.

Initial policy: one active generation per conversation; 16 operations per backend
including unreleased results, released-pending work still counts. UTF-8 generated
answer at most 16 KiB; stop generation with an explicit size error rather than silently
truncating and committing. Context uses a bounded recent suffix of messages with
explicit token/byte budgeting; no unbounded full-history fetch after every send.
No tools, automatic multi-agent chains or persistent generation resumption required.

Proposed Chat authority amendment: a separate assistant-append operation accepts only
the exact selected same-World `rintawa.narrator@1` principal. It includes a known
message/entry ID, the initiating user-message ID and expected lastEntryId. Chat System
checks conversation membership, role, unchanged append boundary and monotonic entry ID
before one atomic commit. Stale context rejects the answer, never inserts it into a
new conversation state. Narrator alone does not receive arbitrary Chat write access.

Keep model work outside System/SQL. Streaming text, if shown, is provisional. Only
Chat COMMIT creates a saved assistant message. Poll/status may retry while live;
generation begin and append never replay automatically after unknown outcomes.
Stop/restart cancels or abandons unfinished generation; saved messages survive.
Network timeout after upstream acceptance can have incurred cost: retry is explicit.

Before implementation, finalize exact domain DTOs/error enums in rtwKit's Character/
Chat/Narrator spec using these bounds. This is new domain policy, not a new Core API.

## Baseline and World composition: accepted concurrent model

Baseline remains running while Worlds start/stop. No exclusive mode, global baseline
replacement, automatic baseline stop or restoration workflow is to be implemented.
Multiple Worlds run concurrently subject to resource admission; the current eight-World
limit is a budget default, not a single-World product model or unlimited capacity.

Current implementation starts baseline in Host scope and materializes the independent
default-world recipe into fresh World instances. Do not claim live baseline inheritance.
Reuse this model: defaults/templates seed a World's persisted composition; its explicit
choices are authoritative after creation. Changing baseline/defaults must not silently
change existing Worlds. Installation is not activation or World access.

An extension excluded/disabled in World A may remain active in baseline and World B,
but MUST NOT gain access to A through fallback, shared provider identity, stale handles
or root-scoped transport. Apply these observable rules at the existing shared boundaries:

- World domain instances, Systems, property authority and transport points come only
  from A's enabled composition and explicit provider selections. No lookup fallback
  from a missing/disabled World owner to a same-name baseline owner.
- Shared Host execution providers may serve only explicitly admitted dependents in A.
  Their delegated imports use the dependent's exact activation/scope/permissions;
  a baseline provider's privileges never substitute for a disabled dependent.
- Excluding a shared provider for A denies new A-dependent starts/channels and retires
  its existing A bindings. It does not stop the provider or its B/Host resources.
  A composition whose required provider is excluded must fail validation or explicitly
  disable its dependents; never silently use an alternative root provider.
- Disabling a domain owner leaves its persistent data intact, revokes runtime read/
  write authority and routes, and does not install another owner merely because its
  schema/name matches. Re-enabling must use approved exact selection.
- MVP composition changes may require orderly World stop then restart. Close admission
  and drain already accepted commands before claiming disable complete; no forced
  cancellation/rollback guarantee for previously admitted work. Busy keeps ownership.
- Frames, queryables, subscriptions, tasks, readers and handles for removed A
  activations are revoked. Stale callbacks cannot acquire replacement activation
  authority; resources for B remain valid.

A World without enabled web views remains visible as a managed World in the root
shell's list, with title/status and approved lifecycle/export actions. It exposes no
World content panels and receives no generic shell content viewer as fallback.
Show 'No web interface enabled for this World' rather than an empty broken workspace.
The administrative shell's explicit World-management capability remains valid; it
is not permission to read arbitrary domain properties or act as a disabled backend.
Trusted native Host code remains outside guest sandbox guarantees.

Do not invent a Core UI flag. Absence/disable of view components and World-specific
provider selection express the behavior. Baseline web-ui keeps serving other Worlds.
An explicit prohibition of a shared baseline provider is a World binding constraint,
not a request to kill its process. Before implementation, map this to existing scoped
composition/target APIs; if they cannot represent the negative selection, document
and implement the smallest shared composition amendment, not a web-specific deny list.

Acceptance: A disables an owner/provider while B uses it; baseline stays alive, A
cannot query/mutate/mount through it, B continues. Test ordinary/delegated callbacks,
stale handles, stop/restart, missing required provider and exported/imported selection.
A headless World can run backend work and be managed without exposing content panels.

## Delivery order and completion evidence

1. Stabilize existing combined Character->Chat restart: recorded disconnect/CDP
   failures must be localized, not excused by occasional green runs. Fix repeated
   full-history reads and retain paging under concurrent appends.
2. Implement template content handler + exact dependency planning + stopped snapshot
   export + validated seed creation. Prove two fresh independent Worlds in a clean
   home from one self-contained RTW. No browser assumptions at this checkpoint.
3. Implement approved administrative browser package transfer and template chooser.
   Missing dependency/permission, corrupt file, cancellation and interrupted publish
   must give concrete recovery with no partially visible World.
4. Implement approved workflow authority and character-first entry. Import opens
   one linked conversation; lists default closed; selection resumes after restart.
   Exercise lost replies between the two commits without duplicate creation.
5. Implement approved Narrator/AI domain contracts with real provider configuration.
   A deterministic provider fixture establishes rollback/timeout/stop behavior;
   explicit opt-in real provider run establishes end-user integration and cost.
   Do not make paid requests or transmit user cards without user authorization.
6. Export the resulting World; import from file into a clean home and a second World.
   Verify chats, source/portrait, portable settings, asset closure, owner bindings,
   fresh IDs at World/instance level and absence of copied credentials. API-key
   reconfiguration is expected. Re-export/re-import must preserve local graph links.
7. Verify the concurrent baseline/World exclusion matrix above, including a headless
   World, World-local replacement, no root fallback and uninterrupted second World.

Keep raw commands, revisions, artifact hashes, logs and exit codes per changed slice;
reuse valid evidence. No blind 'continue developing' loop. Each slice ends with the
observable scenario, remaining limitations and any narrowly required contract change.

The template document alone was not a full AI-MVP instruction. This plan supplies
ownership/workflow/AI/release gates. Sol should complete exact bounded DTOs and error
cases within these boundaries before implementation, then proceed slice by slice.
Return to the owner only for a consequential deviation: broader authority, changed
persistence meaning, increased limits, hidden replay or an incompatible lifecycle.
Do not require another Astra pass merely because a slice ends.

## Verified implementation matrix — 2026-11-22

Revisions: Core `2f1cf07625cd848176c05cb5507aa80ac497022c`; rtwKit
`d0a9ed8` plus the preserved working Character/AMOLED implementation. No Package
Manager or separate World Manager is part of this delivery.

| Requirement | Existing mechanism | Missing | Acceptance / evidence |
| --- | --- | --- | --- |
| Character / linked Chat / restart | Actual WASM owners, native take-resource, CAS, World transactions, browser UI | Welcome coordination; bounded recent history; intermittent connection cause | Existing `/tmp/rtwkit-amoled-{chat,character}-final.log`; slice 1 must discriminate failures |
| Templates and portable export | Immutable RTW/CAS, validated current DDL, snapshots, composition materialization | Template handler/catalog, seed validation/publication, resource closure, stopped export | Create twice in a clean home; corruption/busy/approval failures |
| Browser package transfer | Shell admin World/session API; small ephemeral upload | Separate disk-backed RTW upload/download and chooser | Cancel/interruption/corrupt RTW; no partially visible World |
| Roleplay entry | Exact scoped transport, selected providers, own-UI domain admission | Selected-workflow authority and entry/selection DTOs | Import COMMIT then linked Chat; reconcile lost replies without replay |
| AI | HTTP/secrets capabilities; pure inference codecs | Narrator/provider runtime and bounded assistant append contract | Deterministic local fixture before any opted-in paid request |
| Concurrent Worlds | Separate persisted compositions, scopes, eight-World budget, exact delegated contexts | Explicit World-local negative shared-provider binding | A excludes provider; B/baseline continue; A handles retire after drain |
| Headless World | Host lifecycle and administrative list; no domain fallback required | Explicit UI state and exclusion matrix evidence | Managed/exportable World with no mounted content |
| Portable Character assets | Current records preserve exact source and portrait refs | Audit current camelCase/string-size refs against template portability shape before export | No guessing or silent schema rewrite |

Source audit for exclusions: `CompositionProfile` has positive selections but no
negative target/provider constraint. `ExecutionTargetRegistry` resolves globally
by target (`artifact_loader.rs` / `execution_targets.rs`). Omitting a baseline
provider from A's activation list does not establish an exclusion. Keep this gap
open until slice 7; stopping the provider globally would violate the accepted model.

Current work, 2026-11-23: slices 1 and 2 are complete. The installed-template
portion of slice 3 and Character-first workflow of slice 4 now have real-browser
acceptance. Disk-backed browser RTW file transfer/export in slice 3, Narrator/AI,
World-local provider exclusions and final full-MVP acceptance remain open.
Slice 1 Core is committed as `b997e98`; native templates and the new World-session
WIT methods are pending owner commit. rtwKit's artifact tooling still pins that
previous commit; update its Core pin after the companion Core commit exists.
The live dev installation has been updated to this checkpoint; no MVP completion
or published release is claimed.

### Slice 1 bounded recent-read addition

Add an ordinary owner-scoped WIT `world-data.find-relations-before(targets,
direction, property, before, limit)` returning strict descending relation-ID order.
`before` is exclusive; absent means start with the greatest ID. Existing forward
`find-relations` is unchanged. Reuse 64 targets, 1..256 rows, existing byte bounds,
exact readable-property checks, pinned snapshots and existing indexes. No writes,
extra table, cross-World access or limit increase.

The real consumer is Chat's recent window; Narrator's bounded suffix will reuse it.
Chat adds read-only `recent {conversationId,before?}` with at most 32 entries and a
64 KiB encoded reply, items presented in ascending order, `older` exclusive cursor
and `head` equal to the greatest returned entry ID. Ordinary `page {after?}` stays
available. Selection/refresh/send read one recent page; explicit older paging
prepends/merges by entry ID and preserves the scroll anchor. Never derive a cursor
from a separately-read lastEntryId or automatically scan the whole history.

Acceptance: reverse order/exclusive boundaries/filter/isolation/unchanged limits;
old pinned snapshot versus concurrent append; real guest recent/older pages and
64 KiB cap; UI history no longer grows its request count with total transcript size.

### Slice 1 checkpoint and evidence

Character/Chat/browser foundations were preserved. The recurring Root disconnect
was reproduced: a fresh Host clock read returned `ClockError::Unavailable` during
an authenticated request; the runtime closed that socket, producing Chromium 1006
and detaching both panels. The runtime now rejects the request with correlated
`unavailable/not-started`, preserving the socket/channels. It still observes the
clock before admission and authenticates fail-closed. Core's nonblocking clock
contract was not changed. A separate reproduced framing bug now leaves coalesced
bytes in TCP until the bounded decoder has input capacity; limits were not raised.
Both fixes have failing-before/passing-after real-WASM evidence and persistent
regression scripts. Neither relies on automatic mutation replay.

Chat now has a bounded recent window and explicit older pages. Actual Chromium
coverage builds 70 committed messages, appends one during paging, reads 32/32/6
native pages and the later row through the forward cursor, verifies the 64 KiB
limit with escaped 16 KiB messages and denies another World's reads. The UI displays
32 then 64 then 71 messages in order and preserves a visible message anchor. It
uses the actual retained DOM row and immediate scroll adjustment rather than
animated total-height compensation. Character import/link/message/restart also
passes with the fixed runtime and a synthetic local card.

Evidence manifest: `/tmp/rintawa-story-slice1-evidence.json`; socket reproductions
and exact execution metadata: `/tmp/rintawa-story-socket-audit.md` and
`/tmp/rintawa-story-socket-checks.json`. Main successful raw logs:

- `/tmp/rintawa-story-slice1-core-check-loopback.log`: required full Core gate,
  464 Rust tests, exit 0 (loopback permission required).
- `/tmp/rtwkit-story-slice1-check.log`: required rtwKit gate, 70 Rust / 16 browser
  unit tests plus WASM Clippy/typecheck/build, exit 0.
- `/tmp/rtwkit-story-slice1-character.log`: real Character/linked Chat/restart, exit 0.
- `/tmp/rtwkit-story-slice1-history-browser-anchor-after.log`: real bounded native
  and browser history/anchor, exit 0. Its final UI build/typecheck is in
  `/tmp/rtwkit-story-slice1-anchor-build.log`; unchanged Rust checks are reused.
- `/tmp/rintawa-story-native-regression.log` and
  `/tmp/rintawa-story-permanent-clock-regression.log`: coalesced frames and isolated
  clock-failure injection, both exit 0, exact metadata in the socket sidecar.

The next slice is template CLI acceptance with stopped snapshot export and two
independent Worlds in a clean home. Its source inventory is
`/tmp/rintawa-template-inventory.md`. No template/AI/exclusion feature is claimed
implemented by this checkpoint. No Package Manager, separate World Manager,
exclusive Host mode, paid provider calls or private-card transmission was added.

### Slice 2 checkpoint — Native templates and portable snapshots

Core installs arbitrary verified RTW containers without changing composition.
The built-in World-template handler validates exact dependencies, declared schema
owners, seed integrity/graph/payloads and the full asset closure. Both embedded and
thin dependency payloads are checked, including old generic CAS imports; aggregate
expansion limits apply. No package handlers or guests run during installation.

Creation selects an immutable installed template before constructing anything.
It publishes a new DB and freshly materialized composition as one directory rename
from private staging. The resolved entry is returned from that same materialization.
Restart recovery skips active stages and reclaims abandoned ones. Requested grants
require explicit approval, never copy trust from an export. The original default
recipe uses this publisher too; no template is applied to an existing empty World.

Stopped export captures SQLite WAL state, exact composition/artifacts and supported
AssetRefs, preserving local graph IDs. It excludes local approvals, credentials and
Host preferences. The completed private RTW passes the same import validator before
no-clobber output publication. Unsupported schema owners, resources, unrecorded
required Host targets and active/unclean lifetimes fail before publication.
World runtime locks cover activation, drain, failed cleanup retries and implicit
Native teardown; provider checks match the actually loaded artifact as well as its
stable instance/component identity. Positive prerequisites are implemented here;
World-local negative execution-provider exclusions remain slice 7.

rtwKit generates `world-portability.json` from its existing Chat/Character schema
functions and builds the exact local `ChatRoleplay.rtw`. Both accepted AssetRef
encodings are explicit per schema; no persisted rows are rewritten. The current
entry is Chat UI. Welcome/import coordination and Narrator/AI are later slices.
No Package Manager or separate World Manager was introduced.

Successful checks and durable evidence:

- `/tmp/rintawa-story-slice2-core-final.log`: required Core gate, 483 passing tests;
  the external RTW test is explicitly ignored by this general gate.
- `/tmp/rintawa-story-slice2-real-wasm-final.log`: that real RTW test was executed
  separately and passed. Actual Character/Chat Systems commit, stopped export
  restores two clean-home forks with new World/instance IDs and preserved graph
  IDs/source bytes, independent edits, fresh owner bindings and refusal of a running
  provider A when disk selection changes to B under the same instance ID.
- `/tmp/rintawa-template-implicit-drop-final.log`: ten cleanup/lifecycle regressions,
  including an Engine component destructor barrier while the writer is already
  gone, keep export/delete blocked until teardown finishes.
- `/tmp/rintawa-template-thin-crc-tests-resumed.log`: template regressions include
  malformed nested installers, expected digests, warm/thin corrupt dependencies,
  unavailable catalog entries and refusal to export an unimportable dependency.
- `/tmp/rintawa-story-slice2-cli-final.log`: CLI template choice/approval/create,
  independent composition, export, clean-home install and repeated forks.
- `/tmp/rtwkit-story-slice2-full-check.log`: full rtwKit gate, 70 Rust / 16 JS tests,
  native/WASM Clippy, typecheck, documentation and package assembly, exit 0.

All commands, revisions, source/RTW hashes and raw final logs are preserved under
`.vscode/evidence/story-slice2/` and copied to
`/home/mart/Desktop/rintawa-evidence/2026-11-22-story-mvp/slice2/`.
The prior independent Sol findings and their subsequent fixes are recorded there;
source gates and failing-before regressions are distinguished from final checks.

Limitations: Linux/loopback local acceptance; the real-WASM fixture uses trusted
Native command ingress rather than browser upload. Older experimental RTWs without
portability declarations are explicitly unsupported for export; no migration or
compatibility layer was added. CLI transfer is complete; browser template upload,
chooser/download, welcome/import-to-Chat and AI are not claimed by this checkpoint.
Next: slice 3, the bounded administrative browser transfer/chooser contract, then
implementation using this shared Native path.

### Installed-template browser and character-first entry checkpoint

Owner request, 2026-11-23: expose the installed **Text Roleplay** template in the
creation dialog, create and start its independent World, mount its declared entry,
import Character and open the linked Chat. This checkpoint connects installed
packages; disk-backed browser RTW transfer remains a separate unfinished acceptance
item of slice 3. No Package Manager, AI or exclusion completion is implied.

The existing privileged `world-sessions` boundary gains `list-templates` (bounded
catalog), `template-info(digest)` (bounded inert descriptor JSON),
`create-from-template(digest,title,approve-runtime-permissions)` and
`entry(world-id)` (optional stable instance/component). Read/write permissions,
browser shell-only enforcement, mutation budgets and message limits remain unchanged.
Creation uses the existing native atomic publisher and returns only after publication;
activation is a separate admitted `set-active` followed by read-only reconciliation.
A lost creation reply must never cause automatic resubmission: refresh the World
list and let the operator select the published World. Capability approval is an
explicit unchecked dialog control displaying the template's exact requested grants.
Missing providers, malformed templates and refused grants leave no published World.
Entry resolution is exact and World-scoped; unavailable entries stay recoverable.

Within rtwKit, the Character extension owns the workflow in its separate story UI. Its static web-bundle descriptor
may declare bounded single-provider contracts, registered under its actual delegated
principal. Text Roleplay explicitly selects Character/story for
`rintawa.roleplay.workflow@1`. Character and Chat admit only their own UI or the
exact selected live workflow for allocate/import/create (Chat send remains own-UI).
Import and create remain separate committed operations with preallocated known IDs;
unknown outcomes are read back, never replayed. After restart discover existing
conversations and Characters before offering import. A Character without a linked
conversation offers explicit Continue; failed Chat creation never deletes Character.

The browser bridge adds owner-scoped `ui.state.get/set`, one fixed preference key
per exact component, at most 1 KiB of inert flat string state (at most eight keys,
64 ASCII bytes/key and 256 UTF-8 bytes/value). No caller-selected preference key,
owner or scope. It stores current conversation and preallocated workflow IDs, never
credentials or runtime handles. `ui.navigate` is a local shell/MessagePort request
for an exact same-World instance/component with the same bounded inert state.
The shell resolves the source from the mounted binding, refreshes the live catalog,
rejects foreign/missing/ambiguous destinations and mounts under the destination's
own principal. State is data only, delivered to the target on initialization; it
cannot override dispatch scope/owner. The destination validates domain IDs and
rereads the World. Existing layout serialization remains unchanged.

Acceptance: actual Chromium installed template selection and explicit grant review;
creation auto-start and exact entry; Tavern V2/V3 import -> one linked Chat -> saved
message -> restart/resume; failure between commits preserves Character and offers
Continue; forged/cross-World navigation or workflow calls are denied; two Worlds
remain independent. Preserve raw redacted logs and final RTW hashes.

Wire details for this checkpoint: `list-templates` returns JSON rows
`{digest,name,description,invalidReason:string|null}`. `template-info` returns the
existing template descriptor as inert JSON. Both encoded Native results are at
most 64 KiB and additionally respect the current Host message budget; excess is
`message-too-large`. Creation requires all three fields (canonical exact digest,
1..256-byte nonblank title, explicit boolean approval); it returns the ordinary
World summary. `entry` returns null or `{instanceId,componentId}`, with the World
coming exclusively from its request. Native errors retain the existing World
session cases; unapproved/invalid composition is rejected. Read failures may be
retried; admitted creation and unknown lifecycle requests are never replayed.

Character's ordinary `ui` remains its independent collection interface. The separate
`story` component provides the workflow contract and reuses the same web bundle.
Only the story descriptor declares that contract, so ordinary standalone Character
installation does not accidentally enable onboarding by single-provider resolution.
The story component requests only background-task and user-resource-import; Chat
renders assets under its own existing asset-read permission.


### Character-first entry acceptance, 2026-11-23

The installed Text Roleplay dialog explicitly reviews World grants, atomically
creates a new World, requests start and resolves Character/story by its fresh exact
identity. Character story UI imports the card, waits for COMMIT, creates its linked
conversation through selected-workflow authority and navigates to Chat's own UI.
The default conversation drawer is collapsed. Native owner-scoped preferences hold
current selection and inert recovery IDs, without changes to the layout schema.
Read-only transient failures are retried/reconciled; create/import/send are not
replayed after unknown outcomes. Ordinary Character/ui keeps its collection mode.

Actual Chromium acceptance covers Tavern V2/V3 in two independent concurrently
running Worlds, failure before Chat admission after Character COMMIT and explicit
Continue, one linked conversation, a saved message, Host restart/resume without
duplicates, denial of workflow attempts to send Chat messages and of Chat callers
to import Characters, and a forged MessagePort navigation to another World.
The standalone Character PNG/portrait/CAS failure/restart browser regression passes
as well. Native self-contained export/independent forks were rerun on these RTWs.

Required checks: Core 483 tests, rtwKit 71 Rust + 17 JS tests, WASM Clippy, builds,
typecheck and documentation gates passed. The external real-WASM template test
passes separately (one test). Important logs, RTWs, source hashes and screenshots
are preserved at `/home/mart/Desktop/rintawa-evidence/story-entry-2026-11-23/`;
private working copies also remain in `.vscode/evidence/story-entry-ready/`.
The normal system launcher was checked against the updated dev home: HTTP 200 and
clean SIGINT shutdown, exit 0. Existing Worlds were not rewritten or templated.
The dev setup helper now installs only runtime/shell in baseline and the immutable
Text Roleplay package, never Character/Chat baseline instances or a default recipe.
