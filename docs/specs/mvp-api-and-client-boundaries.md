# MVP API and client boundaries

**Status: accepted design, 2026-11-21; not implemented.** The owner accepted D1–D7
on the condition that UI uses general extension interaction and adds no UI-specific
Core code. A subsequent browser review is recorded below as D8 (proposed amendment;
not covered by the earlier acceptance). The role flag from the draft is therefore replaced by the generic
existing-contract lookup below. Architecture work P01–P04 is complete. This document refines
[D1–D5](core-mvp-contract.md), adds D6–D7 below, and selects the first browser/native
scope. Where the earlier draft left alternatives open, the choices here supersede
them. No product code, builds, tests or performance measurements accompany it.

Acceptance covers this design and its stated MVP limits; it is not evidence of
correctness, minimal implementation or release readiness. New mechanisms still
need the concrete requirement and acceptance checks specified here.

Core baseline: `fb1072a2c9169af1b36ea9cdffed31c146ed0341`; rtwKit baseline:
`77782727020f6066be4d4c205b077dd3f9555775`. Recheck relevant source before coding.
This is a contract for a local single-operator MVP, not a stable universal ABI.

## 1. Authority and atomicity: selected scope

### Three different authorities

1. Host admits an exact artifact instance/component and its runtime capabilities.
2. D1 binds property names to an exact instance and commands to an exact component.
   A property owner can change its properties on any item; topology remains shared.
3. A domain owner decides which client may request which operation on which item.
   This rule belongs at the trusted operation handler/System, not in client UI.

Core contains no Player, Character, inventory, memory or narrative ontology.
Loading an extension as a schema owner makes it trusted for those property names.
It is not an item-restricted agent. An agent needing limited control acts as a
client of the authoritative domain owner and does not own those schemas.

**D6 supplies authenticated component callers**, so this is implementable without
trusting a JSON `owner` field. A handler defaults to deny for restricted methods.
Read-only public methods may explicitly admit every active caller in their World.
Every mutation path to protected domain data must enforce the same rule, including
alternate transport points; restricting only the primary UI endpoint is insufficient.

For the first product, a domain backend may authorize its own instance's UI
components and deny others. Cross-package access needs an explicit domain rule,
not a match on logical package name. A concrete restricted-control probe grants
caller activation A the domain action `move` on X, denies it on Y, and rejects
payload spoofing, another instance and A after restart. A small owner-held RAM
map suffices for this probe; it is not a new Core grant registry. Durable gameplay
permissions may be owner-protected domain properties, but must never issue Host
capabilities or confer ownership of other schemas.

For the first cross-package flow, use an explicit domain authorization: Chat's
own UI asks Chat to begin one generation for conversation X. Chat creates a
random bearer operation token, scoped to reading the required context and
appending one response with a preallocated message ID. UI passes it to Narrator;
Narrator never receives general write authority. Chat validates it on every call,
binds it to X, and retains the result until operation release/stop (16 operations).
A repeated append with that message ID returns the existing result only when the
domain request matches; another payload is rejected. Import can use the same
library pattern for one authorized import. This is an extension protocol, not
a new Core permission or durable job. Tokens intentionally authorize their holder;
they must not appear in logs or unrelated contribution requests. D6 identity is
still used for the initial owner-UI authorization and explicitly caller-bound rules.

If authorization depends on mutable domain state, check that state during System
evaluation against the pinned snapshot. A trusted owner can include the attested
caller in its own submitted command payload; clients cannot submit that owner's
commands directly. The owner is responsible for preserving the attestation, not
accepting a client-supplied substitute. Already-admitted commands may finish after
caller stop; revocation denies new admission, not retroactive rollback.

**Limits deliberately accepted for this proposed release:** no Host-enforced
per-item sandbox for a schema owner, no authenticated human accounts, no ownership
of graph topology, and no confidentiality promise for item/relation existence.
Untrusted clients cannot bypass the owner's property checks, but an admitted
malicious System can still alter shared empty topology. Applications requiring
stronger topology isolation must not be advertised as supported by this MVP.

### Atomic composition

Use one authoritative owner for each invariant that must commit atomically.
For an economy example, balances and ownership needed for a purchase belong to
one domain authority; its System validates and returns one transaction. Separate
UI, AI, import and presentation packages consume that authority. Code organization
may use ordinary build-time libraries; it does not require separate runtime owners.

| Option | Decision and reason |
| --- | --- |
| One owner for the invariant | Selected: existing snapshot + transaction; no coordination service |
| Sequential calls to independent owners | Allowed only when partial completion is acceptable; not an atomic purchase |
| Arbitrary grants over foreign properties | Rejected for MVP: bypasses the original owner's invariants and complicates revocation |
| Runtime composition of several owners' proposed effects | Deferred: needs ordering, common validation and a precise failure contract; no current Chat requirement |

This is a real limitation: independently installed economy/inventory owners cannot
perform an all-or-nothing joint update under this contract. Do not silently merge
their transactions, hide compensation, or add cross-owner writes to satisfy it.
Cross-World atomic transactions are also unsupported. These decisions avoid
speculative machinery; they do not prove that it will never be needed.

## 2. D6: attested callers and delegated transport

