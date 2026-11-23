# Live permissions and replaceable UI contracts

Status: both slices authorized by the owner on 2026-11-23 and implemented;
verification checkpoints below distinguish earlier and current evidence. The owner requested these goals in
this order. This document defines the first implementation boundary and records
the second boundary; the owner instructed implementation without repeated confirmation. Better Settings / Better Main Menu are usage
scenarios, not extensions to implement in this task.

## Grounded behavior before slice 1

- `HostHome::grant_runtime_permission`, `grant_http_loopback_origin`, and
  `grant_secret_read` edit local composition under its existing file lock.
  `LocalHostAccess` forwarded administrative requests to these disk-only
  methods. `activate_composition` loads their values into Engine on activation.
- Engine already has shared runtime/origin policy and `SecretManager`; it can grant
  to an existing component. A Host adapter is missing; restarting is not required
  by capability isolation. Do not recursively lock/reenter Engine from a guest import.
- `web-ui/App.tsx::approveHttp` deliberately stops/starts a World and requests Host
  restart on Home. This is the path to remove after live acknowledgement is proven.
- `Settings.tsx` imports the container directly and filters `ui.list` by
  `settings=true`. This is not a replaceable Settings provider contract.
- Ordinary browser `query/serve` already uses the D8 Zenoh transport under the
  admitted dependent component context. `runtime-context.selected-providers`
  already exposes the current scope's selected providers. Do not build another bus.

## Slice 1: live policy, then one setup consent

Scenario: add Ollama on Home, approve its requested access once, discover models,
select a connection in a World, approve that World's missing access once, and send
a message. Existing Host/World/component activations, mounted panels, and composer
draft must survive each permission decision. Configuration sharing remains separate
from permission to transmit World data or read a credential.

### Accepted observable contract

1. An online administrative decision captures exact scope, instance, component,
   verified RTW digest and live activation. The Host validates manifest requests
   and that same live identity when applying it. Stop, replacement, or a different
   selected artifact invalidates an outstanding decision; it never retargets.
2. Successful acknowledgement means the requested local policy was durably saved
   **and** made effective for the captured live activation. The next admitted
   operation uses it, without browser/World/Host restart. A saved-only change must
   not masquerade as success. Disk/live failures and unknown acknowledgements are
   explicit; reconciliation reads state, never automatically repeats an operation.
3. Storage and live policy do not become one invented cross-system transaction.
   Validate a complete change before mutation, serialize application with lifecycle,
   and fail closed on incomplete application. Define/report persisted versus effective
   state when failure prevents both from agreeing; do not silently enable a new
   activation while repairing the old one. No policy journal or background job system.
4. Runtime permissions, exact loopback origins and exact secret-read grants use
   their existing respective owners. No AI endpoint/model/Settings concepts in Core.
   A live revocation denies new relevant operations before success is returned.
   Existing ownership, sandboxing, manifest ceilings and scope isolation remain.
5. HTTP/origin revocation cancels affected native requests and prevents returning
   queued response bytes, including when the guest does not poll again. Other
   components, Worlds and still-authorized origins continue. Already transmitted
   bytes and remote side effects cannot be recalled. Exact-owner close remains
   usable for cleanup; polling a revoked request may report failure, never data.
   Own application status/release does not grant new HTTP authority or replay work.
6. Secret-read revocation prevents subsequent reads and rechecks authority after
   vault I/O before disclosing a value. It does not erase a key already returned to
   WASM or revoke already sent Bearer authentication. Cancel network access through
   HTTP/origin revocation; do not claim secret-to-request provenance we do not track.
7. Live grants enable future allowed operations. They do not recreate cancelled
   tasks, replay imports, resurrect a trapped component or restart prior work.
   Resource cleanup must be defined per affected capability, not by broadly
   stopping a World or clearing all rights of an unrelated component.

