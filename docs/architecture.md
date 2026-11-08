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
- generic host capabilities and native World Runtime boundaries;
- no Core-owned UI renderer/protocol after the Portable UI retirement.

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

The World model has **no durable effect/job identity** and no built-in
post-COMMIT signal transport. There is no job outbox, retry table or effect lease.
Any external work must be initiated outside the authoritative transaction through
an extension-owned interaction boundary. No automatic retry or replay is implied.

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

Commands enter a bounded single-writer runtime. The runtime validates each command
payload against its registered JSON Schema before the System evaluates it against a
pinned SQLite WAL snapshot. The System proposes a `WorldTransaction` containing only
an ordered item/property change set. The runtime validates property values against
the persisted vocabulary and applies the changes as one `BEGIN IMMEDIATE` transaction.
Items and properties become visible atomically or not at all; the reply is produced
only after COMMIT.

```text
command
  -> bounded single-writer queue
  -> command JSON Schema validation
  -> System (pinned snapshot)
  -> WorldTransaction
       changes: create/delete item, set/remove property
  -> vocabulary validation
  -> one SQLite transaction (COMMIT)
  -> committed outcome or explicit error
```

There is no durable command ledger: a repeated command is evaluated and committed again,
and no receipt, disposition or commit position is returned. The single-writer queue is
the only ordering mechanism, and it is in-memory; a Host restart loses queued work.

Read-side access uses pinned SQLite snapshots and bounded item/property queries.
Native World snapshots can find values by exact versioned property key using the
existing (name, version, item) index and an exclusive item-ID cursor; pages are
limited to 256 values or 8 MiB of JSON data, whichever is reached first.
Oversized pages return a read error rather than a truncated success. Relation
queries can batch up to 64 target item IDs and
page through deduplicated relation-items, again in ID order. These are direct
read primitives, not the retired Projection service or a new extension transport.
The authorization gate is applied to indexed property searches before querying;
relation structure does not grant access to foreign property values. Deletion
checks now inspect distinct affected property keys through one joined SQLite
query instead of performing one properties lookup per adjacent relation.

Each running System gets a property-scoped snapshot: its trusted command-schema owner
comes from Host-installed runtime declarations, never from the command Principal.
The Runtime accepts active property owner bindings only from Host-installed package
definitions, revalidated against the durable JSON Schema. While those bindings
are grouped by logical ExtensionId, Host rejects two different runtime instances
of the same logical extension that both declare World schemas: otherwise the
instances could inherit each other's property access even with distinct schema
keys. World System calls are pinned more narrowly to the exact declaring
instance/component, including when other same-ID components advertise the same
service contract. This is an explicit MVP restriction, not per-item ownership.
An absent binding denies
read and write even when that definition is stored in SQLite. A System can neither
read another owner's property directly, enumerate its property values, set/remove
that property, nor delete an item if the resulting SQLite cascade would erase
another owner's property on that item or a relation. Rejection occurs before COMMIT.

Structural item/relation changes only enter through an installed, Host-bound
command System. The Host pins the service provider to the owning extension,
never an arbitrary same-scope package; the World Runtime validates the proposal
and SQLite validates relation endpoints and cascades. These are composition and
integrity boundaries, not independent per-item ownership permissions.

Native Host snapshots remain unrestricted and must not be exposed to guests. There
is no per-item owner or full graph ACL in this MVP: an item with no protected
properties is not protected solely by its identity. A World file deliberately does
not persist extension ownership, so future Host approval of an artifact declaring
an identical property key and JSON Schema can establish a new active owner. Exact
artifact admission and composition policy must therefore remain a trusted Host
boundary. No automatic cross-extension property access is granted. The retired
World Projection service transport is intentionally not replaced here.

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

The dedicated World runtime-signal bridge and its subscriptions have been removed.
The separate generic Extension Engine event callbacks remain in-process and do not
imply durable delivery, World-change notifications or replay. After reconnecting,
consumers must re-read authoritative World state rather than assume they received
every notification.

## Content and files

Rintawa uses separate paths for different lifecycles:

- **RTW CAS** — immutable packages and content containers;
- **Asset Store** — immutable binary media addressed by `AssetRef`;
- **UserResourceRef** — bounded ephemeral file ingress for picker/import workflows.

