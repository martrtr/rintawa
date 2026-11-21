# Minimal Core: boundaries and proposed amendments

**Status:** Accepted design, 2026-11-21; implementation pending. The owner accepted
D1–D7 subject to general extension interaction and no new UI-specific Core code.
**Baseline:** `fb1072a2c9169af1b36ea9cdffed31c146ed0341`.
**Scope:** SDK/WIT, Extension Engine, Host, World Runtime and Storage.
**Evidence:** static source inspection; no builds, tests or benchmarks were run.

**Design completion:** [MVP API and client boundaries](mvp-api-and-client-boundaries.md)
resolves P01–P04, specifies exact guest/bridge interfaces, adds D6–D7 and narrows
release promises. Read both before implementation. The companion records the
acceptance condition and replaces the draft UI flag with general contract lookup.
The companion also contains proposed D8 following the browser review. It is not
covered by D1–D7 acceptance: it enables nonblocking, bidirectional hosted transport.
Neither document proves implementation correctness or release readiness.

## Goal and required scenarios

Keep immutable RTW/assets, sandboxed WASM, exact composition, scoped transport,
four World tables and serialized writes. Close the gaps in identity, commit
acknowledgement, graph reads and resource lifecycle. Extensions own domain schemas,
operations and presentation; Core owns authenticated identity, atomic commits,
consistent reads, resource limits and cleanup.

Required scenarios:

1. An importer parses a user-selected file, stores assets in CAS and sends
   normalized data to the Character owner. The owner creates items, relations
   and properties in one command. No global template library is required.
2. Chat commits a message to a conversation, reads a bounded page through
   relations and reads the saved state again after restart.
3. Narrator obtains application data from Chat/Character, collects contributions
   and calls an AI Provider outside the World writer. Chat saves the completed
   response through its own command. Chat also works without AI.
4. Two Worlds running the same packages do not share data, permissions or handles.
   Stopping one World does not revoke another World's resources.
5. An extension adds its own properties to an existing item and contributes
   through an explicit interface without patching Core or another extension's DOM.

These scenarios guide Core acceptance. Chat, Character, Narrator and browser wire
schemas belong to the extension repository. Core fixtures do not prove a working
product. This is a recommended design after the comparisons below, not a claim
of global optimality or demonstrated release readiness.

Core has no Character, Player, Psychology, Memory or AI concept. These are domain
extensions in the first product. A World is a state/access/atomicity boundary,
not a mandatory fictional-universe ontology. Other applications may reuse these
mechanisms without adopting the first product's domain schemas.

## Existing behavior and ownership

| Owner | Authority and source of truth |
| --- | --- |
| World | Four tables: `world`, `items`, `property_registry`, `properties`; UUID items, graph, property definitions/current values and `world.manifest` |
| World Runtime | Command evaluation on a pinned snapshot, validation of the complete effect, one write queue and outcome after COMMIT |
| Host | Exact artifact selection, scope, permissions and activation/stop; local policy/preferences separate from the transferable World |
| Engine | WASM execution, Host-attested identity, resource limits, scoped transport and execution targets |
| CAS | Verified immutable bytes; a digest is neither object identity nor authorization |
| Extensions | Domain schemas/errors, query points, UI, import, AI orchestration and transient generation state |

Relevant implementation paths are [SQLite schema](../../crates/storage/src/sqlite/schema.rs),
[SQL apply](../../crates/storage/src/sqlite/apply.rs),
[World Runtime](../../crates/world-runtime/src/runtime.rs),
[snapshots](../../crates/world-runtime/src/snapshot.rs),
[Host ingress/lifecycle](../../crates/host/src/runtime.rs),
[WASM adapter](../../crates/extension-engine/src/runtime/wasm.rs),
[WIT](../../crates/extension-engine/wit/engine.wit),
[transport](../../crates/extension-engine/src/transport.rs) and
[Host profiles](../../crates/host/src/profile.rs).

The World schema remains unchanged. Event logs, outbox, durable command receipts,
revision columns, grants stored in properties, persistent local Principal,
projection registries, durable jobs and automatic replay are outside scope.
There is no automatic migration of old experimental RTW/World formats. Hot-swap,
multiplayer, federation and transactions spanning Worlds are not MVP requirements.

## D1. Exact identity and property authority

**Proposed contract amendment.** Current property checks use logical `ExtensionId`
and full versioned `SchemaKey`, whereas SQL replaces values by `(item, name)`.
Command admission already checks an exact component. These boundaries must agree.

