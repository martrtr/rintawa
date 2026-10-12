# Architecture

Rintawa is a local-first runtime for persistent Worlds composed from immutable RTW
packages. Core provides generic execution, storage, authority, composition and UI
infrastructure. Product semantics live in extensions.

## Boundaries

Core owns:

- RTW and asset storage;
- package activation and scoped composition;
- extension execution and service routing;
- authoritative World state, transactions and persistence;
- generic permissions, runtime capabilities and durable effects;
- renderer-neutral Portable UI and presentation routing.

Core does not own Chat, Characters, Narrator, AI, inventory, quests or a specific
renderer. Standard product packages live in
[rtwKit](https://github.com/martrtr/rtwKit).

## Artifacts and composition

RTW artifacts are immutable and addressed by SHA-256. Installation only places an
artifact in the local CAS; activation is a separate decision.

Rintawa keeps three composition layers:

```text
baseline
  global host tools and UI

default-world
  recipe copied into newly created Worlds

world:<WorldId>
  exact composition of one persistent World
```

A World pins exact artifacts, provider policy and runtime permissions. Existing Worlds
do not live-inherit later baseline or default-world changes.

Service resolution is scope-local. A World does not silently fall back to a baseline
provider.

## Extension model

Extensions expose versioned contracts through the Extension Engine.

- `Single` contracts resolve one provider, optionally selected by scoped policy.
- `Multiple` contracts support bounded enumeration and targeted calls through opaque
  caller/scope/contract-bound handles.
- Runtime permissions are explicit and owner-scoped.
- Cross-scope operations require explicit Host capabilities.

Execution targets, services, UI surfaces and runtime effects are revoked with their
owning component lifecycle.

## Outbound HTTP

Core offers two distinct owner-scoped network capabilities:

- `http-fetch`: the existing bounded synchronous HTTPS GET used by current packages.
- `http-request`: a separate permission-gated, nonblocking HTTPS request API with
  methods, validated headers, bounded body, response events and cancellation.

The `http-request` Component Model interface exposes `start`, `poll` and `close`.
The Host runs the request outside the guest callback and returns ordered response
headers, provisional byte chunks, then completion or failure. The guest does not
receive arbitrary sockets or filesystem access. A started request remains bound to
the initiating component principal and is cancelled when its handle is closed or
its owning runtime resources are revoked.

The default outbound request policy permits only publicly routable HTTPS targets,
with DNS destination pinning, no ambient proxy and no followed redirects.
Host-approved HTTP loopback origins are a distinct, exact-origin policy:
a grant for `http://127.0.0.1:11434` does not authorize another port, LAN IP or
other component. Public HTTP and automatic loopback grants are prohibited.
The Host persists exact origin approvals with each composition profile and
restores them during activation only for manifest-authorized component principals.
The CLI and the standard Package Manager runtime-policy interface expose explicit
grant, list and revoke operations. Existing Worlds do not inherit later baseline
changes; copied default-World recipes retain their own exact approvals.
An operator first grants the manifest-requested `http-request` permission, then the
separate exact loopback origin. For example:

```shell
rintawa grant-runtime <instance> <component> http-request
rintawa grant-http-origin <instance> <component> http://127.0.0.1:11434
rintawa list-http-origins
rintawa revoke-http-origin <instance> <component> http://127.0.0.1:11434
```

The persisted policy is reapplied on Host activation. Editing a running Host
profile does not yet hot-reload its active permission managers; an operator
restarts the affected runtime to apply new or revoked persistent grants.

Per-owner and process-wide concurrency, body, response, header, chunk and timeout
budgets are Host-enforced. This transport does not know AI, Chat or World schemas.

## Ephemeral asynchronous operations

The generic **async-operations** Component Model interface provides component-owned
handles and ordered nonblocking progress and data events, followed by an
unambiguous completed, failed or cancelled terminal state. Chunk sequence
numbers advance monotonically. A full bounded queue refuses new updates with an
explicit backpressure error rather than dropping or reordering bytes. The Host
validates exact component ownership for every poll, update, cancellation and close;
handles are unique to the activation, cleaned up on component stop, bounded per
component and per WASM instance, and automatically expire after a bounded idle or
terminal retention interval. Cancellation immediately discards provisional chunks.
Terminal results are small; larger payloads must use separately bounded transports.

These operations are **ephemeral** progress transport, not durable WorldEvents or
World effect jobs. The current phase supports one authenticated component principal
publishing and observing its own operation. Explicit cross-extension service
handoff, observer authorization and consumer-triggered cancellation are still open.
The eventual Narrator durable-effect handler must attach a stable job identity to
the transient operation and apply a separate idempotent WorldCommand on completion.
Core never interprets progress bytes as model tokens or commits provisional data.

## World authority

A World is the authoritative unit of simulation and persistence.

Commands enter a bounded single-writer runtime. A registered System evaluates a command
against a pinned snapshot and proposes a `WorldTransaction`. The Host validates
authority and commits state, events and durable effect jobs atomically to SQLite.

```text
command
  -> System
  -> WorldTransaction
  -> atomic commit
       state
       mutation log
       WorldEvents
       durable effect/outbox jobs
```

`Principal`, `Actor` and durable `ControlGrant` records define who may act for an
entity. Extensions do not receive raw database handles.

Read-side feature views use owner-pinned Projection services. Renderer code receives
validated projections, not World storage access.

## Content and files

Rintawa uses separate paths for different lifecycles:

- **RTW CAS** — immutable packages and content containers;
- **User Content** — versioned logical content such as reusable Character templates;
- **Asset Store** — immutable binary media addressed by `AssetRef`;
- **UserResourceRef** — bounded ephemeral file ingress for picker/import workflows.

Large media uses bounded streaming paths instead of Portable UI text/base64 payloads.

## Portable UI

Extensions publish semantic Portable UI surfaces, nodes and actions. They do not inject
React, DOM or renderer-specific widgets.

A UI Layer renders the visible presentation. One presentation session sees:

```text
its baseline-visible surfaces
+ surfaces from at most one explicitly focused World
```

Focus is presentation state, not World lifecycle. Multiple Worlds may remain active while
one UI session focuses only one of them.

The active UI Layer exposes an O(1), owner-checked presentation generation. It changes
when a visible surface is mounted, patched, unmounted or deactivated, or when that
layer's focus/pending-focus state changes. Mutations in unrelated World scopes do not
invalidate its generation. Web Runtime checks this token and its local management
context revision before rebuilding the full presentation on its idle network tick.
A newly connected renderer still receives a full snapshot during the handshake.
Portable UI admission also enforces three renderer-neutral JSON byte budgets:
768 KiB for one surface snapshot, 768 KiB for the complete visible surface
envelope of each active UI Layer (including its focused World), and 16 MiB
across all mounted snapshots retained in the Host. These values leave space
beneath the current 1 MiB UI Layer Host message ceiling for focus and shell
metadata. The runtime counts serialized UTF-8 JSON bytes, not Unicode characters.
It validates an initial mount and every patched result before publication;
it also checks deferred activation, late UI Layer attachment, and World focus
against the currently visible aggregate. Failed admission leaves previous
snapshots, revisions, and focus untouched. Unmount, deactivation and unregister
release retained byte accounting. User action payloads and streamed media have
separate budgets and never become presentation values implicitly.

A World chooses its initial UI through the platform-owned
`rintawa.world.presentation@1` binding. The selected provider must own the declared entry
surface. World Manager therefore opens a World without knowing whether the World presents
Chat, a visual novel, 3D UI or something else.

## Standard product composition

The standard experience is implemented by rtwKit packages:

| Package | Responsibility |
| --- | --- |
| rtwKit Web Runtime / rtwKit Web UI | Browser transport and standard renderer |
| rtwKit Package Manager | Repositories, dependency review, install/update and scoped package management |
| rtwKit World Manager | World catalog, metadata, lifecycle, focus and creator discovery |
| rtwKit Character Library | Reusable Character templates and Tavern V2/V3 world creator |
| rtwKit Chat | World-scoped conversations, messages, branches and Chat presentation |

World creation is extensible through `rintawa.world.creator@1`. World Manager renders
creator choices; creator providers return capability/results, not foreign UI trees.

## Invariants

- Product semantics and renderer-specific behavior stay outside Core.
- Scope boundaries are explicit; there is no implicit cross-scope provider fallback.
- World focus is presentation state, not lifecycle state.
- Extensions do not receive raw filesystem paths, SQLite handles or DOM access.
- Nondeterministic external work runs after commit through durable effects/jobs; canonical
  World state changes through authoritative commands and transactions.
