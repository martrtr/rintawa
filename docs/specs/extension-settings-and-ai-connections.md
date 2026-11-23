# Extension settings and usable local AI

Status: owner confirmed shared AI connections on 2026-11-23, and clarified that
Settings lists only extensions running in the selected baseline/World scope.
Owner condition for implementation: this must be a generic extension primitive,
not AI-specific Core logic. The one-key published preference below meets that
condition for shared configuration across an extension's World instances.
The settings surface is implemented; shared connections do not approve an arbitrary scope
query, implicit baseline provider fallback or broad credential inheritance.

Live-policy amendment: owner accepted immediate exact-activation application and
grouped consent in [live-permissions-and-ui-contracts.md](live-permissions-and-ui-contracts.md).
It supersedes the original next-activation/restart approval behavior below. Current
model-discovery behavior also supersedes the historical no-network-on-open checkpoint.

## Starting behavior before this work

rtwKit web-runtime already catalogs and mounts exact active web components in
isolated frames. Its descriptor has one entry per component. web-ui owns shell
navigation and now supplies Settings. Chat still owns an AI configuration
dialog and the Provider authorizes the selected Chat UI. This couples settings to
Chat and supports one World-local connection only.

Native runtime-policy already has exact scoped loopback-origin read/grant/revoke
operations. Current browser allowlist does not expose them. Approvals take effect
on next activation. Existing shell lacks runtime-policy-read/write grants.

Character preserves exact Tavern source in CAS and a small name/source/portrait
record. first_mes remains in the source; creation of its linked Chat currently
creates no greeting. Narrator maps card prompt fields and recent saved messages.

## Settings surface

web-ui supplies one Settings overlay reachable on Home and in a World. It contains
the running baseline shell's Appearance on Home and sections contributed by active
extensions in the selected scope. A contribution is an ordinary web component declaring optional `settings = true` in its existing
web-bundle descriptor. ui.list returns this flag. No second registry, HTML injection,
settings-schema interpreter or new Core UI API. One component is still one entry.
Settings components are excluded from app launchers and DockView split menus.

The menu captures baseline (`worldId:null`) or one concrete World. Enumerate only
matching active entries; resolve by the existing World/instance/component key and
mount its current opaque ID through RootBridge. Use existing iframe channels,
limits, theme and resource ownership. A stale/missing/ambiguous entry is unavailable,
never substituted with another instance. Closing/switching/stopping unmounts its
frame. The overlay does not start Worlds. Baseline has shell settings even when
no extension contributes a section. A World with zero contributions shows an empty state.
The current `packages/web-ui/src/Settings.tsx` renders Appearance unconditionally.
Restrict it to Home and test the empty World in the next UI change.

AI settings belong to an AI Provider settings component, authorized by its exact
sibling backend identity. Remove the embedded Chat configuration dialog. Chat only
consumes provider-neutral readiness and generation. Narrator needs no new controls.

## Connection editing

AI settings have a connection list and explicit Add provider, Edit, Delete and Use
actions. Initial templates: Ollama (OpenAI-compatible endpoint, initial
http://127.0.0.1:11434/v1, no key) and OpenAI-compatible (explicit endpoint/model,
key required by default). No model download, network request or approval on opening
the menu or selecting a template. User enters a model. No remote default dispatch.
At most 16 named connections. Each World explicitly selects at most one. Accepted
generation retains its chosen connection. Deleting/changing it does not replay/cancel work.
Keys remain in native credentials and are bound to exact connection ownership;
connection labels, endpoint, model and selection are non-secret data.

Owner decision: connections are shared. UI discovery remains strictly local to the
selected scope: Home lists only baseline contributions; World A lists only A's
contributions. A common connection is data/service selection, not permission to
render a baseline extension in A's settings. Opening Settings starts nothing.