- Bind property authority to the exact Host-attested extension instance selected
  by approved artifact composition. Components in that instance share its property
  authority; command submission still requires the exact declaring component.
  A logical `ExtensionId` alone grants no access.
- Keep active bindings in Host/Runtime RAM and rebuild them from approved
  composition. Add no SQLite owner column. Derive caller identity from the
  callback, never JSON, UUID claims or transport point names.
- All active versions of a property name have the same owner. Different owners
  for `x@1` and `x@2` conflict before bindings or schema declarations are published.
- `SetProperty` checks both the incoming key and any stored version it replaces.
  An unbound stored version is not authorized. A legitimate version transition
  explicitly declares the required old and new definitions under the same
  approved owner; this does not introduce an automatic converter.
- Validate the effect of the entire ordered transaction, including repeated
  Set/Remove/Delete operations. The original snapshot alone is insufficient
  when an earlier operation changes the row affected by a later operation.
- Item deletion must not cascade into foreign or unbound properties. Unconditional
  deletion of a character and every independent mod's data is not promised.
  Extension-owned archive markers are possible; no distributed delete protocol
  is introduced here.

Item/relation topology remains shared within a World. An admitted System can
create items/relations and delete empty items; there is no separate item ACL.
This is coarse admission of approved packages, not a multi-user authorization
model. Protecting item existence against every other admitted System requires
an explicit additional decision.

Other extensions obtain domain data through the owner's published API. The
current transport does not supply authenticated caller identity to providers;
active components in a scope can call its points. D6 now proposes attested callers
and domain-owner checks for restricted methods. Until implemented, these points
are public within their World. Never implement per-caller ACL from a claimed
`owner` in JSON. Component identity does not authenticate a human browser user.

## D2. One guest command completion model

**Proposed lifecycle/API amendment.** Keep nonblocking `world-data.submit/poll`,
repair ticket lifetime and add explicit release. Do not add a second synchronous
public guest API in this proposal.

Current `finish_transport_query_execution` calls `discard_guest_execution`, which
clears `world_data_tickets`. A command can therefore continue after the next
callback loses the handle needed to observe its outcome.

| Operation | Contract |
| --- | --- |
| `submit(schema, payload) → ticket` | Checks identity, scope and admission. Success means bounded queue admission; it does not mean COMMIT. Failure before enqueue leaves no queued command |
| `poll(ticket) → pending / committed / failed` | Nonblocking. `committed` follows SQL COMMIT; `failed` is a definite negative outcome. A terminal poll consumes the ticket |
| `close(ticket)` | Releases the receiver/quota without cancelling an admitted command. Closing an unknown handle returns invalid-request |

Tickets belong to an exact component activation in one World. They survive
ordinary callback entry/exit until terminal poll, close or owner/World teardown.
Foreign, unknown and obsolete activation handles are rejected. Handles are not
reused within an activation; restarting a component in the same process must not
make an old handle refer to a new command. Process restart invalidates all old
transport sessions and operation handles.

Keep the existing limit of 16 unconsumed tickets per component, including completed
tickets; exhaustion returns queue-full. Stop/trap releases all owner tickets.
No TTL, receipt cache or persistent polling service is introduced.

Native Host callers may retain the existing `wait`. Guests must not busy-loop.
A provider stores a ticket in bounded application RAM and polls on a later short
status callback or through the existing task pump. Example application protocol,
not an additional WIT abstraction:

```text
Chat.begin-send(item_ids, payload) → {pending, operation}
  → world-data.submit(chat.append@1, payload)
Chat.status(operation) → {pending} | {committed, item_ids} | {failed, reason}
  → world-data.poll(ticket)
```

The extension defines bounds and lifetime for its application operation state,
including retention of a terminal result until its reply. This does not promise
recovery after reply loss or restart. Shared SDK helpers may follow demonstrated
code duplication; a Core jobs framework or second generic ticket protocol is
not justified by this example.

Transport timeout, reply loss or Host crash after enqueue means an **unknown
outcome**, not rollback. Callers reread domain state using known item IDs and
request an explicit user retry if necessary. Neither CommandId nor an operation
handle promises deduplication/exactly-once. Domain deduplication must be an explicit
rule of the particular command.

## D3. Sufficient guest World primitives

**Proposed public API addition.** Expose existing native primitives through the
current interfaces rather than adding a query language. The current ordinary
guest has single reads/property pages; its System snapshot lacks native graph
queries. Host-generated item IDs are also absent from the guest boundary.