Evidence: [transport](../../crates/extension-engine/src/transport.rs) currently
accepts a scope without a caller in `query`; [WASM imports](../../crates/extension-engine/src/runtime/wasm.rs)
reject delegated transport and use the provider Store's scope in `runtime-context`.
[Target callbacks](../../crates/extension-engine/src/runtime/wasm/target_provider.rs)
already receive the dependent `ComponentContext`; tasks already retain an owner.
Use those existing boundaries, without a second broker or guest-selected scope.

### Exact WIT amendment

Extend `runtime-context` and replace the handler signature in the next incompatible
WIT package `rintawa:engine@0.0.2`:

```wit
// In runtime-context:
record principal {
    instance-id: string,
    component-id: string,
    activation: u64,
}
record context {
    scope-id: string,
    world-id: option<string>,
    principal: principal,
}
// current() keeps its existing signature and error variants.
// Reads the existing resolver's live selected set in the executing scope:
selected-providers: func(contract-id: string, version: u32)
    -> result<list<principal>, error>;

// In transport-handler:
use runtime-context.{principal};
on-query: func(point: string, payload: list<u8>, caller: principal) -> list<u8>;
```

Activation is Host-generated, nonzero, not reused during a Host process lifetime;
restart invalidates every old session/handle/grant. It is an identity discriminator,
not a secret. No new persistent identity file. The runtime context is the executing
callee; the callback argument is the immediate caller. A nested call identifies
the intermediary, not the original caller. There is no ambient impersonation chain.
`selected-providers` exposes the existing
`ExtensionEngine::resolve_active_contract_providers_in_scope` selection, including
preferred-provider policy, in the actual executing scope. It does not add a resolver,
registry, transport router or permission. IDs/versions must pass the existing contract
validation (version nonzero); malformed requests return rejected. Undefined,
unresolved or ambiguous selections return rejected, never arbitrary fallback.
Unavailable context returns access-not-active; internal failure returns unavailable.
Return at most 64 active principals and at most 1 MiB serialized data; exceeding
either returns rejected without truncation. Ordering is stable instance/component.

The lookup is read-only, available only during active ordinary/delegated callbacks;
no guest-provided scope and no registration, teardown or System access. Read a
Host-owned current selection view without recursively locking the running Engine
or invoking providers. A result grants no rights and becomes stale on stop; callers
must not use it to bypass admission. The same primitive works for any selected
backend, importer or renderer. UI-specific flags such as `is-host-shell` are excluded.

The Host stamps caller/scope/activation outside application payload bytes and
validates live membership before provider dispatch. Native transport wrappers must
also require an admitted execution context. Missing/invalid metadata never becomes
an anonymous privileged request. Internal envelope encoding is private; public
Zenoh networking remains disabled. No signatures or network authentication service
are necessary for this closed in-process transport; exposing Zenoh externally would
require a separate security contract. Application payload remains limited to 64 KiB.

All query providers use the changed handler, including fixtures. Existing query,
publish, subscribe, poll and unsubscribe WIT signatures/error variants remain.
Pub/sub is still an unauthenticated application hint, never an authorization or
commit result. Authoritative mutation points must be pinned through existing
`profile.authoritative_providers`; zero replies is unavailable and multiple
successful replies is a configuration/protocol error, never permission to pick one.

### Delegated execution

The Engine records the actual dependent scope, World and activation when starting
a hosted component. Its tasks carry that same bound execution context. Resolve
it from Host state, never from provider-submitted WorldId/instance strings. After
a callback, restore/clear the context even on error. Provider context never leaks
into a dependent task; a provider task never inherits the last dependent's scope.

Enable existing query/publish/subscribe/poll/unsubscribe for those contexts, with
quotas and handles charged to the dependent activation. Reject operations during
registration, teardown, or before activation completes. The accepted D6 baseline
keeps hosted callback-style `declare` and World-data unavailable. Proposed D8 below
adds general polled queryables and nonblocking queries, superseding the consumer-only
Web restriction when accepted. World Systems remain separate backend components.
No `query-in-scope`, arbitrary `run-as`, or new delegated handle API is added.

Apply the corrected executing context to existing permission checks and
`runtime-context.current`, not only transport. Host/admin imports still need the
dependent's own grants. Stop/drop/trap revokes its tasks, subscriptions, network
and upload handles; handles never alias another activation. A provider failure
invalidates all of its dependent clients. Unrelated providers/World backends remain
active; availability of clients sharing one failed provider is not independent.

A target provider is trusted for the components it executes. It can inspect their
shared Store and emulate their actions during their callbacks. It cannot be made
untrusted by these checks. Browser panels must not supply executable code to that
Store; their JavaScript runs in isolated frames.

## D8. Nonblocking transport for hosted and ordinary components

**Status: proposed amendment after browser review; owner acceptance required before
implementation.** D1–D7 retain their recorded status. This amendment replaces the
consumer-only restriction for hosted transport and adds the primitives below.
The browser and DockView sections describe the resulting target design, conditional
on D8. No product code has changed.

A browser contribution must be able to answer an ordinary Zenoh query without an
extra WASM backend. A synchronous call from a hosted task cannot wait for another
panel served by the same provider Store: that Store must return to its pump to
forward the query and receive the response. The same issue occurs for non-browser
execution targets with cooperative event loops. Neither another UI router nor a
browser-only Core callback solves the general boundary correctly.