An AI settings section on Home therefore requires an explicitly installed and
running baseline AI extension component that owns those connections. It is optional
extension infrastructure, not Core AI knowledge or an automatically added baseline
dependency. Each World retains its own enabled/disabled AI binding. Missing or
disabled World AI must not fall back to an ambient baseline generation point.
The draft below proposes storage, binding, credential access and export behavior
for owner acceptance.

## Local HTTP approval

Ollama requires a separate explicit confirmation of the canonical loopback origin
and exact consuming backend. Expose existing Native runtime-policy operations only
through the selected root shell, with its existing policy permissions. An ordinary
settings frame cannot grant itself authority. Saving endpoint data is not approval.
No wildcard origin or auto-approval on import/template installation.
Changes apply on orderly restart; show that state and offer an explicit stop/start
for the captured World. An admitted World command may finish during drain. Baseline
changes requiring Host restart must say so. Secret writes retain the approved CLI
contract in this slice; do not add browser secret-write as a convenience.

## Tavern greeting and prompts

On character-first Chat creation, save nonblank first_mes as the first assistant
message in the same transaction as conversation creation; absent/blank yields an
empty Chat. Allocate known IDs before submit. No automatic generation on import,
no greeting replay after restart, and no repeated mutation after unknown admission.
Greeting text observes existing 16 KiB / encoded-message limits; oversize reports
an explicit refusal and keeps the committed Character for recovery. Do not silently
truncate. The original source stays byte-exact in CAS.

Character import now derives `rintawa.character.greeting@1` on the Character object
in the same import transaction, alongside the unchanged card reference. It stores
{text:string|null,error:null|greeting-too-large}; blank first_mes has neither text
nor error, oversized/over-escaped first_mes has only the error marker. The full
original remains in CAS. Character get adds this bounded greeting record; lists
remain small. No JavaScript PNG decoder or extra asset grant is required.
Chat begin-create accepts an optional greeting {messageId,entryId,text}; its System
checks all distinct/unoccupied IDs and commits conversation/message/relation and
lastEntryId together. Unknown creation is recovered by the known conversation ID.
The default local display label is `You`, matching Chat; exact {{char}}/{{user}}
markers in card fields/greeting expand once to Character name/that display label.
Other macros and user-written messages are left literal. This is presentation and
prompt data, never an authenticated principal or template code execution.
Prompt mapping must preserve the Character system/scenario/personality/example/
post-history fields; the saved greeting participates as assistant history.
No additional persona settings or executable macro language is introduced.

## Acceptance

- Settings opens on Home and in A/B; exact scope filters and independent mount
  cleanup; contribution never appears as Chat/app/split. Stopped World unavailable.
- Ollama and OpenAI-compatible templates add/edit/select/delete connections, survive
  the appropriate restart and never dispatch on settings reads. Invalid config,
  operation quotas and unknown saves remain explicit. Keys never appear in World,
  browser storage, diagnostics or export.
- Ordinary panels cannot invoke policy mutation; shell requires existing grants;
  exact localhost origin is approved only by explicit user action. Import grants none.
- Synthetic Tavern V2 JSON and V3 PNG first_mes is saved once, survives restart,
  enters the first AI context with prompt fields, and remains in exported clone.
  Oversize/collision/stale/failed creation leaves no partial Chat or greeting.
- Actual browser and WASM use a loopback fixture; no paid provider or private-card
  transmission. Preserve commands, hashes and raw logs outside /tmp.

No Package Manager, generic Settings Manager service, schema migration, new World
table, implicit remote provider, durable AI jobs or automatic replay.

## Settings surface checkpoint — 2026-11-23

The independent shell surface is implemented in the companion rtwKit working
change. The actual browser test installs a JS-only Dice-derived settings fixture
in baseline and two Worlds. Six checks pass: scoped mount, no app launcher,
independent A/B, stopped-World refusal, >32 repeated mount cleanup, narrow viewport.
No dev package is installed yet: the AI section and connection scope remain open.
Rust format/clippy/native/WASM tests/docs, repository validation, TypeScript and
20 browser SDK unit tests passed in scripts/check.sh up to the web build. That build
initially failed because a scratch .vite-temp symlink pointed outside writable
roots; making that generated cache local allowed the remaining build steps to pass.
This is split check evidence, not a claim that the initial gate exited zero.