| Addition | Required behavior |
| --- | --- |
| `world-data.read(items, properties)` | One snapshot per bounded batch; at most 64 IDs and 64 selected keys; missing records omitted; unauthorized keys rejected before reading |
| `world-data.find-relations(targets, direction, property?, after?, limit)` | Conversation relation page; at most 64 targets/256 rows, exclusive ID cursor and owner-checked optional property filter |
| Corresponding read/find operations in `world-system-snapshot`, including property search | Graph/value validation against the command's already pinned snapshot, without a second reader or network calls |
| `world-data.new-item-ids(count) → ids` | Host-generated UUIDv7 IDs for objects/relations, count 1..64; no row reservation or authorization; unused IDs persist nothing |

WIT DTOs follow existing native records/directions and JSON representation; no
SQLite types escape. Errors are invalid-request, access-denied, message-too-large
or unavailable, without partial batch success. Bound total response bytes before
returning them to the guest. A row limit is not a byte limit. The current 1 MiB
WASM message budget is stricter than the 8 MiB native page budget: use the exact
signatures, encodings and error mapping in the companion API specification.

An ordinary call uses one snapshot for that call only. Cursors across calls do
not promise one historical view; data may change between pages. A System snapshot
remains pinned throughout evaluate. No long-lived snapshot/continuation resource
is added without a concrete requirement for consistent multi-page reads.
A bounded Chat page can be reread after updates.

Generated IDs do not attest ownership. Incoming UUID validation and authorization
remain separate. Trusted native APIs remain available; existing single reads may
be convenience operations on the same authority boundary.

## D4. Write ownership and Host persistence

**Proposed clarification of single-writer/persistence guarantees.** Separate
`SqliteWorldStorage::open` calls currently create separate connections/Mutexes.
SQLite serializes apply transactions, but not the preceding System evaluations.
Host metadata and profile callers also perform separate read/modify/write steps.

One writable owner holds the canonical World file throughout runtime operation,
including snapshot → evaluate → commit. A second runtime/process receives an
explicit busy error before System execution or DB changes. Use an OS-backed
exclusive ownership lock released on close/crash, without TTL leases or tables.
Read-only snapshots do not require write ownership. Reopening storage cannot
bypass it; trusted write methods require the admitted owner.

All active World changes use its existing write queue: domain commands and
Host-only metadata operations. Metadata read/modify/write is atomic at that
writer, preserves other manifest fields and replies after COMMIT. No extension
metadata service/schema is added. For an inactive World, Host temporarily obtains
the same ownership and performs one SQL transaction. Create/delete, readers and
WAL cleanup obey the same lifecycle boundary.

`set-metadata` still replaces the whole title/description/cover block. Two
replacements yield the last block in writer order; fields outside that block
are preserved from the state read within the current transaction.

Serialize the **entire** profile load/modify/save, including policy validation,
across HostHome handles/processes. Use a narrow per-file lock and retain atomic
temporary-file publication. Independent profiles should not block one another
unnecessarily. Preference reads do not rewrite files. Activation-time approval
snapshots remain the model; immediate live revocation is not promised where
restart is required.

Before opening a writable database, validate the actual structure of the one
supported format, including required constraints/triggers/indexes, not just their
names. Unknown structure is rejected without repair/migration. Validation must
accept current-DDL databases after ordinary reopen/backup and must not depend on
incidental `sqlite_master` row ordering.

## D5. Lifecycle, budgets and API removal

**Proposed implementation/ABI changes; accept the scope before implementation.**

Use one lifecycle Store and at most one System Store per exact WASM component.
All its command schemas share the System endpoint. Evaluation receives command
and snapshot, not lifecycle globals; it must not depend on `guest.start` state.
System RAM has no persistence guarantee. Traps return errors and discard invalid
execution state; subsequent commands do not inherit invalid handles.

Account for combined allocations across both Stores/all memories and the number
of admitted components/Worlds. Numeric budgets are Host policy, not ABI. Reject
admission before irreversible allocation/publication. Preserve fuel and HTTP/
transport quotas. Defaults need measurements; eliminating budget multiplication
through additional Stores does not depend on a benchmark result.

Lifecycle invariants:

1. Register collects declarations without trusting claimed ownership. Validate
   identity/schema/policy conflicts before durable publication.
2. Start may prepare resources, but World-data is unavailable until Host finishes
   World activation. Start must not require World reads or data migration.
3. Stop closes admission for new owner World/transport operations. Graceful World
   shutdown drains admitted commands before releasing Stores/handles/tasks/
   subscriptions. Cleanup errors are returned.
4. Crash need not drain the queue. Committed state survives reopen; unfinished
   work is not replayed. Stopping one World preserves unrelated World resources.

