# Architecture

Rintawa runs persistent Worlds assembled from immutable RTW packages. The Host
coordinates extension execution, composition, storage, authority and UI presentation.

## Packages and composition

RTW artifacts are validated and stored by SHA-256 digest. Importing an artifact
adds it to the local content-addressed store; activation selects an exact artifact
for a composition scope.

There are three composition scopes:

| Scope | Purpose |
| --- | --- |
| `baseline` | Host-level extensions and presentation |
| `default-world` | Configuration copied when a World is created |
| `world:<WorldId>` | Independent composition of a persistent World |

A World retains its selected artifacts and permissions after creation. Later
changes to `baseline` or `default-world` do not silently modify existing Worlds.

## Extension execution

The Extension Engine loads components through versioned interfaces and manages
their lifecycle. Components can provide services and declare dependencies on
other services.

- `Single` contracts select one provider within the current scope.
- `Multiple` contracts allow bounded provider discovery and targeted calls.
- Runtime permissions are granted to individual components, not implicitly to
  their execution providers.
- Stopping a component revokes its registered services and runtime resources.

WASM components run through the Component Model. Execution-target providers can
supply additional runtimes without changing the extension service model.

## World state

Each World has authoritative SQLite-backed state. Commands are evaluated against
a consistent snapshot by registered Systems. The World runtime validates the
proposed changes before atomically committing state, events and durable effect
jobs.

```text
WorldCommand
    -> System
    -> WorldTransaction
    -> SQLite commit (state + events + effects)
```

WorldEvents describe committed changes. Durable effects handle external work
after commit; follow-up changes return through authoritative World commands.
Projections provide scoped read access for extensions and presentation.

Multiple Worlds can remain active independently. Selecting a World for display
does not change another World's runtime state.

## Content

Rintawa stores different kinds of content according to their lifecycle:

- **RTW store:** immutable extension and content packages.
- **User Content:** logical, versioned user-managed records backed by RTW artifacts.
- **Asset Store:** immutable binary assets referenced by digest and metadata.
- **User resources:** bounded temporary handles for file import workflows.

## Portable UI

Components publish renderer-neutral UI surfaces and actions. A selected UI
layer renders the surfaces visible to its session, including those of the
currently focused World. The Host validates presentation updates and action
dispatch against ownership and revision constraints.

A World's initial presentation is selected through the
`rintawa.world.presentation@1` binding.

## Standard packages

The web runtime, web interface and standard application extensions are
maintained in [rtwKit](https://github.com/martrtr/rtwKit). Their sources and
release artifacts are managed independently from the Rintawa Host.