The implementation must settle lock order, partial-save/application failure and
resource cancellation in source and discriminating tests. A public facade or WIT
change is justified only where the current administrative boundary cannot carry
the captured identity and effective outcome. No generic hot-reload subsystem.

### Setup UX

- The selected administrative shell presents one readable capability summary for
  a concrete setup action, with optional advanced denials. It may group generic
  HTTP, one exact local origin, shared-configuration read and an exact secret path;
  their independent grants remain visible and enforceable.
- The extension requests consent, never grants itself rights. Confirmed scope and
  component identity come from Host state, not an arbitrary foreign World in JSON.
- Reuse the chosen connection's existing approvals; ask only for a missing or changed
  destination, credential binding, capability or scope. A label/model edit that keeps
  the confirmed destination/key mode does not need another identical consent.
- Keep current explicit template permission review in this slice. Remembering trust
  for future Worlds/new package revisions is a separate policy decision, not an
  excuse for auto-approving imports. Exported approvals never authorize a clone.
- Native credential entry stays native. This slice does not introduce browser
  secret writes. A generic exact-path approval is separate from storing the value.
- Offline CLI profile editing is not a live Host command channel. Do not claim it
  updates a running Host. If live CLI administration is required, design that path
  explicitly rather than adding an unreviewed watcher or second IPC server.

### Required evidence

- Real browser + WASM, synthetic loopback server: one setup consent -> inventory
  -> chosen World connection -> generation with unchanged activation IDs, mounted
  panels and draft. No `set-active` stop/start or browser navigation to apply grants.
- Missing capability/origin/key read stays denied in B when approved only in A.
  Include two active Worlds using the same target-provider Store.
- Race consent with stop/replacement and same instance ID/different digest; a late
  update cannot authorize the replacement or change its persistent approval.
- Revoke while DNS/connect/body/queued response is pending and while vault read is
  held. Assert immediate admission denial, bounded cleanup without another guest
  callback, no late response/secret disclosure, and unaffected B/other origin.
- Inject persistence failure, unavailable/poisoned policy state, lost acknowledgement
  and simultaneous lifecycle change. Report actual outcome and avoid replay.
- Own close/status/release still frees resources; revoked requests cannot drain
  bytes. Regrant does not revive cancelled work or transfer an old handle.

## Slice 2: ordinary UI protocols over Zenoh

After slice 1, replace hard-coded discovery/composition, not just the sidebar look.

- Settings renderer: one selected extension provider in the executing scope.
  Sections: fanout on an extension-owned versioned point, consumed by that renderer.
  Default web-ui uses these same protocols; installation of an alternative alone
  is not ambient authority or an arbitrary-priority override. Explicit selection
  resolves ambiguity. No renderer inherits administrative grants from the old one.
- Home renderer: a separately selected baseline provider, able to replace the whole
  Home content rather than add cards to the built-in list. World navigation and
  privileged consent remain mechanical shell responsibilities. World lifecycle
  access still requires the replacement's own existing capabilities or a bounded
  intent to the administrative shell; mounting is not a permission grant.
- Appearance becomes an ordinary shell-owned contribution using the same section
  protocol. Preserve one global theme without allowing arbitrary Settings renderers
  to write another component's preferences or choose a foreign World context.
- Chat owns its composer-actions point. Contributors provide bounded data/action
  or view references; invocation executes as the contributor, never as Chat's writer.
  Nested slots belong to the consuming extension. No injected foreign DOM/code,
  arbitrary URL, Core widget tree or semantic interpreter in web-runtime.
- Transport replies must be tied to their verified source/selected principal;
  supplied owner IDs are not proof. View references resolve to the actual live
  component in the admitted scope; stale, duplicate or stopped routes disappear.
- World discovery runs from an admitted World component. Root may mount the
  resulting selected renderer; it may not send an arbitrary query-as-World request.
  `ui.list/mount`, sandboxed frames, theme delivery and transport handles remain
  mechanical. Retire `settings=true` discovery after replacement; no parallel legacy
  discovery path or settings-specific dispatcher in Core/web-runtime.

