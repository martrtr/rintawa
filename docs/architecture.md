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