Undeclaration does not stop an executing callback. Check activation on new
privileged imports and bound external work. Busy/failed stop must expose incomplete
cleanup, not report success. Use existing owners/quotas/completion paths; no new
scheduler is prescribed. Stop/query races require acceptance evidence. Fuel alone
does not establish a wall-clock deadline for Host imports or trusted native code.

Removals, after checking callers:

- Remove synchronous `http-fetch`; retain `http-request` and shared network
  validators. GET becomes a request; redirects remain forbidden by that API.
- Replace `set-world-default` and its computed compatibility flag with operations
  on the existing scoped default-world recipe in CLI/WIT callers.
- Remove service-budget fields without execution consumers and obsolete API comments.
- Remove generic capability/contribution APIs only if inventory finds no required
  consumer; remove SDK/WIT/DTO/errors/tests together.
- Retain the activation dependency resolver and Host shell binding for now.
  Do not expand them into a second runtime router above Zenoh.

Do not change persistence formats for cosmetic renames. A necessary profile
format change requires an explicit minimal amendment, without automatic aliases
or migrations for retired experimental formats.

## Transport and web boundary

Retain scoped Zenoh. Query/fan-out is short bounded exchange, not an LLM wait.
Provider ordering/completeness is not guaranteed, and timeout does not cancel
side effects. Consumers merge contributions. A public authoritative mutation
point requires a Host-selected provider; an absent selected provider never
triggers fallback. Command admission and transport selection protect different
boundaries.

Pub/sub is a hint to reread, not durable completion. The current interface does
not automatically detect dropped messages. UI refreshes on reconnect/explicit
refresh and queries for required results.

Currently `begin_transport_query_execution` enables transport/World-data but not
secrets, HTTP, assets or admin imports. Retain that mode for the first slice.
An AI Provider/importer may accept bounded RAM work and use an existing task
callback for permission-gated HTTP/CAS operations, registering the task at start.
If the first consumer reveals substantial repeated queue machinery, compare a
narrow permission for the required nonblocking import in on-query. Do not open
the entire control plane or add generic jobs without a separate amendment.

The selected first web consumer is now the hosted web-bundle provider in the
companion specification: one server, isolated panels and delegated tasks with
Host-attested context (D6). This supersedes the earlier per-World root-gateway
recommendation after examining the existing target/task mechanism. Host supplies
the World; browser JSON cannot select an execution scope. The trusted Host shell
uses its own administrative capabilities. React/Vite/TypeScript remain independent
of this security contract. No `query-in-scope` or Core DOM protocol is added.

## Alternatives and total complexity

| Decision | Alternatives | Recommendation and cost |
| --- | --- | --- |
| Data | Four tables / arbitrary JSON store / extension databases | Keep graph constraints, indexed reads and one transaction; other options lose guarantees or duplicate persistence/coordination |
| Identity | Forbid duplicate ExtensionIds / exact instance binding / grants DB | Instance binding reuses existing identity without composition bans or another ACL store |
| Foreign data | Owner API / direct read grants / access for everyone | Owner APIs fit current scenarios; direct grants add policy/revocation. Atomic reads or mutations spanning private owners are not promised |
| Completion | Blocking guest call / existing tickets / durable receipts | Repair existing tickets; blocking a Store on the queue conflicts with the short transport deadline and responsiveness. Durable recovery is not required |
| Routing | Zenoh / direct in-process dispatcher | Keep existing integration; replacing it recreates routing/fan-out/lifecycle without a demonstrated benefit. This is not a distributed-readiness claim |
| Resolver | Remove / activation-only / generic service registry | Retain actual bootstrap/dependency/shell consumers; removal needs replacements, while another runtime registry adds no required guarantee |
| Writer | Ownership lock / SQL write transaction throughout evaluate / revisions and retry | Ownership protects snapshot-to-commit without long SQL write transactions or a retry protocol |
| UI | World-local gateway / existing hosted tasks with attested scope / Core UI framework | Select hosted tasks via D6; avoid duplicate serving/discovery and a Core UI framework |

## Critical audit and known limits

- Removable mechanisms: unused service budgets, duplicate fetch and compatibility
  wrappers after caller replacement. Do not restore already removed subsystems.
- Boundary DTOs/checks may be necessary, but delegate to one authority. Contract
  selection and transport authority have different current responsibilities.
- Cross-world transactions, durable generation, global brokers, universal UI
  schemas, snapshot continuations and hot-swap have no present requirement.