Before implementing slice 2, specify exact messages, selection/bootstrap and nested
mount rules. One source issue needs a concrete choice: a browser JS component starts
serving queries only when its frame mounts, but Settings must discover unopened
sections. Forcing every JS-only extension to add a WASM backend is not an acceptable
silent workaround. Compare existing backend publication with minimal generic
declarative publication by the execution target; do not add a Settings-only registry
or hidden privileged renderer. This is a consequential design question, not a need
to implement Better Settings/Main Menu now.

Acceptance uses default renderers plus test fixtures, not new product extensions:
two unchanged contributors work with either selected Settings implementation; a Home
fixture replaces the complete Home content without shell source edits; World A/B
and stop/unmount remain isolated. Prove an unopened JS-only contribution is found,
then removed on stop; a forged view/action cannot inherit shell or Chat authority.

### Slice 1 implementation boundary

The existing runtime-policy WIT gains `live-component-policy(scope,instance,component)`
and `apply-live-change(change)`. Both require runtime-policy-write; JSON is strict,
bounded by the existing Host message limit. Snapshot returns an attested target
(scopeId, instanceId, componentId, artifact, activation as decimal string), requested
and effective rights plus separately reported persisted rights. A change is
`{target,delta:{grantRuntime?,revokeRuntime?,allowHttpOrigins?,denyHttpOrigins?,
allowSecretRead?,denySecretRead?}}`, at most 64 total entries; contradictory or duplicate
entries reject. Origins are canonical exact loopback origins; secret paths are exact.
No secret value or AI configuration enters the policy boundary.

Native Host holds its existing composition lock; Engine's shared live-policy control
fences activation retirement/replacement. No Engine mutex, View lock or guest callback
is held across profile I/O. Policy write locks are acquired before persistence;
after successful persistence publication takes no further fallible locks. Failed or
lost acknowledgement requires reading persisted and effective policy; no replay.
Granular revoke does not invoke the broad component-rights sweep.

HTTP cancellation is registered atomically with permission admission. Revoked tokens
remain revoked after a regrant; response polling holds the authority check through
extracting queued bytes. Revocation sets native cancellation immediately even without
guest polling. Host handles/results obey their deadline; workers check cancellation
between blocking phases and retain their existing quota until exit. An OS resolver
or other blocking OS I/O may outlast that deadline: immediate thread termination is
not promised. No guest response can be delivered afterward, and cancellation is
checked before a worker leaving DNS may transmit a new request. Remote work cannot
be undone.
Secret reads compare a per-owner grant revision after vault I/O, so revoking A does
not invalidate B's independent read. The global policy revision remains.

Browser `ui.request-permissions` is a bounded intent for the source instance only.
The selected root shell captures the backend target before displaying consent;
ordinary frames cannot call native apply/inspection. Confirmation groups missing
rights, with optional per-right denial. A bound control-plane notification lets the
requesting frame refresh read-only state without remount/restart or replaying work.
This is administrative control, not the ordinary UI contribution discovery that
slice 2 moves to Zenoh. Existing offline HostHome/CLI writes remain disk-only.

Tasks and loopback sockets are registered under the permission admission lock.
Revoking background-task prevents later callbacks; a callback already admitted may
finish. The Host pump prunes cancelled tasks without entering the guest. Revoking
loopback-listen closes listeners and their accepted streams; loopback-connect closes
outbound streams. Other capabilities/owners survive, and regrant cannot revive old
handles. Exact-owner cancel/close remains usable without the revoked permission.
Passive bounded CAS readers and staging uploads have no autonomous effect: each
read/append/finish rechecks its capability; their existing quota is retained until
explicit close/cancel, activation teardown, or the existing transfer idle timeout.
Regrant can resume these uncancelled passive handles. Already admitted atomic Host
writes/World commands may finish; revocation is not rollback of accepted work.

### Slice 2 implementation design