Durable evidence and source hashes:
/home/mart/Desktop/rintawa-evidence/settings-surface-2026-11-23/.

## Shared AI connections — implementation under the owner’s generic-primitive condition, 2026-11-23

**Recommendation.** Keep generation in the selected World Provider. Run the same
new AI RTW optionally on Home to edit one non-secret catalog. Give an explicitly
approved World backend a read of one published value from its exact running
baseline counterpart. This requires a small Core authority change. A stopped or
missing publisher disables new shared generation. A can still open, use manual
Chat and export. Installing or opening Settings starts nothing.

### Verified execution paths and alternatives

| Route | What actually exists | Cost for two independently stoppable Worlds |
| --- | --- | --- |
| Baseline execution target with explicit World dependents | A baseline WASM can publish an execution target on start. Its proxy can register a single inference contract for each dependent. In delegated start/tasks it can declare a D8 polled point, authenticate that World's selected Narrator, read that dependent's preferences, and start HTTP/read exact secrets under the dependent. A root task could maintain the catalog in the shared Store. | Feasible for compositions requiring that exact target. All per-World handles, operations and grants must belong to the dependent. A Store trap invalidates every hosted AI client. The proxy has no independent World-data access or World System endpoint. Keeping the old World config command would require an ordinary backend alongside it. |
| Baseline catalog, ordinary World execution | Ordinary World backends already have distinct Stores, operation tables, World transport, HTTP/secret grants and teardown. Native preferences have a scoped owner. Guest reads always derive the executing owner. | One new, read-only boundary below. No target descriptor, service-to-service RPC, shared request queue, cross-World cancellation logic, or baseline dependency in the story template. Exact catalog and consumer RTW revisions must match. |
| Generation inside the baseline principal | Scope-local selected providers and Zenoh do not route a World query to Home. A forwarded query authenticates the intermediary. An HTTP worker started under the root belongs to the root. | Requires new inter-scope call admission, attestation and cancellation on World stop, or an explicit delegated frontend plus root-owned work reconciliation. Root keys and origins would authorize multiple Worlds. More rights and lifecycle work than either route above. |
| Retain one copied config per World | Existing ordinary World System/portable property works today. | Editing the common connection would require repeated copies or synchronization. A credential CLI alone cannot provide an editable Home catalog. |

For the target route, the loader preflights **all** required targets before
instantiating any component. A package cannot bootstrap its own required target.
A dedicated baseline publisher and a World descriptor are needed in different
activation units; the descriptor could live in another World RTW, but bundling
it with Narrator couples their enable/disable. Required custom targets must be
in template `hostRequirements` even for disabled slots. Materialization, activation
and export check exact baseline prerequisites. An optional target without such
a prerequisite resolves by process-global target string whenever present. Using
it safely while allowing an absent publisher would require optional **exact**
prerequisites with matched loading/absence behavior in all three paths, or
separate AI-required and manual compositions. That price buys little when only
non-secret data is shared. [Loader](../../crates/extension-engine/src/artifact_loader.rs),
[target registry](../../crates/extension-engine/src/execution_targets.rs),
[template recipe and prerequisite resolution](../../crates/host/src/templates/mod.rs),
[World activation](../../crates/host/src/runtime.rs), [export](../../crates/host/src/templates/export.rs).

The delegated example is rtwKit `packages/web-runtime/src/wasm.rs`,
`register_component/start_component/on_task/stop_component`. See Core
[WasmExecutionTargetProxy](../../crates/extension-engine/src/runtime/wasm/target_provider.rs),
[delegated task access](../../crates/extension-engine/src/runtime/wasm.rs)
and [world_system_endpoint](../../crates/extension-engine/src/engine.rs) (concrete
WasmComponent downcast). D6/D8 supply dependent identity and polled declarations.
A binding cannot invoke the current Provider unchanged. Its `guest.register`
declares `config@1` and a configure command, `on_task` requires a World,
`config()` reads World data, and `secret_path()` derives a World key.
[Current guest/transport rules](mvp-api-and-client-boundaries.md),
[World lifecycle rules](mvp-story-delivery.md). Companion source paths refer to
the rtwKit checkout, principally `packages/ai-provider/src/{wasm,model}.rs`.