- Extensions retain workflows/schemas/transient operation state; Core retains
  writer, authorization, rollback and identity. Requiring consumers to copy
  these guarantees indicates a misplaced boundary.
- Lost commit replies, version replacement, a second writer, stop races and
  ambient cross-world UI authority are the discriminating failure scenarios.
- Exactly-once external AI/crash recovery requires a durable protocol; reliable
  delivery cannot be obtained from lossy pub/sub. Neither is promised here.
- One writer bounds throughput of one World. A callback occupies its Store;
  trusted native callbacks cannot be safely preempted. Process budgets and
  throughput require measurements. Current scaling means independent Worlds,
  bounded indexed reads and extension growth without feature-specific Core patches.
- Property names/versions and saved values create durable contracts. Future
  packages must choose them; a UI framework or private Rust module layout is
  not such a contract.
- Direct calls are simpler for a single demonstration component. They do not
  eliminate isolation/lifecycle/authorization boundaries for independent WASM
  packages. These amendments do not justify additional crates or registries.

## Acceptance scenarios

These are required future checks, not completed evidence. Reuse existing storage,
property-authority, lifecycle and WASM transport tests where applicable. Record
revision/diff, exact command/toolchain, exit status and raw artifact/log paths.

| ID | Observable result |
| --- | --- |
| A1 | Same-ID foreign instance cannot read; different-version owners cannot overwrite; authorized version transition works; denied ordered/cascade effects roll back completely |
| A2 | A second writable owner, including another process/path alias, is rejected before evaluation; close/crash releases ownership; readers remain available |
| A3 | Concurrent profile mutations preserve independent records; metadata replacements follow writer order and preserve unrelated manifest fields; failed publication leaves a complete previous file |
| A4 | Current-version DB with substituted constraints/triggers/index is rejected before writing; genuine current databases reopen; reads do not repair files |
| A5 | Real guest submit returns pending and survives the next callback to committed/failed; close, quota, stop/trap/reload and reply loss obey D2 without busy-polling or automatic retries |
| A6 | Guest obtains UUIDv7 IDs, creates objects/relations/properties without native seeding, and uses bounded batch/graph reads; System reads its pinned snapshot |
| A7 | Missing records, unauthorized filters, invalid bounds/cursors and oversized responses follow D3; no cross-World access or mixed state within one batch |
| A8 | Multiple command schemas share one System Store; lifecycle state is separate; aggregate allocations/admission are bounded |
| A9 | Stop during callback/HTTP/command closes admission, drains graceful World writes and releases resources without false cleanup success or disrupting another World |
| A10 | Remaining APIs support fresh fixtures; removed ABI is explicitly incompatible; default recipe/pinning and allowed nonblocking HTTP still work |
| A11 | Fresh home, two Worlds, CAS import, guest-created graph, Chat, mock AI outside the writer, saved reply and restart work; lost notification triggers reads and unfinished generation is not replayed |

A11 may be demonstrated by the first new extension vertical slice. A complete
mock product inside Core is not a prerequisite for inspecting or designing rtwKit.

## Acceptance authority and implementation boundaries

### Completed design proposal

The companion specification resolves the previously open decisions: D6 authenticates
component callers and binds hosted tasks to their own scope; domain owners enforce
item-specific operation rules; atomic invariants remain under one owner; the browser
wire/session/frame contract is specified; external native supervision is excluded
from this release. D7 adds session entropy and permission-gated asset readers.
Guest signatures, initial budgets and B1–B11 acceptance checks (including proposed D8) are explicit there.
These accepted design choices do not prove support for all future scenarios.

### Approval and delivery

D1–D7 are accepted within the companion's stated scope and no-UI-specific-Core
condition. Private structures/algorithms/module layout remain implementation choices. Any new public
API, schema, permission, mandatory dependency or weaker guarantee outside the
accepted scope needs a narrow amendment with alternatives and consequences.

Together the documents define public behavior and interfaces; internal algorithms
and data structures remain implementation choices. Verify each slice against
current source. Measure the selected defaults before release; do not silently
replace contracts with a new framework if implementation evidence reveals a gap.

The Core-to-rtwKit handoff requires repaired authority, observable guest commit
completion, sufficient import/read primitives and targeted lifecycle/persistence
checks for that path. Inspect rtwKit read-only earlier to validate consumers.
Product schemas, the specified browser bridge, installation/discovery UX and the full
user workflow are then implemented in rtwKit; a justified shared-boundary correction
may return to Core. Do not wait for every cleanup item to finish before this
integration, and do not treat Core fixtures as a product release.