The following elaborates the authorized second slice. Product acceptance is separate
from the design; the final implementation checkpoint follows below.

**Verified replies.** Add `transport.poll-query-sourced` alongside the raw poll.
It consumes the same query handle/result once and returns provider principal +
payload for each reply. Engine stamps the responder's principal in the native Zenoh
reply attachment; a guest can choose payload bytes, not that principal. Verify the
principal remains active in the caller's scope before exposing sourced results.
Missing/invalid source metadata fails sourced polling; raw polling remains unchanged.
Existing reply count/byte/owner/deadline bounds apply, including metadata validation.
Native callbacks remain trusted code; WASM callback and polled replies are stamped
by Engine, never by a browser. This supplies identity, not semantic endorsement.

**Unopened JS-only publishers.** The web-bundle descriptor may declare bounded
`contributions = [{point, payload}]`. Points obey existing transport key validation;
payload is opaque UTF-8, at most 64 KiB in total and at most 16 entries. The target
admits ordinary polled queryables under the dependent component's context and
answers through existing next/reply imports in that component's task. No iframe
must mount first. Publication failure is explicit and retried only as read-only
registration; stop closes declarations, queued requests and activation ownership.
No persistent contribution registry and no Settings schema in the execution target.
An extension needing dynamic metadata may use its existing ordinary backend instead.
A JS-only section does not need to ship a WASM backend solely for discovery.

**Catalog and selection.** Mechanical View metadata adds decimal activation,
`launchable` (default true), and provided contract keys with a `selected` boolean.
The latter is computed with existing selected-providers in the dependent actor's
own admitted scope. Root uses that catalog to mount a unique selected renderer;
Root never executes transport queries as a foreign World. Ambiguous/missing/stopped
selection produces an unavailable placeholder, without silently falling back to Home.
Retire `settings=true`; embedded views use launchable=false. Old experimental bundles
are not implicitly migrated, and existing Worlds remain pinned to their own RTWs.

**Section protocol, version 1.** Selected renderer contract:
`rintawa.ui.settings.renderer@1`; fanout point:
`rintawa/ui/settings/sections/v1`, request `{"method":"describe"}`.
Each sourced reply is `{"id":string,"title":string,"order":integer}`; strings
are nonempty and bounded (id 64 ASCII bytes, title 128 UTF-8 bytes), order in
[-1000,1000], unknown fields reject. The contributed view is exactly the live View
whose instance, component and activation match the verified response source in
this scope. Do not accept an arbitrary foreign view reference from payload.
Duplicate (source,id) or ambiguous view matches are omitted with an error state;
sort by order, title, then exact source key. Refresh queries re-read current state;
stop removes the route and closes its mounted view. Title and ordering are data,
never HTML/code. Two sections contributed once work with either selected renderer.

**Nested viewports.** A consumer requests a same-scope live View in a bounded slot
and supplies a viewport rectangle relative to its own iframe, plus ordinary bounded
initial state. Root creates and owns the child iframe/port as a sibling in trusted
DOM, clipped to the parent's actual viewport. The renderer receives no child native
channel, nonce, URL, foreign MessagePort or RPC ability. Geometry/resize only move
the bounded viewport; they cannot cover Root navigation or native consent. Source,
parent and target activations are revalidated; closing/stopping a parent closes its
children. Bound slot IDs and mount count use existing channel quotas. This is a
mechanical mount primitive, not a Core widget tree or a semantic Settings dispatcher.
Keep real iframe isolation: a malicious parent must not impersonate its child.

**Home and Appearance.** `rintawa.ui.home.renderer@1` selects an independent baseline
Home renderer. It replaces the entire Home content, uses ordinary own-capability
Host World APIs, and requests bounded navigation/create intents from Root where
needed. It inherits no shell policy permissions. Root retains world rail, workspace
lifecycle, logout and privileged consent. Default Settings/Appearance can be a small
separate UI package installed baseline and in the template, without installing an
administrative shell in each World. Appearance is an ordinary section; its selected
`rintawa.ui.appearance@1` actor may request a bounded global theme change from the
Root owner, never write Root preferences itself. Appearance state remains one global
source of truth. A stopped World exposes no baseline section as a fallback.