Expose an asynchronous form of the existing transport and a polling form of a
queryable. Keep existing synchronous query/callback APIs for their current users;
both forms share the same Zenoh implementation, selection, quotas and namespace.
No second service-discovery mechanism or persisted operation state.

```wit
// Added to transport:
use runtime-context.{principal};
record inbound-query {
    handle: u64,
    caller: principal,
    payload: list<u8>,
}
begin-query: func(point: string, payload: list<u8>) -> result<u64, error>;
poll-query: func(handle: u64) -> result<option<list<list<u8>>>, error>;
close-query: func(handle: u64) -> result<_, error>;
declare-polled: func(point: string) -> result<u64, error>;
next-query: func(queryable: u64) -> result<option<inbound-query>, error>;
reply-query: func(handle: u64, payload: option<list<u8>>) -> result<_, error>;
close-queryable: func(handle: u64) -> result<_, error>;
```

- These operations use the current attested activation/scope in ordinary or hosted
  callbacks. Registration, System evaluation and teardown cannot create resources.
  `declare-polled` is available after activation, allowing a mounted browser to
  declare its handlers. It applies exactly the existing authoritative-provider
  admission and concrete-name validation. It confers no World write permission.
- `begin-query` admits bounded work and returns without waiting for replies. Its
  500 ms deadline starts at admission, including dispatch delay. `poll-query` is
  nonblocking: none means pending, some is the completed reply list (possibly
  empty). Terminal success/error consumes the handle. Close releases the result
  receiver; it cannot undo provider side effects. No automatic retry on timeout.
- Each polled queryable retains incoming Zenoh queries in arrival order; `next-query`
  hands out at most one unexpired request without waiting. It supplies the Host-
  attested caller. A retained Zenoh query is the reply route; do not resolve a
  provider again or create an application routing table. `reply-query` sends one
  bounded reply and consumes the handle; none rejects it with a transport failure.
  Multiple replies from one handler are outside the current Rintawa callback model.
- Add `invalid-handle` to transport.error for foreign, closed, expired or unknown
  handles. Full admission/queue returns limit-exceeded; other mappings stay as in
  the existing API. Handle numbers never alias another activation. Close-queryable
  undeclares and rejects all of its queued/outstanding requests; stop/trap does the
  same for all owner resources. A reply after the original deadline is rejected.
- Per activation: at most 16 outgoing requests and 16 incoming queued/checked-out
  requests, each counting completed-but-uncollected state where applicable. Count
  callback and polled declarations together under the existing limit of 16.
  Preserve 64 KiB messages, 32 replies and 512 KiB aggregate replies per request;
  cap retained incoming bytes at 1 MiB and outgoing result bytes at 8 MiB per owner.
  Across the Host allow at most 64 running outgoing queries. Reject overload before
  spawning work; do not create an unbounded thread per browser message.
- Expire retained requests before admission, polling or reply, and at cleanup.
  The original Zenoh query deadline remains authoritative even if an idle owner
  has not polled. An idle owner may retain only the bounded stale records until
  the next cleanup; no background delivery/retry service is required.

The browser SDK uses begin/poll internally to implement a Promise-based query;
it pumps incoming requests to registered JavaScript handlers and sends replies.
Registration and pending handles belong to the actual hosted activation and mount;
closing a panel undeclares its handlers. Backends need not know whether the other
participant is a browser. Direct World System evaluation remains an ordinary WASM
backend responsibility; presentation-only contributions require no such backend.

Alternatives: retaining consumer-only Web forces extra backend packages for simple
contributions; synchronous delegated callbacks deadlock/starve their shared pump;
running a second native remote-api bridge needs the identity/lifecycle integration
compared below. The selected amendment adds seven general lifecycle operations,
not seven UI operations. Implement only after accepting this explicit cost.

## 3. Exact D2/D3 guest interface

Keep current single-item/property operations and JSON encodings. Add the following
to `world-data`; add `read`, `find-relations` and `find-properties` with identical
arguments/results to `world-system-snapshot`, reusing each interface's error type:

```wit
// direction is defined once in world-system-snapshot and used by world-data.
enum direction { outgoing, incoming, either }
read: func(items: list<string>, properties: list<string>) -> result<string, error>;
find-relations: func(
    targets: list<string>, direction: direction, property: option<string>,
    after: option<string>, limit: u32
) -> result<list<string>, error>;
find-properties: func(key: string, after: option<string>, limit: u32)
    -> result<list<string>, error>;

// world-data only:
new-item-ids: func(count: u32) -> result<list<string>, error>;
close: func(ticket: ticket) -> result<_, error>;
```

`find-properties` already exists in world-data; do not add a duplicate. Every
string ID uses the existing UUID parser; generated IDs are UUIDv7. Do not introduce
a new rejection of other valid UUID versions that native readers already accept.
Keys use the existing `name@positive-version` parser. UUID output is canonical.
Arrays accept zero to 64 IDs/keys; repeated IDs/keys are deduplicated, but the
input count is checked before deduplication. Empty selections return empty data,
not all data. Relation targets are ORed and deduplicated. Limits are 1..256;
`after` is an exclusive item/relation ID, independent of whether that row exists.
Results use stable ascending ID order; properties additionally use key order.
Property filters mean exact key presence, not arbitrary JSON predicates.