### Generic Core read and attribution

- Add one optional manifest declaration on an ordinary WASM component,
  `published-preference = "<key>"`, validated against existing preference key
  bounds. It designates one non-secret value for that component's active baseline
  role. Add a manifest-requested, locally granted runtime permission
  `published-preference-read` for the exact consuming World component and a WASM
  import `read-published-preference() -> result<option<string>, error>`. No
  arguments, enumeration, foreign write, browser method or transport change.
  The entire published value is visible to that approved backend; authors must
  explicitly choose data suitable for sharing. All other preference keys stay private.
- Only an active ordinary World backend may call this import. Capture its exact
  live execution binding, then resolve exactly one active ordinary baseline
  component with the **same verified RTW digest and component ID** and a publication
  declaration. Scope, instance and activation come from the Host. Starting/stopped,
  delegated, System, register/stop, unverified source bytes, and wrong-scope
  contexts cannot use it. Missing grant returns permission denied. Missing,
  ambiguous, stale or mismatched source is unavailable. An active source with no
  published value returns none. Apply the existing 64 KiB preference/value bound.
  Return only the designated value in a single snapshot. No guest SDK can supply
  the source, digest, component, preference name or scope.
- This attestation is **new work**. `RtwExtensionLoader::try_load_stored_extension`
  receives the exact CAS digest. Carry it and the verified manifest designation
  through registration into the existing live binding created by
  `ExecutionContexts::begin`. Today that binding has scope/principal and provider
  linkage only. Look up only active bindings in the existing synchronized view,
  do the bounded preference read without its lock, then revalidate both exact
  activations and the read grant before returning. Never recursively lock the
  Engine or infer running identities from a mutable composition file. A staged
  CLI replacement cannot impersonate either running artifact.
- Publication authorizes the current value of the **one named owner preference**.
  Use a dedicated key for data intended to be shared. Its ordinary atomic writes,
  deletes and upgrade/rollback lifetime already belong to the baseline owner.
  A newer running revision intentionally exposing the same key can use a compatible
  saved catalog as soon as the World installs the matching revision. An incompatible
  value yields invalid configuration before any network call. A rejected save
  preserves the previous value. No writer history, new profile field or republish
  step is needed. Attestation establishes who is running and may publish/read;
  the consuming backend still validates the data, including its World-confirmed
  destination. [Preference owner adapter](../../crates/extension-engine/src/runtime/wasm/control_plane.rs),
  [atomic persistence](../../crates/host/src/lib.rs), [profile and bounds](../../crates/host/src/profile.rs),
  [live bindings](../../crates/extension-engine/src/execution_context.rs).

### rtwKit contract

1. Ship one revised AI RTW containing an ordinary backend and a sibling settings
   view. `guest.register` supplies static scope-local contract metadata and omits
   the old config property/command in this revision. Registration needs no World
   lookup. At start/task time the attested context selects the role. Home exposes
   only catalog administration/readiness and never admits inference. The World
   backend supplies the existing selected `rintawa.inference.text@1` service and
   World binding settings. Pin each writable point to its exact local backend
   through existing authoritative selections. Settings reads/edits check the
   Host-attested live sibling instance and settings component, including on Home.
   Other frames/Chat cannot administer a connection. Only that World's exact
   selected Narrator activation may begin generation. Status/release belong
   to the original caller activation. Narrator and Chat stay ordinary World RTWs.