**Chat composer actions.** Point `rintawa/chat/composer-actions/v1`, same describe
request and sourced self-view binding. Reply `{id,label,order}` has the section
bounds. Clicking a contribution opens that exact contributor's view with bounded
initial state under its own authority. No arbitrary URL or delegated Chat mutation.
Dice supplies the first concrete action; Better Settings/Home remain test fixtures.
Acceptance includes malformed/forged replies, unopened JS-only publication, source
stop/replacement, cleanup beyond channel quota, nested parent forgery, two Worlds,
and selected replacement renderers working without changes to contributors or Root.


### Historical slice 1 verification checkpoint

Implemented in Core and companion rtwKit. Core `bash scripts/check.sh` and the
companion full check script exit zero. Engine includes 138 unit tests, including
live identity fencing, failed persistence, per-owner secret-read races, native
HTTP queue disposal, and task/socket capability-specific revocation.

Real Chromium + WASM acceptance uses `provider-models-smoke.mjs` (6 checks),
`story-onboarding-smoke.mjs` with `RTWKIT_AI_SMOKE=1` (22 checks), and
`settings-smoke.mjs` (7 checks). Fixtures use synthetic cards and loopback AI, with
no actual credential values. The model test deliberately loses a successful live
policy acknowledgement and verifies exactly one policy write plus reconciliation.
Story coverage includes live shared-catalog recovery, unchanged activation/frame/
draft, separate World approvals, B after A stop, exports and no generation replay.

A historical concurrent stop/write run reached the existing explicit unknown
outcome flow; B remained active/readable and its saved snapshot contained no new
message. The final separate run completed without needing that recovery. No change
promises infallible transport or rollback from absence in a read snapshot. Raw
historical failures and final success logs are retained with the source/artifacts.
Actual Ollama/paid providers, real vault values, and blocking OS DNS timing were
not exercised. Slice 2 remains a design at this checkpoint, not completed UI work.

## Owner-directed creation consent and real local inference — 2026-11-23

The owner requested upfront AI access with the other World-creation permissions,
real local Ollama generation tests, and implementation of slice 2. The template
already requests broad HTTP and published-configuration read; the missing upfront
part is known exact local origins and credential paths. No per-operation consent.

Baseline extensions describe their known creation-access requests through ordinary
sourced fanout at `rintawa/world-creation/permissions/v1`, request `{"method":"describe"}`.
Reply `{componentId,httpOrigins,secretRead,label}` describes access to that exact
package's corresponding component in a new World. Root binds the verified source
artifact to matching template slots, validates the target manifest again natively,
and includes the known destinations/paths in the existing creation consent. This
is not permission inheritance from baseline. New destinations/credentials introduced
later need a delta decision; exports never carry local approvals. AI advertises
its currently configured connections; choosing among those approved connections
later does not open a second permission dialog. The later owner-directed shared
primary amendment below supersedes the original manual selection requirement.

An additive Native/WIT creation method accepts bounded strict JSON:
`{digest,title,approveRuntimePermissions,permissions:[{slot,componentId,artifact,httpOrigins,secretRead}]}`.
It requires world-session-write and runtime-policy-write, checks exact immutable
slot/artifact/component and manifest ceilings, and writes local approvals into the
fresh private composition before atomic World publication. Existing creation and
materialization/export behavior remain; no local approval is stored in the RTW.
Cancelled/invalid creation publishes no World; an unknown acknowledgement must be
reconciled rather than replayed. Runtime discovery carries opaque metadata, not AI
configuration semantics; Core contains no AI names or endpoint defaults.