`read` returns `{"items":[ItemRecord],"properties":[PropertyRecord]}`. Records use
the existing serde forms in [state.rs](../../crates/world/src/state.rs): item
`{id,kind,subject,object}`, property `{item,key:{id,version},data}`. The record key
is an object (existing serde), unlike the `name@version` WIT argument. Each page string is exactly
one such record. `properties-of` returns only authorized records. Missing records
are omitted; unauthorized selected keys fail even when no row exists. A missing
selected version in a batch/page is omitted, not implicitly converted. Preserve the
existing single `load-property` version-mismatch error, mapped to invalid-request;
do not silently change native single-read behavior during this addition.
Check the total serialized result, including arrays/escaping, against 1 MiB;
over-limit responses fail without truncation. Input strings together are also
limited to 1 MiB. Native 8 MiB limits do not override this guest boundary.

One ordinary call uses one snapshot. All System imports share its pinned snapshot.
Pagination across ordinary calls has no historical consistency promise. Host-issued
IDs reserve no rows and confer no authority; allocate them before submitting a
command. No network, random generation or World mutation imports in System evaluate.

Use existing errors: inactive context → `access-not-active`; malformed ID/key/count/
cursor → `invalid-request`; unauthorized key/command → `access-denied`; exceeding
bytes → `message-too-large`; queue/ticket quota → `queue-full`; inaccessible runtime
or storage → `unavailable`. No partial success. Trusted Rust entry points expose
equivalent distinctions through their existing module error enums, without a new
universal error framework.

`submit/poll` keep existing signatures and `pending/committed/failed(string)`.
The failure string is a bounded human-readable diagnostic, not a stable error code;
clients must not parse it. `failed` means definitely not committed. If a lower layer
cannot determine COMMIT outcome, return `unavailable`, never a fabricated `failed`.
A valid terminal poll consumes the ticket; unknown/released/old tickets are invalid.
Close releases a receiver, not an admitted command. Pending is not a busy-loop cue.
Status ownership is the exact activation, even if numeric IDs otherwise match.

The first shared rtwKit SDK helper implements the existing begin/status pattern
once: at most 16 operations per backend activation, no hidden retries, bounded
poll scheduling, terminal result retained until explicit application release or
stop. This is library code over D2, not another Core ticket system. A lost status
reply can be retried while that operation remains live; crash/restart still loses
RAM results. Helpers never promise request deduplication or durable delivery.
The reusable operation wire shape is `begin(payload) -> {operation}`,
`status(operation) -> {state:"pending"|"committed"|"failed", value?, error?}` and
`release(operation) -> null` at owner-defined points. Reserve an operation slot
before submitting; an admission failure releases it. Status may consume the Core
ticket once, then caches its terminal result (complete serialized status reply at
most 64 KiB) in that same bounded
slot. Release does not cancel the command; unknown handles return invalid-request
and other callers cannot poll/release them unless that domain protocol explicitly
uses a bearer token as above. Stop clears all slots. Do not add a general operation
persistence schema.

## 4. D7: two missing primitives required by the browser/import path

### Session entropy

No guest randomness import is present in the inspected WIT. Add to `host`:

```wit
variant random-error { access-not-active, invalid-request, unavailable }
random-bytes: func(count: u32) -> result<list<u8>, random-error>;
```

Use the OS cryptographic source; count 1..256, at most 4096 bytes per callback.
Exceeding count/callback allowance is invalid-request. Available in ordinary active
callbacks and start, including delegated callbacks; unavailable during registration,
System evaluate and stop. No permission or durable storage is needed for bounded
entropy. Never substitute UUIDs, timestamps or deterministic PRNG output for tokens.

### Reading persisted assets

The native [AssetStoreAccess](../../crates/extension-engine/src/host_access.rs) can
read assets; guest WIT currently only imports them. A saved portrait must render
after restart without copying it into a property or using a second asset database.
Add runtime permission `asset-read` and these `asset-store` functions:

```wit
open-asset: func(reference: asset-ref) -> result<u64, error>;
read-asset: func(handle: u64, maximum-bytes: u32) -> result<list<u8>, error>;
close-asset: func(handle: u64) -> result<_, error>;
```

Requires an active permission-checked ordinary/delegated context; never System
evaluate. The ref is the existing digest/size/media-type record. Open verifies the
file once through existing `AssetStore::open_asset` and retains its reader at offset
zero; it does not copy the whole blob. Sequential read maximum is 1..65536; EOF
returns empty. Close releases the reader. Unknown/foreign/old/closed handles and
zero count are invalid-asset; oversized reads are message-too-large. Missing or
size/digest-invalid content is invalid-asset; storage errors are unavailable.
The media type is validated reference metadata, not a stored proof of content type.

Limit eight open readers per activation (quota error: unavailable), charge them
to the actual executing owner, and close them on stop/trap. A browser channel also
owns its readers and closes them on disconnect. Do not reuse numeric handles for
another activation. Reuse [CAS opening](../../crates/artifacts/src/asset.rs), which
hashes the complete file: reopening for each 32 KiB read would repeatedly hash it.
A one-shot full read cannot fit the 1 MiB guest boundary for larger assets. These
three operations avoid both problems without a verified-file cache or a new table.
CAS remains immutable through supported APIs; concurrent tampering by arbitrary
native OS code is outside the active-reader guarantee. A later open verifies again.