2. Home owns Add/Edit/Delete, with at most 16 entries and one atomic published
   catalog value within the preference byte limit (60 KiB encoded in this AI
   implementation, leaving transport room for metadata). Entries carry a backend-generated
   random 128-bit connection ID, bounded nonblank label (128 UTF-8 bytes), template
   kind and the validated config fields above and in the companion
   `p10-narrator-ai.md`. Never accept a caller-chosen ID for creation. Verify the
   expected previous entry on edit/delete so stale dialogs cannot overwrite it.
   Re-read after an unknown save response; never replay a mutation automatically.
   Home may show the exact CLI secret path but cannot read/write a value for display.
3. World Settings lists authorized connection metadata and offers **Use here** or
   **Disconnect**. Only a deliberate choice writes one owner-local World preference
   containing the ID and confirmed canonical base URL/key mode. Bind one connection
   per World; there is no global default for generation. Unbound, deleted,
   mismatched or disabled is unavailable. Never select the next entry. Editing
   label/model/sampling applies to the next request. To change canonical base URL
   or key mode, replace with a fresh ID and show that previous bindings require
   new consent. This prevents an existing key grant or saved choice from silently
   sending prompts/keys elsewhere.
4. At generation admission, the World backend reads the current published value,
   resolves its local choice and validates the tuple once. Derive the exact vault
   path `ai.connection_<32 lowercase hex digits>.api_key` from the stored connection
   ID. Use the accepted native stdin/keyring CLI once per connection and a separate
   exact secret-read approval per consuming World backend. The root backend needs
   neither HTTP nor key-read grants. Ollama uses an entered model, no key, and an
   explicit exact canonical loopback-origin grant to each World backend; remote
   OpenAI-compatible uses validated HTTPS with the explicit key policy. Grant
   runtime HTTP to the World backend independently. A trusted root-shell approval
   shows the exact captured World, backend and origin and revalidates them before
   native policy mutation; untrusted frames cannot grant anything. The shell needs
   only the generic policy action, never the AI catalog protocol. Apply grant
   edits on the next orderly activation. Deletion removes the catalog entry. Its
   ID is never recycled. The UI names the retired path so the operator can use
   native secret-delete/revoke for leftover vault values and grants. A shared
   key rotation by CLI takes effect on subsequent reads. No automatic model download, test prompt,
   connection attempt, remote fallback or secret lookup on opening Settings.
5. `begin-generate` keeps its existing wire contract, bounded operation admission,
   exact caller and one HTTP handle in the World Provider's Store. Snapshot the
   connection/config and any needed credential before starting that request.
   Report missing catalog/selection, stale binding, missing HTTP/origin permission,
   missing key grant/value, locked vault, invalid config and exhaustion without
   echoing keys, prompts or provider error bodies. A catalog edit/deletion or
   publisher stop affects future admission only. Host HTTP permission/origin
   revocation retains its existing enforcement. On A stop, reject new work, drop
   A's operations and let native ownership retire tasks, transport and HTTP handles.
   `guest.stop` performs only local memory cleanup. `OwnedHttpRequest::drop`
   signals worker cancellation; an already sent request can incur cost and a
   blocked native call can take until its deadline to exit. Already admitted
   Chat writes may drain. No restart reconstructs unfinished generation or retries
   an unknown begin. B's tasks, handles and commits continue independently.
   [HTTP ownership](../../crates/extension-engine/src/runtime/wasm/http_request.rs),
   [WasmComponent::stop](../../crates/extension-engine/src/runtime/wasm.rs).
6. New exports carry saved Chat/Character data and the usual exact extension
   composition. The shared catalog, endpoint/model, binding, keys, exact secret
   grants, origins and in-flight work stay local. Runtime capabilities may be
   requested for fresh local approval. A same-home fork and a clean-home import
   both open disconnected, including for keyless HTTPS. A user can bind the
   local catalog and approve their new World backend. This **changes** the
   accepted portable endpoint/model result and World-derived key path. Existing
   exact Provider RTWs and snapshots retain their original portable property and
   key semantics. Do not rewrite their data on open, copy old keys, or promise an
   in-place migration. [Existing secret contract](host-secret-configuration.md),
   [current story export and AI checkpoint](mvp-story-delivery.md).

