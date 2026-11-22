# Installable World templates and portable World state

Status: implementation design under the owner's story-MVP delivery direction,
updated 2026-11-22. This document extends the existing World/RTW contracts; it does
not report implemented export or approve guest access to arbitrary databases.
Exact new wire schemas must be recorded before implementation. No product code is
changed here.

The end-to-end product sequence and proposed workflow/AI/Host-lifecycle amendments
are in [mvp-story-delivery.md](mvp-story-delivery.md). That plan includes browser
template-file selection and self-contained World export as MVP completion gates.
This document alone is not the complete AI-chat implementation instruction.

## Goal and current evidence

A template creates independent Worlds with an initial composition, optional state,
assets and an extension-owned starting experience. Chat Roleplay, a lore campaign
and a theme collection are data choices, not Core World types.

Current source has four-table World persistence, atomic transactions, snapshots,
asset/artifact CAS, and composition materialization with fresh instance IDs.
`HostHome::create_world` uses one default recipe. `CompositionProfile::materialize_all`
remaps composition-owned references. `SqliteWorldStorage::open` validates current DDL.
The storage test `test_should_accept_current_schema_after_vacuum_backup` covers a
SQLite copy, not a complete portable World. There is no complete World export/import
path in the inspected Host/CLI. Arbitrary JSON properties can refer to external
resources; Core currently has no authoritative reference inventory.

World persistence is current committed state, not a transcript or automatic history.
NPC memory/lore exist only if their owners write properties/assets. RAM tasks, AI
requests, transport handles, drafts and external game state are not saved implicitly.

## One container, distinct operations

Use existing RTW format 1 with content `rintawa.world-template@1` and entry
`template.json`. No new archive format or executable installer.

- Install: verify immutable package and dependencies, make an installed template
  available for selection. Does not create or activate a World or grant permissions.
- Create from template: resolve its exact installed revision and create a NEW World.
- Export World as template: package a stopped World's committed snapshot and portable
  composition/resources. Creating from it always forks; it does not resume the same
  WorldId or overwrite an existing World.
- Exact backup/restore of an installation, including its identity and Host policy,
  is a separate administrative operation, outside this format.

An empty recipe and a saved campaign use the same template content type. Optional
seed state distinguishes them. The source template is never live-linked to created
Worlds. Updating/deleting it does not mutate those Worlds or their assets.

## Package contents

`template.json` contains exactly:

- `format: 1`; bounded `name` (1..256 UTF-8 bytes), `description` (0..4096).
- `components`: at most 128 extension-instance slots, each with unique `slot`, exact
  artifact digest, and an optional package ID/version for display. Multiple slots
  may use identical RTW bytes. Slots are package-local labels, not runtime identities.
- `composition`: enabled states, explicit selected-contract/authoritative-point
  bindings and requested runtime grants, all referring to those slots/components.
  Reuse current composition semantics; no second resolver or implicit owner choice.
- `entry`: optional `{slot, componentId}`, selecting an installed web component.
  The shell resolves its fresh identity after creation. Core neither renders nor
  interprets its domain. Missing/wrong target entry is an explicit validation error.
- `hostRequirements`: optional exact artifact/component requirements for shared
  execution-target providers. Resolve against explicitly approved local Host
  composition; installation does not start or replace them automatically. These
  are prerequisites, not World instances or permissions to disable baseline.
  World-local provider exclusions must survive export/materialization and prevent
  implicit fallback to a baseline provider. They never stop that provider globally;
  see the concurrent composition rules in mvp-story-delivery.md.
- `seed`: optional relative path to `seed.sqlite` in the current four-table format.
- `assets`: exact AssetRefs with relative embedded file paths. Each digest appears
  once as bytes; references may have independently validated media-type metadata.
- `artifacts`: optional embedded RTW paths for the exact component digests.

Use one template descriptor limit of 1 MiB rather than increasing RTW root-manifest
limits. Existing RtwLimits (archive/entry/uncompressed size and entry count) remain
upper bounds. Parse/validate with bounded depth (32), field lengths, duplicate-field
rejection and canonical relative paths. Files are streamed through existing verified
CAS import, never concatenated in a WASM buffer. Do not raise limits silently.

Dependencies are digest-pinned at export/build. Semver is authoring/catalog input,
not an import-time choice of different bytes. Package-manager resolves/install plans;
Host verifies exact installed artifacts. Missing packages produce a list to install,
not an implicit network fetch in the World transaction.

Two packaging choices use the same descriptor: thin packages reference installed
exact artifacts; self-contained packages embed them. Every World-owned asset required
by the seed MUST be embedded; a local author's warm CAS must not hide missing content.
A thin package is not advertised as an offline-complete modpack. No recursive template
inheritance, post-install scripts, remote URLs or nested dependency auto-execution.