`asset-read` explicitly permits reading known exact refs from the installation's
shared asset CAS; it is not World-scoped confidentiality. A digest alone does not
grant this permission. No listing API. Grant it only to components whose asset
access is approved. A client needing item-private media instead goes through its
trusted domain owner; do not give it global asset-read and claim item isolation.
Packages without this permission cannot use the web server as an unrestricted
`/cas/<digest>` proxy. Package static files are a separate exact-artifact view.

## 5. Browser runtime: selected implementation contract

### Browser transport comparison and selected direction

The [official zenoh-ts client](https://github.com/eclipse-zenoh/zenoh-ts) uses a
WebSocket connection to a remote-api plugin or standalone bridge. The
[plugin documentation](https://github.com/eclipse-zenoh/zenoh-ts/blob/main/zenoh-plugin-remote-api/README.md)
describes remotely created sessions, publishers, subscribers and queryables.
This is a viable browser transport, not something unavailable in Zenoh.

| Option | Integration cost in this repository | Decision |
| --- | --- | --- |
| Official zenoh-ts + remote-api | Adds router/bridge deployment or native embedding, version coordination, and mapping remote sessions to current Host activations, World scope, permissions and stop | Do not select for the first local WASM-hosted product |
| Adapter to existing Host-mediated Zenoh | Reuses local Zenoh, existing network/task/target owners and security boundaries; needs bounded browser framing and D8 asynchronous handles | Selected target; all routing/fan-out remains in Zenoh |
| Independent UI RPC/service bus | Duplicates provider discovery, routing and failure semantics | Rejected |

The inference is repository-specific: current transport opens an in-process session
with no network endpoints/scouting, and the existing web runtime is a WASM target
provider. Official remote-api is not a drop-in replacement for that boundary.
This comparison does not claim that upstream cannot enforce permissions or that a
custom adapter is faster. Native/network deployment could change the trade-off.
No benchmark or remote-api integration experiment was run in this documentation task.

Keep the adapter's scope mechanical: authenticate/bind a channel, encode bytes,
forward general operations and clean up handles. It must not interpret Chat/AI/UI
payloads, discover semantic providers itself, aggregate contributions, or implement
retries/durable delivery. The browser SDK exposes the same Rintawa transport model
used by other extensions; it does not pretend to implement all of upstream Zenoh.

### Ownership and reuse

Keep the existing execution-target mechanism and select a rewritten rtwKit
`web-runtime` as the trusted provider of `rintawa.runtime.web-bundle@2`. It owns
one loopback HTTP/WebSocket server and verified bundle bytes. A hosted component
gets one private queue and an existing periodic task. The task invokes imports
under D6's dependent context; the listener task only routes bytes. This reuses
the present task/target lifecycle and avoids one gateway/listener per World.

The component `entry` resolves to a small TOML descriptor containing `entry`
(relative HTML path), `title` (1..128 characters), and optional `host-shell = true`
(default false). Reject unknown fields and paths escaping that artifact. For a
shell candidate the provider declares `rintawa.host.shell@1` during registration;
only the Host-selected live candidate, confirmed by D6 context, becomes the shell.
The provider calls `selected-providers("rintawa.host.shell", 1)` under the hosted
candidate's context and compares the unique result with its attested principal.
Only rtwKit interprets that result as an administrative frontend. Core adds no
shell boolean or web role; the existing Host shell binding is reused unchanged.
No widget capabilities, provider protocols, listen ports or per-bundle HTTP servers
are encoded in this descriptor. Rebuild the first-party packages for target @2;
do not interpret the old web-layer format as this new contract.

The provider records descriptors and bound contexts at activation. Its internal
catalog is not a Core UI registry. It cannot invent a dependent from a browser
WorldId. A trusted root shell lists/mounts active UI components; each panel is bound
to exactly one active component and World. World switch unmounts/remounts it.
The root shell's administrative operations execute under its own approved grants.
A panel cannot name another scope, component or provider in requests.

Alternatives rejected for the first slice: per-World root gateways duplicate
serving/discovery and collapse callers into gateway identity; global `query-in-scope`
gives too much ambient authority; loading arbitrary JS into shell shares its DOM
and credentials. D6 is the smaller shared correction for independently hosted UIs.

### Bootstrap, sessions and frames

Bind only `127.0.0.1` on an ephemeral port. Accept exact `Host: 127.0.0.1:<port>`;
reject other hosts, absolute-form URLs and upgrade origins. No public listener,
cookies or permissive API CORS. Startup produces a 32-byte random, single-use
bootstrap token with a two-minute lifetime and an operator launch URL containing
it in the fragment. The MVP delivery is explicit local console output from the
trusted runtime; that output contains a credential and must not be included in
shared diagnostic logs. Do not log tokens/request bodies in access/error logs.
No automatic OS launch API is added to Core.

The shell removes the fragment immediately with history replacement, connects to
`/bridge`, checks the exact server origin, and sends `{"v":1,"type":"auth","token":...}`.
Require the exact shell Origin on upgrade; reject absent/`null`/other origins.
The first frame must authenticate within five seconds. A valid bootstrap returns a
fresh 32-byte session token and a root-shell channel in
`{"v":1,"type":"ready","token":...,"channel":...}` and is consumed.
Reauthenticating an existing session returns its current token, without creating
another session or rotating a credential underneath a live connection; its new
connection gets a fresh shell channel. Every channel is bound to its session and
connection, not just to a component.
The token is kept only in shell memory and its origin-bound sessionStorage so a
page reload can reconnect; never put it in localStorage, a URL or frame messages.
It expires on
explicit close, Host/provider restart or 30 minutes without an authenticated
connection. Limit four shell sessions. Invalid auth closes the socket and clears
the cached token. Logout clears sessionStorage and revokes that session. No
persistent browser login across a Host restart is promised.

Render independent bundles in `iframe sandbox="allow-scripts"`, without
allow-same-origin, popups, top navigation, forms or downloads. Serve their HTML
with a response CSP sandbox too, so opening it in a new tab does not gain shell
origin authority. Deny API CORS and credentialed requests from opaque origins.
Bundle static responses may use `Access-Control-Allow-Origin: *` without credentials
for module/font loading. They contain only installed package bytes, no secrets.

The trusted shell creates each frame, binds its WindowProxy and a one-use random
handshake nonce to the mounted activation, then transfers a MessagePort after
matching source and nonce. `origin == "null"` is never sufficient. An initial
message to an opaque origin may use `*` only to that exact WindowProxy and contains
no root/session credential. Close the channel on any subsequent load/navigation,
unmount, stop or socket loss; remount requires a new server channel and handshake.
Never accept root-window API requests through global postMessage.

Default panel response CSP: `default-src 'none'; sandbox allow-scripts; base-uri
'none'; object-src 'none'; form-action 'none'; connect-src 'none'; script-src
<exact-bundle-base>; style-src <exact-bundle-base> 'unsafe-inline'; img-src
<exact-bundle-base> blob: data:; font-src <exact-bundle-base>; media-src
<exact-bundle-base> blob:; worker-src 'none'; frame-src 'none'; frame-ancestors
<exact-shell-origin>`. Substitute only canonical server-derived URLs, never bundle
strings. No inline/eval scripts. Permissions Policy denies camera, microphone,
geolocation and other unrequested powerful APIs; fullscreen and workers are outside
this first profile. Canvas/WebGL and ordinary UI remain available within browser
limits. Such frames are not CPU/memory or general data-exfiltration sandboxes;
in particular frame self-navigation is not universally preventable with this CSP.
Do not give sensitive data to a panel whose publisher is not trusted to receive it.

This design uses documented [iframe isolation](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/iframe),
[message source/origin checks](https://developer.mozilla.org/en-US/docs/Web/API/Window/postMessage),
and the [response sandbox directive](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/sandbox).
Its actual browser behavior remains an implementation acceptance check.

### Wire API and resource limits

WebSocket frames are JSON. MessagePort uses the same request/result objects without
`channel`; the trusted shell inserts the server-issued channel bound to that port.
All u64 handles are decimal strings in browser JSON; no JS Number rounding.

```json
{"v":1,"id":"r1","channel":"opaque","method":"transport.begin-query","args":{"point":"chat/read","payload":"BASE64"}}
{"v":1,"id":"r1","ok":true,"value":"42"}
{"v":1,"id":"r1","ok":false,"error":{"code":"unavailable","message":"Provider unavailable","outcome":"unknown"}}
```

IDs are nonempty strings of at most 64 ASCII characters, unique among pending
requests on a channel; they correlate replies and do not deduplicate mutations.
Unknown version/method/extra authority fields or invalid types → invalid-request
before dispatch. Bytes are base64. Errors use the invoked WIT variant names plus
invalid-request, channel-closed and protocol-error. `outcome` is not-started only
when definitely rejected before invocation, otherwise unknown for an unsuccessful
exchange; a transport success is not a World COMMIT. Domain status supplies that.

Panel allowlist, with WIT argument names converted to camelCase:

- `transport.begin-query/poll-query/close-query/declare-polled/next-query/reply-query/
  close-queryable` (D8) and `publish/subscribe/poll/unsubscribe`: exactly the
  corresponding WIT arguments/results; subscription handles are channel-owned as well as
  activation-owned. Query is raw bounded fan-out; the domain SDK enforces selected
  single-owner response semantics for mutations.
- `user-resources.begin-upload/append-upload/finish-upload/cancel-upload`: existing
  signatures, channel-owned handles and dependent permissions. This supplies an
  exact user-selected resource ref to the domain importer; the backend reads and
  releases it through existing APIs. Do not put PNG/RTW bytes in a transport payload.
- `asset-store.open-asset/read-asset/close-asset`: D7, own permission and
  channel-owned readers; browser requests at most 32 KiB per read. Render raster media via blob URLs;
  revoke them on unmount. Never serve arbitrary imported HTML/SVG as shell documents.

Root-shell-only allowlist: `world-sessions.list-worlds/create/set-metadata/delete/
set-active` with the existing WIT signatures, plus `ui.list` (active UI descriptors:
opaque id, title from validated metadata, WorldId or null) and `ui.mount({id})` /
`ui.unmount({channel})`. Mount returns channel and frame URL, never a principal
chosen by the caller. These `ui.*` methods exist only in rtwKit. Bootstrap selects
the approved Host shell; a random root-scope UI component is not an admin shell.
Package install/approval remains available through existing CLI; a browser package
manager is not a prerequisite for this MVP. No arbitrary host-import dispatcher.
`session.close({})` is shell-only: acknowledge, revoke that session and all its
channels, then close its connections. Browser disconnect alone permits reconnect;
explicit logout does not. Channel ownership is checked before any method dispatch.

Request frame limit 96 KiB; transport decoded payload/reply remains 64 KiB.
Query result envelope limit 768 KiB accommodates the existing 512 KiB aggregate
reply cap and base64/JSON. Other control requests/replies obey the same envelope
ceiling. At most 64 pending requests/4 MiB queued input bytes per server, 8 pending
per channel, 32 mounted channels total; reject overflow before invocation. Use D8 nonblocking queries; never block a provider task waiting for a browser reply. Drain network input/output
with bounded work; slow sockets may be closed without claiming operation rollback.
Two-second UI request timeout is a waiting policy, not cancellation or an SLA.
Release handles/subscriptions on channel close. An already-admitted command can
still commit. Reconnect creates new channels; reread domain state, do not replay.

Upload append is at most 32 KiB decoded per request; use existing Host streaming
quotas and expected-size validation, cap a user file at 32 MiB. Do not buffer it all
inside the provider Store. Cancel on channel loss; completed resource refs remain
under existing Host lifetime rules until consumed/released. Static package files
use canonical artifact paths, traversal rejection, known content types and nosniff;
provider copied bundle cache is capped at 16 MiB, returning admission failure when
exceeded. A larger product needs an explicit measured cache/read-path adjustment.

### UI authoring

React/TypeScript/Vite for first-party packages; arbitrary ordinary web bundles may
use target @2. Shared build-time tokens and a small shared component library live
in rtwKit. No widget protocol, runtime React federation or Core theme service.
The panel SDK receives `theme` with `{mode:"light"|"dark"}` on its MessagePort at
mount and on change, applies shipped token values, and offers a local palette to
Canvas. No arbitrary CSS is injected from another package. Complex panels render
their own DOM/Canvas; controls and popovers stay within their frame viewport.

### DockView and extension contributions

Use `dockview-react` in the rtwKit Web UI shell for tabs, groups, splits and docking;
do not implement another layout manager. Its [upstream documentation](https://github.com/dockview/dockview)
also describes layout serialization. Pin a compatible release and check its shipped
license when installing; no dependency or lockfile is changed by this specification.
Core, World schemas and transport know nothing about DockView panel IDs or layout.

A shell panel contains the independently mounted extension view, including an
isolated iframe where required. Layout is shell-local state; persist a bounded
serialized layout through existing shell-owned preferences, without another table.
Store stable composition-selection/component and view identifiers, World identity
and user layout; ambiguous matches must produce a placeholder, never a guessed owner. Never persist
session tokens, channel IDs or activation handles. On restore resolve current
approved components again; a missing/disabled extension gets a removable placeholder,
not automatic installation/activation. Validate stored layout and fall back to the
initial workspace on malformed data. Selection of a World does not grant authority.

Dragging/resizing should preserve a panel's live frame where DockView integration
allows it; if it remounts, explicitly revoke/recreate its channel. Hidden tabs need
not lose their backend; closing a panel releases its frame/queries/subscriptions.
Keyboard focus, resize and light/dark styling need real browser acceptance. Native
popout windows are deferred because their origin/session/cleanup is a separate path;
tabs and split panels inside the shell are included in MVP.

Workspace offerings and toolbar/menu contributions are ordinary owner-defined
query/queryable payloads over Zenoh. Shell/Chat asks its documented contribution
point, validates bounded replies and decides placement. A reply may refer to an
installed view with view parameters; it cannot provide a privileged arbitrary URL,
HTML for shell injection or authority. `ui.list/mount` only enumerate/load approved
installed bytes and bind their channels; they are not semantic contribution discovery.
The shared SDK handles framing, while the consumer owns the contribution format.

Required first example: a JavaScript-only Dice panel declares a contribution
queryable and answers a request from Chat or another panel in the same Web Runtime;
no WASM sidecar is added just to return its menu descriptor. A mutation still goes
through the selected authoritative domain backend and its normal permission checks.

## 6. Native and startup: finite release scope

The supported first product runs the CLI Host plus browser shell. Existing trusted
Rust embedding remains available. External native executable packaging, supervised
process targets, Tauri distribution, Android companions and remote clients are
not part of this release contract; there is no placeholder native IPC implementation.

For a subsequent Tauri application, choose the shell/launcher at process startup:
it owns an embedded Host or an explicitly supervised child. Installing a package
does not replace a running `main()`. Reusing web UI does not automatically authorize
the embedded WebView or its Origin; that platform gets a narrow follow-up contract.
An external process is not automatically an OS sandbox. A trusted in-process plugin
can bypass Host capabilities using native OS access; do not call it confined.

The existing target name and component role are independent; do not create a rigid
Native/WASM feature taxonomy. Keep actual implementations concrete. The first MVP
does not promise persistent local user IDs, Android IPC, a protocol for all languages,
or realtime game replication. These are explicit scope decisions, not unfinished
steps that Sol should invent during P05–P12.

## 7. D4/D5 defaults and completion behavior

Retain D4's OS-backed exclusive writer, full profile mutation lock and validation
of the single supported schema. No new World tables or owner columns. Locks cover
canonical file identity, including aliases; unsupported lock semantics must fail
explicitly. Normal release support is local filesystems on the tested platform,
not shared/network drives or arbitrary concurrent native SQLite writers.

Initial Host policy defaults, to measure at release rather than mistake for capacity
claims: 8 active Worlds; 128 active components including hosted components; 64 MiB
combined linear memory per WASM lifecycle/System pair; 512 MiB aggregate linear
memory across live Stores; existing table/instance/fuel limits retained. Hosted JS
does not receive a fictitious separate WASM memory allocation. Browser memory,
compiler overhead and native buffers are not included in the linear-memory limit.

Retain World queue count 128 and add a total serialized pending-command budget
8 MiB per World, counting queued and evaluating payloads until completion. Retain
16 unconsumed D2 tickets, 1 MiB guest messages, 64 KiB transport messages, 32 replies,
512 KiB aggregate transport replies, 500 ms query timeout, 64 active provider
callbacks, and existing HTTP/network/upload quotas. Bounds may be lowered by Host
policy; changing defaults is not an ABI version change. Document which limit failed.

Each ordinary/task callback has a one-second cumulative blocking-import allowance.
Cap each network connect/query wait to remaining allowance and refuse subsequent
blocking imports when exhausted; no independent full timeout for every iteration.
Fuel still bounds guest computation and is not a wall-clock guarantee. Nonblocking
HTTP runs outside the writer. A cooperative stop waits up to five seconds, then
returns busy/incomplete if work is still active: it must retain ownership/resources
needed by that work and allow later completion/retry, not report success and free
live state. Hard termination of arbitrary native code/OS I/O is not promised.

No new universal scheduler/resource framework is required: extend existing owners,
Store limiters, queues and cleanup. The UI provider is trusted and shared; task
fairness/latency must be measured rather than inferred from these numeric caps.

## 8. Acceptance, alternatives audit and handoff

Keep A1–A11 and add the following required checks to the affected implementation
slices. The checks below are planned evidence, not tests run during this design.

| ID | Required observable result |
| --- | --- |
| B1 | Real WASM callers receive distinct attested identities; spoofed JSON, stopped activation and same-name instance cannot impersonate A; nested call identifies intermediary |
| B2 | A restricted domain owner allows A→X and denies A→Y through every mutation entry; forbidden second change leaves no partial transaction; arbitrary cross-owner atomic purchase is explicitly unsupported |
| B3 | Root provider hosting two Worlds dispatches tasks in each dependent scope, never its root or previous scope; subscription/task/upload handles fail across owners and after stop; no hosted World-data/declare access |
| B4 | Independent real frames render form/dialog and Canvas, receive light/dark, cannot use shell DOM/admin session or another channel; navigation/reload revokes ports; opening panel HTML directly stays sandboxed |
| B5 | Invalid Host/Origin/token/nonce, over-limit frames and forged mount IDs fail before privileged invocation; slow/disconnected clients stay bounded and do not cause mutation replay |
| B6 | PNG import uses a resource ref, persists an AssetRef and renders after Host restart through D7; unauthorized assets, corrupt ref, traversal and active-content serving fail; sequential reads are bounded and readers close on disconnect/stop |
| B7 | Missing OS entropy returns error, never predictable tokens; tokens/handles from restart fail; exceeded quotas and failed start release acquired resources |
| B8 | The SDK begin/status helper is used by Chat and one second backend; lost status reply is retryable while live; terminal release, 17th operation, stop and lost commit reply are explicit without automatic mutation retry |
| B10 | Two browser components sharing one provider Store exchange real Zenoh queries/replies without blocking the pump; a JS-only contribution needs no WASM sidecar; timeout, overload, foreign handle and unmount revoke pending resources |
| B11 | DockView restores approved panels and layout without persisting credentials; drag/resize/focus, close, missing component and malformed saved layout behave as specified |
| B9 | Generic selected-provider lookup agrees with existing selection policy for a non-UI backend and the existing shell binding; another scope, ambiguous selection and stopped activation cannot authorize a client; no new UI-specific Core/WIT branch exists |

Critical audit: reuse target tasks, transport, CAS, resource uploads and writer;
add no grant database, job service, router, widget tree or migration layer. D6 is
necessary to distinguish real callers and give hosted UIs their own scope. D7 is
necessary for session credentials and persisted media. If either is removed,
those concrete paths fail. The shared SDK helper avoids repeating operation-lifetime
machinery in each extension. Atomic multi-owner composition is excluded honestly,
not implemented as unreliable sequential writes. No stable-ABI or unlimited-scale
claim is made. Costs retained: trusted provider, bounded in-memory queues, iframe
restrictions, explicit grants and browser reauthentication after Host restart.

Owner acceptance of D1–D7 is recorded above, including the no-UI-specific-Core
condition and B9. All web wire/session/frame/theme behavior remains in rtwKit.
Core provides only general transport, context, lifecycle, permissions, World and
CAS primitives. No parallel UI mutation path or service bus may be introduced.
Implement D6 identity/context with P05, its transport/cleanup
checks through P09, D7 and exact guest reads with P08; realize the browser protocol
in rtwKit P10. D8 requires explicit owner acceptance and is a separate transport
slice after D6 and before P10; it does not block the already accepted D1 correction.
P05 may now start when assigned. Do not make Sol
choose among the rejected alternatives unless evidence shows this contract is
inconsistent or impossible; report a narrow amendment instead.