### Acceptance and evidence

- With synthetic WASM packages, publish one value plus a separate private key.
  Exercise the World import with and without the exact grant, wrong digest with
  identical package ID, wrong component, delegated panel, unpublished key, two
  publishers, registration/stop/System callbacks, replacement during read, and
  an on-disk selection change before restart. Confirm atomic save/delete, retained
  compatible data after a coordinated upgrade, and invalid-format refusal. Only the active authorized case returns
  the one value. No new API for arbitrary scopes or private keys.
- Browser and real WASM: Home edits one Ollama connection. A and B select it
  independently, grant their respective loopback origins and run against a local
  SSE fixture. No HTTP on Settings open/read/save. Pause A mid-response, edit its
  model, admit B, then stop A. Check captured request bodies, stale handle denial,
  released worker/operation quotas, B's single committed answer and the absence of
  a resumed A request. Separately allow A to complete across an edit/delete and
  verify its admitted snapshot. An endpoint/key-mode change cannot reuse old
  binding/credential authority. Use an in-memory vault and synthetic keys only.
- Exercise disabled/missing publisher and mismatched revisions, an AI-disabled
  World, A-only grant/revoke, missing/locked vault, unknown save/unknown begin
  and saturated operation tables. Manual Chat and World lifecycle/export continue.
  Close/reopen Settings and stop A while mounted. Home shows baseline Appearance,
  A shows only A contributions, an empty World shows its empty state, B stays
  mounted. No panel, bootstrap or restart triggers generation.
- Inspect a new exported RTW and both same-home and clean-home imports. Saved
  messages survive, metadata cannot auto-bind, there are no local approvals or
  catalog contents, and a missing baseline AI does not block World creation.
  Confirm an older exact package still exports its own portable config.
- Design verification used local source inspection on Core `1948004` and rtwKit
  `4c0fb23`, including their working-tree AI/Settings/secret changes. These local source checks exited zero. Reproduce
  from the Core checkout with `git rev-parse --short HEAD`,
  `git -C ../rtwKit rev-parse --short HEAD`,
  `rg -n 'missing_required_targets|resolve_component_host' crates/extension-engine/src/artifact_loader.rs`,
  `rg -n 'descriptor.required|resolve_execution_requirements' crates/host/src/templates/{mod,export}.rs`,
  `rg -n 'fn (preference_owner|begin_delegated_task_execution|revoke_runtime_resources_for_owner|read_secret)' crates/extension-engine/src/runtime/{wasm.rs,wasm/control_plane.rs}`
  and `rg -n 'fn (register|on_task|config|secret_path)|Appearance' ../rtwKit/packages/{ai-provider/src/wasm.rs,web-ui/src/Settings.tsx}`.
  Existing regression anchors include `test_should_bind_hosted_tasks_to_dependent_worlds_and_revoke_shared_store_on_trap`
  and `test_should_cancel_workers_when_owner_resources_are_revoked`. This was
  design-time evidence; the implementation checkpoint below supersedes that status.

The owner conditioned acceptance on a universal extension mechanism rather than an
AI-specific facility. This boundary is generic: any ordinary WASM extension can
publish its own common non-secret data to its World instances with explicit grants.
It does not give unrelated code or hosted browser frames access to private data.

For another extension, the same declaration is simply:

```toml
[[components]]
id = "backend"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "backend.wasm"
published-preference = "example.shared.v1"
[components.permissions]
runtime = ["published-preference-read"]
```

Its baseline backend uses the existing owner-scoped preferences.set on that key;
its ordinary World backend uses preferences.read-published-preference, with a local
grant. The value is extension-defined UTF-8 up to 64 KiB, not an AI DTO. The two scopes
run the same exact RTW/component. Native errors distinguish inactive callback,
missing grant, oversize value and unavailable source. The existing preferences.error
is unchanged; the new function has its own published-error, preserving older imports.

