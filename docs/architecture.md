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
- generic permissions and bounded runtime capabilities;
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

These operations are **ephemeral** progress transport, never durable World state. The
owner may publish and observe directly, or hand off **one read-only consumer** through an
already authorized, routed service callback.
The Host supplies the immediate authenticated service caller; the producer cannot
select a reader from guest payloads. An opaque observer token is bound to this
exact principal within the same scope. Delegation transfers consumption of the
ordered queue but never grants producer actions, cancellation or completion.
Stopping either party revokes its leases, and producer shutdown releases the
operation itself. Consumer detachment makes the stream available for a fresh
explicit handoff; no fanout or implicit cross-scope discovery occurs. The
observer registry is shared at the ExtensionEngine level, not instantiated
per WASM loader, so distinct provider/consumer components from separate loader
factories in the same Engine can resolve leases correctly. A real two-component
WASM service fixture exercises this boundary, restart, cancellation and failure.

The World model has **no durable effect/job identity**. A committed command may attach
best-effort runtime signals; no job outbox, retry table or effect lease exists. Any
external work a feature wants to perform is started by the extension after it observes a
signal or result, and is never part of the authoritative commit.

## World authority

A World is the authoritative unit of simulation and persistence. One SQLite database
holds exactly one World and exactly four domain tables: `world`, `items`,
`property_registry` and `properties`. There is no fifth table for commands, events,
effects, jobs, cursors or grants.

The state model is deliberately small:

- every object and every relation is an item with its own UUIDv7 identity, stored as a
  BLOB(16) primary key; a relation item points at two object items;
- all feature data is a property value attached to an item by an exact versioned name;
- the property vocabulary is immutable per `(name, version)` and compiles one JSON
  Schema validator per definition;
- property names are flat identifiers such as `character.identity`, never an
  inheritance path; `player`, `npc` and similar are ordinary extension properties with
  no Core ownership semantics.

Commands enter a bounded single-writer runtime. A registered System evaluates a command
against a pinned SQLite WAL snapshot and proposes one `WorldTransaction` containing an
ordered change set and optional signals. The runtime validates the change set against
the persisted property vocabulary and applies it as one `BEGIN IMMEDIATE` transaction, so
items and properties of one command become visible atomically or not at all. The reply
is produced only after COMMIT.

```text
command
  -> bounded single-writer queue
  -> System (pinned snapshot)
  -> WorldTransaction
       changes: create/delete item, set/remove property
       signals: best-effort, not persisted
  -> vocabulary validation
  -> one SQLite transaction (COMMIT)
  -> outcome + best-effort signal dispatch
```

There is no durable command ledger: a repeated command is evaluated and committed again,
and no receipt, disposition or commit position is returned. The single-writer queue is
the only ordering mechanism, and it is in-memory; a Host restart loses queued work.

Read-side feature views use owner-pinned Projection services. A projection provider may
read only the property names it owns; reads of any other name resolve to `None` even when
the World stores a value. Renderer code receives validated projections, not World storage
access.

### Deliberately reduced World guarantees

The four-table model trades durability features for a small, auditable core. It
explicitly does **not** provide:

- a durable command ledger, receipts, dispositions, commit positions or optimistic
  concurrency tokens;
- an event log, event replay or consumer cursors;
- durable jobs, an outbox, effect leases or effect completion records;
- actor records or control grants: authority is Host composition policy, not World
  state;
- more than one World per database, or any cross-World state.

Post-COMMIT runtime signals are best-effort in-process notifications. They are never
persisted or replayed, and a crash between COMMIT and dispatch loses them by design.

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
- Canonical World state changes only through authoritative commands and atomic change
  sets; post-COMMIT signals and external work are best-effort and never canonical.