## Data ownership: the theme example

A theme engine implements theme selection/rendering. A template may seed ordinary
items with theme-owner properties: theme name, palette, chosen theme and references
to background/font assets. Asset bytes live in shared immutable CAS; their selection
and meaning belong to this World's properties. Different templates can supply
completely different theme lists with the same engine RTW.

This requires the theme owner's schemas to be in the selected composition; importing
unknown theme-shaped JSON does not create authority. No Core themes table, generic
mutable config database or special template asset service. Small structured palettes
belong directly in properties; large immutable media belong in CAS. Extension code
and built-in static defaults may remain inside their RTW. Do not move every default
into World data merely because the format permits it.

An additional content pack applied to an EXISTING World is not template creation.
For MVP its domain owner imports the content using normal commands. Generic World
merge, conflict resolution and template inheritance are deferred.

## Portable references: explicit rule, no UUID guessing

A complete export cannot be inferred from arbitrary strings or opaque asset bytes.
The initial portable profile uses these rules:

1. Item references within a World use local ItemId values. Clone preserves all item
   and relation IDs; a reference outside the World is always (WorldId, ItemId).
   New WorldId means cloned items are distinct world-scoped objects. No graph-wide
   UUID regeneration, which would break references hidden inside domain properties.
2. Asset references in structured properties/manifest use the existing exact
   `{digest,size,media_type}` AssetRef shape. A shared export walker collects these
   recursively, validates them and builds the deduplicated file list. A matching
   shape is reserved as a resource reference in portable state, not guessed text.
3. Files inside CAS are opaque. If a theme document or other blob depends on further
   CAS assets, its owner must also expose ALL those dependencies as AssetRefs in
   its structured World state. Plain source documents (including Tavern originals)
   are preserved as bytes; embedded external URLs are not fetched. Preserving a URL
   does not promise that remote content will be available offline.
4. Persistent runtime activation IDs, session handles, credentials, local paths,
   raw composition-instance IDs and embedded absolute self-World references are not
   portable. Domain state needing these requires a deliberate owner conversion;
   exporter must not replace arbitrary strings or execute an untrusted repair hook.
5. Explicit references to OTHER Worlds are not bundled or silently retargeted. MVP
   portable export requires removing them or an owner conversion before export.

Before enabling export for a schema, its exact extension artifact supplies a small
static portability declaration listing supported property schema keys and affirming
these rules. This is package metadata read by the trusted export path, not another
World registry or guest callback protocol. Unsupported owners/schema versions make
portable export fail with a named diagnostic. There is no claim that a declaration
can detect a dishonest extension's hidden dependencies; selected extension owners
remain responsible for their formats. The first Character/Chat schemas must be
explicitly audited under this rule. Runtime permissions are not granted by it.

Alternative: copying the entire installation CAS leaks unrelated data and can grow
without bound. Guessing digest/UUID strings silently loses or rewrites data. Arbitrary
export hooks duplicate infrastructure and require executing plugins during packaging.
A standard reference shape plus explicit supported schemas is the smaller initial
contract. If real opaque formats later require transformations, add a scoped owner
export contract then; do not promise arbitrary plugin-state portability today.

## Export protocol

Require the World to be stopped with no pending cleanup. Acquire composition lock
and exclusive storage ownership in the established order; if busy, fail without
forcing stop. The exporter holds ownership while capturing DB and composition.
Use SQLite's consistent copy mechanism (e.g. the already exercised VACUUM INTO),
never copy just a live WAL-backed main file. Write to a temporary output package.

Include the entire committed graph and property registry. Do not automatically
remove chat history, memories or private text and call the result an empty scenario.
User creates a preparation copy and edits it with domain tools before export if
necessary. Export UI clearly states that all World data is included.

Convert composition instance references to template slots. Include enabled/disabled
required instances and exact dependencies. Host grants become requested grants,
never approvals. Do not export secrets, Host baseline settings, transport resources,
bootstrap tokens, or arbitrary shell preferences. Initial entry is chosen explicitly.

Collect portable AssetRefs from the captured state, verify all content, export the
referenced bytes only. Include manifest cover. Missing/corrupt assets, unsupported
schemas or unresolved providers abort with details. Package publication is atomic;
a partial output is not a valid template. No mutable source World is modified.

## Create protocol and failure semantics

This needs one narrow shared Host creation/import path; every extension must not
implement crash-safe World publication itself. It is an administrative operation,
not a new generic guest SQL/bulk-write capability.

1. Verify template and exact artifacts; inspect requested permissions and selected
   providers. Reuse explicit local approval. A downloaded composition never imports
   authority from its author's machine. No secret values travel in the package.