Keeping a portable endpoint/model suggestion is not part of this contract. A copied
World starts disconnected and uses locally installed/approved shared connections.
The API is shared read-only configuration, not arbitrary World-to-World reads,
a shared mutable World database or a new application-level service bus.

### Historical shared-settings implementation checkpoint

The generic primitive, scoped Settings, shared connection catalog and World-local
inference are implemented. Core and companion rtwKit full offline check scripts
exit zero. Real Chromium acceptance uses synthetic Tavern cards and a loopback SSE
server: 22 story/AI checks plus six independent scoped Settings checks. It covers
explicit origin consent/restart, no network or vault reads on Settings open,
captured in-flight configuration, independent concurrent Worlds, cleanup, greeting
overflow recovery, saved replies and disconnected same-home/clean-home imports.
The canonical URL regression includes a trailing space in the configured base URL;
requests still target the confirmed canonical path. Unit checks reserve transport
space above the AI catalog's 60 KiB encoded limit.

Raw commands, logs, exact source/artifact hashes and browser outputs are retained
at `/home/mart/Desktop/rintawa-evidence/shared-settings-2026-11-23/`. This is local
fixture evidence, not a test of an actual Ollama installation, external provider or
system vault. A separate preview home is prepared without changing the existing
dev home's two Worlds. The remaining headless/exclusion MVP slice is separate.

## Provider discovery amendment — requested 2026-11-23

Status: accepted by the owner on 2026-11-23 (automatic discovery with separate permissions).

Scenario: on Home, add/edit an AI connection, discover available models and select
one without typing its ID; display its last checked connectivity. Reuse the existing
bounded `model_catalog` decoder and ordinary backend HTTP/operations. No Core API.

Change to the earlier no-network-on-Settings-read guarantee: the AI settings frame
may automatically request `GET <canonical base>/models` for saved connections and
for a stable, valid editor URL. Diagnostic work belongs to that scope's AI backend;
baseline can now perform these diagnostic GETs, but still cannot generate. Existing
HTTP permission/origin checks and exact native secret-read grants apply independently
in baseline and each World. No implicit grants, browser key entry, CORS fetch or
use of a World's authority from Home. Missing approval/key is shown explicitly;
it is not described as an offline server. Baseline approvals require Host restart.
A credential-requiring unsaved connection must be saved before discovery so that
its exact existing credential path is known. Keyless drafts can be discovered.

Use a settings-sibling-only `begin-models` operation; normal status/release semantics,
16 retained operation slots, bounded HTTP response 128 KiB / 512 records / 48 choices,
Host timeout and native stop/revoke cleanup. A settings activation owns one active
probe; closing the frame requests release/close; abrupt frame loss is bounded by the Host
deadline. Stop/revocation uses native cleanup. No persistent
health table or background monitor. Auto probes run sequentially while mounted;
refresh is explicit after completion, with no infinite retries or periodic traffic.
Changing URL/key mode invalidates displayed results; stale responses cannot replace
newer editor state. A draft must not inherit credentials from another destination.

Online means the latest GET completed with a valid model list, including an empty
list; it does not promise successful inference. Offline means connection/timeout
failure. Checking, not checked, permission required, key unavailable and incompatible
response are distinct states. Manual model entry stays available for gateways without
inventory. Selecting a model edits configuration explicitly; discovery never saves
or generates. The API-key control says that Bearer authentication is required and
explains native credential setup; Ollama defaults to no key.

Appearance is a shell-owned global setting, reachable identically on Home and in
World Settings. Only extension contributions retain strict selected-scope filtering.
Remove sidebar headings, move Settings to the top action area and remove the duplicate
theme toggle from workspace actions. No per-World theme or preference schema change.

Acceptance: actual browser/real WASM with local synthetic GET inventory, model pick,
empty/duplicate/invalid/oversize responses, denied/missing credentials, no generation
from diagnostics, captured destination and cancellation/close/stop; stale editor
results cannot overwrite current URL. Scoped Settings, theme application and narrow
viewport remain usable. Actual external servers/credentials are not required.