A caller embedding an `AssetRef` in an extension-owned item property must import
and verify the immutable bytes through the Asset Store **before** committing
that property. Arbitrary property JSON is not introspected for AssetRefs by Core;
there is no implicit CAS validation of domain-specific fields or auto-import.
The Host separately verifies the explicitly typed World cover AssetRef before
committing `world.manifest`. Integration tests cover both persisted references
and opening the original bytes after a Host restart.

The retired generic user-content index (`content/library.toml`) is no longer created,
loaded or modified by the Host; pre-existing files are left on disk. The old
`user-content` WIT interface, permissions and `content-*` CLI commands are absent.
Packages such as the previous Character Library that import the old WIT interface
must be migrated and rebuilt before they can run against this reduced ABI.

Large media uses bounded streaming paths rather than embedding unbounded bytes in messages.

## Extension interaction and presentation after the UI cut

Core no longer owns a Portable UI runtime, surface trees, patch models, UI action
routing, UI Layer selection or World presentation/focus. The former `ui-runtime`
crate, SDK UI model, WIT `portable-ui` / `ui-layer` imports and guest action
callbacks, and their Engine/Host wiring have been removed together. The old
platform rintawa.ui.layer@1 role is no longer defined by Host.

This is a deliberate breaking ABI change. Installed RTWs compiled against the
retired UI WIT/SDK must be rebuilt for a future extension-owned interaction
protocol. Core does not supply compatibility shims or claim a working Web UI.
World lifecycle, native authoritative commands, persistent state, CAS and
permission checks remain independent of presentation.

A replacement interaction transport has **not** been integrated. The proposed
Zenoh approach is an experiment that must first prove query/reply, publish/
subscribe, authenticated ownership, World scope isolation, cancellation and
resource bounds on real native/WASM boundaries. UI formats and component
contributions will belong to the consuming extensions, not new Core widget DTOs.

## Standard product composition

The historical standard experience was implemented by rtwKit packages.
These packages are not verified compatible with the current reduced ABI:

| Package | Responsibility |
| --- | --- |
| rtwKit Web Runtime / rtwKit Web UI | Browser transport and standard renderer |
| rtwKit Package Manager | Repositories, dependency review, install/update and scoped package management |
| rtwKit World Manager | World catalog, metadata, lifecycle, focus and creator discovery |
| rtwKit Character Library | Reusable Character templates and Tavern V2/V3 world creator |
| rtwKit Chat | World-scoped conversations, messages, branches and Chat presentation |

The historical creator-extension protocol and its UI need migration; the
reduced Core no longer implements their presentation path.

## World metadata and storage after the MVP cut

A new World uses four authoritative SQLite tables. Its human-facing title,
description and optional verified CAS cover are stored in the JSON
world.manifest metadata object and are committed atomically. There is no
catalog.toml sidecar and no automatic migration of older experimental World
SQLite databases. Listing worlds reads existing state without creating
metadata files or modifying World databases. The previous global user-content
library and World Projection API have been retired; older RTW packages
depending on those APIs are not supported.

The following standard package composition description is historical until
the separate interaction/UI replacement lands.

## Invariants

- Product semantics and renderer-specific behavior stay outside Core.
- Scope boundaries are explicit; there is no implicit cross-scope provider fallback.
- A World can activate without a UI; Core has no UI presentation protocol.
- Extensions do not receive raw filesystem paths, SQLite handles or DOM access.
- Canonical World state changes only through authoritative commands and atomic change
  sets; generic in-process Engine events and external work are not World state.

## Retired guest World command ingress

The deprecated WIT world-commands submission endpoint, its
world-command-submit permission, and Host deferred command queue have been
removed. The previous endpoint returned only an accepted identifier, not the
authoritative commit result. Native World Runtime commands continue to return
errors or successful results after the single-writer SQLite commit; this
refactor does not add a replacement guest transport.

The persistent host-local identity.toml and its fabricated single-user
Principal have also been removed. Component identity and exact artifact/Host
permission approval remain separate from any future human/session identity.
Existing legacy identity files are left untouched, but not loaded.

## Retired World runtime-signal bridge

The World transaction signal proposal, Host signal queue/pump and the matching
SDK and WIT signal subscriptions have been removed together. World Systems now
return only current-state changes, and successful native World command results
contain a command ID after commit. The generic Extension Engine event-subscription
mechanism is retained for existing active component callbacks and cleanup; it is
not a substitute for durable World events and is not a new notification protocol.
A future extension interaction transport requires separate verification.
