# Installable World templates and portable World state

Status: native CLI checkpoint implemented under the owner's story-MVP direction,
updated 2026-11-23. Install/create/stopped export and real-WASM independent forks
have local acceptance below; the installed-template browser chooser and Character-first workflow are now connected.
Disk-backed browser package transfer remains a subsequent slice. This does not grant guests arbitrary database access.
Exact additional wire schemas must be recorded before their implementation.

The end-to-end product sequence and proposed workflow/AI/Host-lifecycle amendments
are in [mvp-story-delivery.md](mvp-story-delivery.md). That plan includes browser
template-file selection and self-contained World export as MVP completion gates.
This document alone is not the complete AI-chat implementation instruction.

## Goal and current evidence

A template creates independent Worlds with an initial composition, optional state,
assets and an extension-owned starting experience. Chat Roleplay, a lore campaign
and a theme collection are data choices, not Core World types.

At the design baseline (before this native template slice), source had four-table World persistence, atomic transactions, snapshots,
asset/artifact CAS, and composition materialization with fresh instance IDs.
`HostHome::create_world` uses one default recipe. `CompositionProfile::materialize_all`
remaps composition-owned references. `SqliteWorldStorage::open` validates current DDL.
The storage test `test_should_accept_current_schema_after_vacuum_backup` covers a
SQLite copy, not a complete portable World. The baseline lacked a complete World export/import
path in Host/CLI; the native implementation below adds it. Arbitrary JSON properties can refer to external
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

## Slice 2 implementation contracts — 2026-11-22

The first CLI checkpoint uses the existing artifact CAS as installed-package source,
with no second template index: enumerate verified stored RTWs and read template
metadata without activating them. A generic `package-install FILE` validates the
container and stores it without changing any composition. Template content also
undergoes descriptor/dependency/seed/resource checks. Missing installed dependencies
are reported by exact digest; no fetch or implicit activation occurs. Existing
`install` remains the explicit extension baseline-selection command.

The archive boundary adds `RtwArchive::copy_to(path, writer, maximum_bytes)`.
It streams one regular entry, rejects declared size before writing, never writes
beyond the lesser of caller and existing per-entry limits, verifies decompression
and final size/CRC and propagates writer failure. The caller owns temporary-file
cleanup; it must not publish a failed partial copy. No archive limit increases.

Storage adds native-only seed validation/creation and consistent file export.
Untrusted seed reads are read-only and use one pinned transaction. Validate exact
current DDL first, SQLite integrity/FKs, object-only relation endpoints, canonical
stored IDs/keys and every property against its registered JSON Schema. Copy into
fresh trusted DDL with a new WorldId while preserving local graph IDs, exact
properties and nontechnical manifest fields. The caller supplies the resulting
manifest. Failure must leave no destination database; an existing file is never
replaced. Source bytes are unchanged and no source triggers/install hooks execute.
Export holds existing exclusive writable ownership and creates a consistent SQLite
copy including committed WAL data. An existing destination is an error. Host owns
private staging and publishes only after validating schema ownership/composition.

Host creates in a private staging directory outside `worlds/`, retains ownership
through DB/composition completion and atomically renames the complete directory to
its fresh WorldId. Lists never see staged content. Creation and cleanup coordinate
on an existing-style stable filesystem lock; a new Host may reclaim abandoned
staging only after obtaining that lock. Stop/export retain composition-before-DB
lock order. No restore/merge operation or approval inheritance is introduced.

### Existing Character AssetRef encoding — proposed clarification

The native AssetRef JSON uses `{digest,size,media_type}` with numeric size. The
accepted Character/bridge records already use `{digest,size,mediaType}` with a
canonical decimal size string. Rewriting those records would change a currently
working schema. Proposed clarification: a static portability declaration selects
one of these two exact encodings per property schema (`native` or `browser-v1`).
The walker recognizes only the selected encoding, validates it through the same
AssetRef/CAS rules and collects bytes without rewriting stored data. World manifest
uses `native`. Neither arbitrary digest strings nor loose mixed shapes qualify.
The owner accepted this clarification on 2026-11-22; archive/storage semantics
remain unchanged.

The owner accepted both explicitly declared AssetRef encodings on 2026-11-22.
The encoding clarification above is therefore accepted; no Character rows are migrated.

### Exact first template JSON and offline declarations

All descriptor field names are camelCase, unknown/duplicate fields are errors at
any depth; nesting is at most 32 and the descriptor at most 1 MiB. `format` is 1.
`components` contains `{slot,artifact,enabled,packageId?,version?}`. Slot labels use
1..64 ASCII letters/digits/`_-`; at most 128 slots, all unique. `artifact` is a
canonical exact ArtifactDigest. Package/version fields are display metadata.
`composition` is `{preferredProviders,authoritativeProviders,requestedGrants}`:
provider selections are `{contractId,contractVersion,provider:{slot,componentId}}`
or `{point,provider:{slot,componentId}}`; a grant is `{slot,componentId,permission}`.
Use existing contract/point/runtime-permission validators and composition bounds.
`entry` is null or `{slot,componentId}`. `hostRequirements` contains exact
`{artifact,componentId,target}` requirements for the local baseline. `seed` is null
or `seed.sqlite`. `assets` contains `{reference: native AssetRef,path}`; distinct
media-type references may share one digest/file. `artifacts` contains `{digest,path}`
for embedded extension RTWs. Arrays omitted from composition/resources default empty.
Canonical RTW relative paths cannot alias descriptors, seed or distinct content.