### Discovery implementation evidence

The accepted amendment is implemented in rtwKit without new product Core code.
Reuse `model_catalog::ModelProbe`, ordinary HTTP and existing activation-owned
operation slots. The owner requested automatic replies too: Send's successful COMMIT
precedes one generation attempt, with a fresh availability read when needed; only
an explicit error retry remains as Retry reply. No transcript/restart replay.

The final rtwKit check gate exits zero. Local Chromium suites cover six model
checks (including explicit baseline consent/restart, canonical GET, sorted models,
selection, response failures and stale destination cancellation), seven scoped
Settings/theme checks and 22 story/AI checks. The preview is updated separately
from the unchanged default home. Raw commands/source/RTW hashes and historical
failures are preserved at
`/home/mart/Desktop/rintawa-evidence/provider-discovery-2026-11-23/`.
Actual Ollama, paid providers and real credential-store values remain untested.


## Owner-directed AI settings simplification — 2026-11-23

The owner requested one normal provider settings UI, automatic use of baseline's
main connection by Worlds, Settings inside the existing ellipsis menu, and one
creation permission list without a separate AI access block. This supersedes the
earlier mandatory World binding / Use here / disconnected clone behavior.

The connection catalog carries an optional defaultConnectionId. Adding the first
connection makes it default. Choosing a default is explicit thereafter; deleting
it leaves no default rather than silently switching to another entry. Saved catalogs
without a default remain readable and can choose one in baseline Settings. A World
without local settings resolves the current published default at request admission.
No binding, destination/model copy, migration or generation replay is needed.
Already admitted work retains its snapshot. Missing publisher, permission or primary
is explicit; native HTTP, origin and exact credential approvals still apply. A new
primary destination is not an implicit native permission grant. Exports carry no
local connection catalog, key or approvals; fresh Worlds use this Host's settings.

The owner clarified that AI edits made in any World are global. There are no local
catalog copies, bindings or World editor mode. Other extensions can keep ordinary
local preferences or opt into their own published data; future cross-extension groups
are not implemented or implicitly authorized by this change.

Use one generic preferences.compare-exchange-published-preference(expected,value)
operation, each argument an optional raw string bounded to 64 KiB. The verified
baseline owner writes its own declared published key under existing owner authority;
an ordinary exact RTW/component World counterpart additionally needs manifest-requested,
locally granted published-preference-write. Resolve the key/source natively as for
the existing read; no guest-chosen foreign owner, key, scope or digest. Separate
published-write-error preserves the old read ABI. None deletes; conflict never writes.

Capture and revalidate exact active ordinary contexts and the World write grant at
native admission, then release live locks. The baseline profile lock serializes
Home/A/B writes, reloads the exact enabled persisted owner/digest (rejecting staged
replacement), compares the raw previous Option<String>, applies existing quotas and
atomically saves. Already admitted synchronous writes may finish after stop/revoke,
like existing atomic Host writes; never retarget a replacement. A postcommit state
change must not turn known success into failure. Persistence unavailable/unknown is
reconciled by a fresh read, never an automatic replay. Concurrent UI edits validate
expected entries/defaults too, preserving drafts and reporting an explicit conflict.

Both baseline and World AI editors use the same CAS path; otherwise Home's old
get-then-set could overwrite a successful World edit. Keep the exact raw JSON from
load for CAS, including whitespace/omitted optional fields. World HTTP/model probes,
generation, keys and operation cleanup remain under their own executing authority.
Adding the first provider makes it primary; later primary changes affect future
admission in every World. Catalog edits do not grant native network or key access.

Acceptance: fresh World imports a Character and generates through the baseline
primary without opening World AI Settings; primary changes affect subsequent
requests without restart; earlier requests retain their captured model; no automatic
fallback on default deletion; denied destinations/credentials stay denied. Confirm
one creation list and ellipsis Settings on Home/World with real browser/WASM.