Real Ollama reproduction using the installed qwen3.5:4b-q4_K_M reached the current
Host HTTP default at about 15.8 seconds. Add generic `http-request.start-with-timeout`
(1..300000 ms), keeping existing start/default and operation ownership/quotas. AI
uses a 90-second generation deadline within Narrator's existing 120-second workflow;
model discovery retains its shorter deadline. Capture one absolute deadline at
admission, check it after DNS before transmission, and use the remaining duration
for network I/O. A long request is still revoked/closed immediately at the native
handle/queue boundary. Tests must cover an actual Ollama reply through the complete
Chat/Narrator/Provider path; a direct HTTP probe or fixture does not establish that.

During repeated actual UI mounts, native transport lock contention was reported as
queryable publication failure. `next-query` now returns pending for unavailable
control-state admission, matching query-result polling; a declaration's recorded
publication failure remains explicit unavailable. A held-lock regression and the
existing publication-failure test distinguish those cases. No query is replayed.


## Current implementation checkpoint

Both slices are implemented in Core and companion rtwKit. Native sourced replies,
atomic creation-time local access, bounded explicit HTTP deadlines, and nonblocking
queryable contention behavior pass the full Core gate. Runtime descriptor publication,
standalone Home/Settings/Appearance, source-verified sections and composer actions,
and Root-owned isolated nested viewports replace the former settings flag path.
The companion full gate passes; subsequent Root consent/navigation changes pass
TypeScript, browser/domain/contribution tests and all web builds.

Real local Ollama 0.40.17 with qwen3.5:4b-q4_K_M reproduced the old ~15.8 s timeout.
A direct longer request exhausted its output in reasoning without visible content.
Ollama connections now default Thinking to Off (reasoning_effort=none), with explicit
Server default available; other provider templates are unchanged. This is a local
compatibility observation, not a guarantee for all OpenAI-compatible implementations.
Actual browser/WASM Chat→Narrator→Provider generation saved an assistant response.
Known AI origins are visible in creation consent and approved on the fresh World
before activation; selecting an approved connection requires no second consent.

Nested UI cleanup covers delayed acknowledgements, slot close/reuse, missing child
bootstrap, parent document retirement, explicit close, stop and repeated mounting
beyond the channel quota. Root clips foreign viewports to trusted visibility and
overflow ancestors. No renderer receives its child's native channel or credentials.
Home navigation is disabled until initial layout loading completes, matching World
navigation, so startup restoration cannot override an accepted early Home click.

Exact commands, raw logs, source/artifact hashes and browser checks are preserved
in the private evidence directory ui-protocols-2026-11-23 and its Desktop copy.
Historical failed/harness-only runs remain identifiable and are not passing evidence.
Existing Worlds remain pinned to their own packages; no experimental compatibility
migration or Package Manager/Better Settings/Home product is included.

Final acceptance: 10 replacement/isolation checks, 6 real local Ollama checks,
6 live-consent/model-list checks and 22 Story/AI lifecycle/export checks pass. The
90-second deadline is exercised with continuous SSE heartbeats; no partial reply
is committed. Browser import approves known destinations anew; a clean-home import
without baseline AI retains no origins, key approvals or connection binding.


## Shared AI settings simplification — owner-directed follow-up

AI edits are shared from Home or any World; Worlds use the baseline primary
automatically. The first added connection becomes primary; later changes are explicit,
and deleting the primary does not choose a fallback. Settings is inside the ellipsis
menu in both scopes. Known origins, exact key reads and shared-write permission
appear in the ordinary creation list under its one existing consent, without a
separate AI block. Model/name edits do not request unchanged permissions again.

The generic additive published-preference CAS/write capability and its exact-source,
raw-value, admission, persistence and unknown-outcome contract are defined in
[the shared configuration specification](extension-settings-and-ai-connections.md).
This reuses current extension preference ownership; no AI data types, global groups,
foreign UI context or privileged proxy is added to Core. Future extension grouping
needs its own explicit contract if required. Per-scope HTTP/credential rights remain.