Offline seed ownership cannot be inferred from JSON Schema equality. An extension
that supports portable World state supplies `world-portability.json` at its RTW
root, bounded to 1 MiB/depth 32, with
`{format:1,properties:[{key,componentId,schema,assetRefs,portable:true}]}`.
`key` is the exact property key, `componentId` its actual declaring component,
`schema` the exact JSON Schema document, `assetRefs` is `native` or `browser-v1`.
The exact selected artifact binds this declaration; duplicate property-name owners
and missing/mismatching definitions reject a seed/export. Carrying schema bodies
is necessary for offline validation without executing guest registration. Packages
must generate this metadata from their existing schema definitions to avoid a
second author-maintained schema. This does not grant runtime authority, override
actual registered schemas or make a dishonest package's dependency claims true.
A package may omit this file when no stored schema is being exported/imported.

Persist the resolved optional entry as generic `{instance-id,component-id}` in its
World composition; materialization remaps it with other composition references.
It grants no permission and does not force an entry's activation. An absent web
view still yields the accepted headless administrative state. Export includes no
Host preferences by default: portable domain settings belong to opted-in World
schemas; credentials and shell layout remain local.

CLI adds `package-install FILE`, `template-list`, `template-info DIGEST`,
`world-create --template DIGEST [--title TITLE] [--approve-runtime-permissions]` and
`world-export WORLD FILE`. The approval flag explicitly approves only this template's
listed World runtime requests, after checking each against its exact manifest.
It never grants baseline permissions. Without it, nonempty requests return a named
approval-required error and create nothing. Exact Host prerequisites must already
be selected and approved locally. Neither install/create nor export starts a World.
Existing `world-create` uses the same staged publisher with the trusted default
recipe; it does not change global defaults around a request.

Exact Host execution prerequisites are persisted as `execution_requirements` in
World composition (`artifact`, `component-id`, `target`). Materialization preserves
these immutable identities. Activation checks both the approved baseline selection
and the actual registered target owner before admitting World components; a changed
Host provider fails instead of falling back. This is the positive binding required
for offline export, not the later World-local negative exclusion mechanism. Worlds
from the previous trusted default recipe have no such recorded prerequisites;
portable export refuses an unrecorded required external target rather than guessing
its provider from a name. Operator composition may supply the explicit requirement.

Lifecycle correction required by stopped export: a native per-World `runtime.lock`
is acquired before activation registers any guest and retained through writer drain
AND all guest/resource cleanup, including failed activation/stop retries. Export and
delete require this same nonblocking lock, then composition, then SQLite ownership.
This closes the interval where a stopped writer alone did not attest completed
component cleanup. Existing stable files are never unlinked while waiters exist.
An execution prerequisite must match the artifact actually loaded by this Host,
not only the current on-disk selection with a stable instance ID.

Generic native CAS imports can precede content-specific validation. Catalog rows
therefore carry an optional bounded `invalid_reason`: malformed template metadata
is shown as unavailable with its exact digest and reason; it does not hide other
valid templates or become selectable. Normal package-install rejects malformed
content before root publication. Embedded dependencies must be declared component/
Host digests, be extension RTWs, pass every payload CRC and match the expected
identity before entering shared CAS; nested templates are not dependency installers.

Template plus embedded dependency expansion shares the existing aggregate RTW
uncompressed-byte/entry ceilings; independent nested archive limits must not
multiply into an unbounded validation pass. Host prerequisite resolution does not
force approval of unused optional manifest capabilities. Actual target publication
and sandboxed callback permissions are checked during activation; a stopped World
can be constructed before that live readiness is known. No callback is invoked
or permission granted merely to inspect/install a package.

Dependency verification is identical for embedded and thin references, including
artifacts already present through an older/generic CAS import. Each distinct digest
is checked once per preparation and consumes the same expansion budget. Implicit
Native Host destruction also retains World lifecycle leases until worker drain and
Engine/proxy teardown finish; dropping the library owner cannot expose the earlier
cleanup/export interval.

## Native acceptance checkpoint

Implemented CLI commands and all Native contracts above are covered by the current
[delivery evidence](mvp-story-delivery.md#slice-2-checkpoint--native-templates-and-portable-snapshots).
The real-WASM integration is `crates/host/tests/template_runtime_test.rs`; invoke it
explicitly with built RTW fixtures as documented in the repository README. The
CLI smoke is `scripts/template-cli-smoke.py`. Browser transfer/onboarding/AI are
subsequent steps; Native acceptance is not a claim that those flows already exist.