2. Validate seed SQLite schema with existing allowlist, integrity/FKs, property
   payload schemas and graph invariants. Do not execute embedded DDL/triggers as
   installation scripts. Match registry definitions to the exact approved schema
   owners; duplicate or missing owners reject creation. Schema-valid state is still
   untrusted domain input; owner code must validate invariants before acting on it.
3. Verify/import assets and package CAS bytes. Immutable leftovers after failure
   are acceptable, as for current Character import; no pretend cross-store rollback.
4. Build DB/composition in private staging. Generate new WorldId and fresh runtime
   instance IDs; preserve item/relation IDs and remap only composition-owned fields.
   Replace only technical World identity and user-selected metadata, not arbitrary
   domain JSON. No imported approval files, native executable hooks or auto-start.
5. Publish the complete World under existing lifecycle serialization. No reader may
   observe a World with an unpublished composition. Return identity and resolved
   entry only after publication; starting it is a separate explicit operation.

Crash before publication leaves no visible World; staging cleanup is recoverable.
Crash after publication but before reply is an unknown creation outcome, not licence
to recreate automatically: list/reconcile existing Worlds and request user action.
Existing create-from-default remains a convenience over this path. Never switch a
machine-global default recipe around one request to implement template selection.

Seed rows are a fresh-World initialization, not a bypass allowing a template to write
an existing live World. Unknown storage formats fail; no migration compatibility is
required. Native seed import requires validation of contents beyond current DDL check.

## Entry and roleplay workflow

Shell chooses an installed template and mounts its resolved entry. It never knows
what importing a Character means. Chat Roleplay entry owns onboarding: empty state,
import, linked conversation and resume. More conversations are a drawer, not a
permanent hierarchy or a privileged Core chat. Current own-UI mutation rules do not
permit an unrelated coordinator: use the narrow selected-workflow authority amendment
specified in mvp-story-delivery.md and record its exact domain DTOs before wiring
cross-package onboarding. This template format
neither grants that authority nor introduces a universal workflow engine.

Shared CAS bytes do not imply shared mutable Character memory. Two created Worlds
have independent DBs and composition, even when their initial item IDs/assets match.
No cross-World transaction, multiplayer authority or synchronization is implied.

## Minimal delivery and acceptance

First implementation includes install/list template, create with explicit selection,
empty and seeded templates, exact assets, requested approvals and export of supported
stopped Worlds. CLI/file import first; browser upload of a potentially large modpack
must not be forced through the 16 MiB user-resource path. A streaming administrative
package-install browser contract is separate; do not raise that limit to 1 GiB.
Registry publishing is optional for local file acceptance, not for future usability.
The expanded story MVP additionally requires browser file selection and export UI;
record the privileged streaming transfer contract before implementation, within the
administrative authority and bounded disk-backed transfer rules of that plan.

Required checks before claiming delivery:

- One theme-engine RTW, two templates with different palette/media catalogs; no code
  fork, correct CAS closure after installation into a clean home.
- Export Character/Chat World with history/portrait/source; create twice in a clean
  home and restart. New World/instance IDs, preserved local links, independent edits.
- Verify fresh owner bindings; copied requested grants cannot approve themselves.
- Missing assets/dependencies, unknown DDL/schemas, corrupt payloads, duplicate owners,
  archive traversal/bombs and interrupted publication fail without partial Worlds.
- Export while active/busy is refused; source stays intact. Crash/publish recovery and
  concurrent create use one shared lifecycle implementation.
- Snapshot with runtime-only/unsupported references refuses portable export. No
  unrelated CAS, secrets or shell credentials enter the output.
- Template update/remove leaves existing Worlds unchanged; stale entry gives explicit
  recovery, never fallback to a similarly named component.

## Critical audit and limitations

Necessary additions: one RTW content handler/descriptor, shared validated seed creation,
resource-closure convention and schema opt-in. Each solves a present requirement:
installable recipe, initialized state, missing CAS bytes, and unknowable opaque refs.
No extra World tables, generic object catalog, live template relation, job service,
theme API, migration engine, UUID rewrite framework or installer scripting.

Reuse SQLite snapshots rather than inventing a second complete JSON graph encoding.
Reuse composition rather than inventing dependency/authority resolution. Reuse CAS
rather than copying per-World asset directories. Stopped export avoids a distributed
snapshot across independently changing composition and data. Costs are explicit:
exports require stop, first portability requires owner opt-in, existing arbitrary
plugin state cannot be promised portable, and selected native/schema handling needs
implementation and acceptance. This is a complete initial design boundary, not a
claim to anticipate every future extension format.
