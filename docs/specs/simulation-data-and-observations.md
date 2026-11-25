# Simulation data and observations

Status: proposed extension data contract, 2026-11-25. Requires owner acceptance.
Companion to [simulation experiences](narrative-and-companion.md). All new record
names below are semantic roles, not already registered schemas or a new Core ABI.
The existing four-table storage, ownership and portability contracts remain intact.
The [product foundation](package-and-conversation-foundation.md) precedes
transcript-derived Memory: independent listed chats and immutable shared prefixes
must have stable source semantics first. Typed actor/location/fact examples do not
need to wait for the complete dialogue UI or package repository support.

## 1. Model the invariant before choosing the record

A useful data model answers which fact is authoritative, who can change it, what
must change atomically, how readers find it, and what survives deletion/restart.
Putting arbitrary JSON into four tables answers none of those questions by itself.
Likewise, allocating one RTW or database entity per noun is unnecessary.

| Representation | Use when | Example and limit |
| --- | --- | --- |
| Object item | Independent identity, lifecycle, references or multiple records matter | Actor, location, scene, meaningful memory, unique key |
| Bounded property | A current fact or tightly coupled state belongs to one owner | Location description, actor condition, one memory body |
| Relation item | Public graph adjacency is useful and endpoints are objects | Scene participation, route, public possession |
| ID in protected JSON | A reference is private or only used by its owner | Observer/evidence IDs in a private recollection |
| AssetRef | Immutable bytes need resource transfer and portability | Portrait, model bundle, imported card, explicit recording |
| Derived cache | The value can be rebuilt from canonical records | Observer index, lexical index, embeddings, UI rows |

A property name describes one format/role; it is not an instance key. Do not create
`memory-for-actor-123` schemas. A property on an item is unique by `(item,name)`;
its schema version is not a record revision. Relations have their own ID and
properties, but cannot be relation endpoints. Reify a relationship as an object
only when other objects genuinely need to refer to that relationship itself.

Do not split a small fixed model into an item per float. Do not place an unbounded
conversation, all memories or the whole World inside one JSON property. Use one
record per independently addressable unit, with bounded aggregate fields where
all members share lifecycle and invariants. An inventory stack can be a quantity;
an individual key needs identity if keys differ or are separately transferred.

The following rules are grounded in [DDL](../../crates/storage/src/sqlite/schema.rs),
[property writes](../../crates/storage/src/sqlite/apply.rs),
[reads](../../crates/storage/src/sqlite/query.rs) and
[owner snapshots](../../crates/world-runtime/src/snapshot.rs):

- JSON references are not foreign keys. The owner checks them and defines missing
  reference behavior. JSON Schema alone cannot establish membership or authority.
- Relations are directed and their endpoints/adjacency are shared graph state.
  A private tag does not hide those endpoints.
- Deleting an object cascades to incident relations/properties. The active owner
  must be allowed to delete every affected property, including foreign attachments.
- All active schema versions of one property name have one exact instance owner.
  A matching JSON schema does not grant a different instance access.
- Schema definitions are immutable. Schema-version changes, semantic model changes
  and value revisions are different operations.
- Separate read calls do not form a pinned long-running snapshot. UUID ordering is
  useful for keyset paging, but allocation time is not commit order.

## 2. Identity scope and revisions

Within one World, item ID is logical identity; names are mutable labels. In durable
local references omit WorldId unless the reference is deliberately external. A
World template copy preserves item IDs but creates a new World and fresh local
component identities/approvals. The same local item ID in two Worlds does not
identify one continuously synchronized person.

CAS digest identifies immutable bytes, not an actor, provider or permission.
A service operation token identifies transient work owned by an activation, not a
durable event. A component activation must never be saved as a persistent source
identity. Source contracts and domain IDs identify evidence; the current admitted
provider is resolved and checked when reading it. Replacing that provider without
an explicit data conversion cannot make old evidence trusted automatically.

Use canonical unsigned decimal u64 strings for new counters shared with JS, matching
Chat's existing revision convention. Start new editable records at `"1"`, empty
stream counters at `"0"`, increment with checked arithmetic and reject exhaustion.
No wrapping, floats, leading zeros or implicit rewriting of malformed imported data.
The initial record counter value is not a security boundary.

| Counter | Purpose | Changes on |
| --- | --- | --- |
| Record revision | Optimistic replacement/edit of one record | Meaningful record change |
| Scenario revision | Guard an atomic rules state used to choose an action | Relevant Scenario commit |
| Source sequence | Order committed observations in one source stream | Append of a new source record |
| Source history epoch | Detect edits to earlier source meaning/visibility | In-place semantic correction/deletion or observation-policy change affecting that stream; selecting another chat changes source identity |
| Processor revision | Protect checkpoint/config/correction against stale work | Processing or relevant owner edit |
| Cache source revision | Prove an index corresponds to canonical owner state | Every relevant canonical mutation |

Do not add every counter to every record. They solve different concrete races.
A location with only revision-checked rename needs one revision. An append-only
outcome stream can keep epoch unchanged until an explicit correction/retention
operation changes its evidentiary meaning. A single aggregate Scenario revision
is the simplest first rules guard; split it only after contention measurements,
keeping every invariant's dependency set within one owner transaction.

## 3. Core domain records

The names in this table define responsibilities. Actual property keys and JSON
Schemas are finalized for the accepted slice, generated from the same definitions
used for validation and portability. Optional fields are added only for a consumer
that needs them; this table is not an instruction to store empty placeholders.

| Record | Required semantics | Optional when needed | Writer |
| --- | --- | --- | --- |
| Actor authored description | Stable item ID, name, authored revision, effective description | Source AssetRef, portrait, traits, examples, presentation defaults | Character |
| Location | Name, description, revision | Zone/parent ID, tags, spatial representation reference | Scenario or a chosen geography owner |
| Route | Endpoints, rule/type, revision | Cost, door/item requirement, directional condition | Owner of movement rule |
| Scene | Status, one canonical location binding, revision | Rule configuration, clock binding, presentation references | Scenario |
| Participation | Scene, actor, active/left/archive state | Role, appearance ID, joined/left time when used | Scenario |
| Mechanical actor state | Only variables of the chosen ruleset with units/ranges | Resource maxima, skills, bounded effects | Rules owner |
| Possession | One canonical holder of an individual item, or bounded stack quantity | Equipped slot, condition, transfer constraints | Inventory/action owner |
| Outcome | Stable ID, normalized intent identity, result, rule/config version, logical time | Roll, changed item IDs, bounded visible descriptions | Action owner |
| Observation | Source identity/position, perceiver/view policy, asserted occurrence/content | Actor/target IDs, modality, visible excerpt | Source owner |
| Fact | Subject/value, validity and provenance when independently referenced | Confidentiality policy, correction/supersession | Authoritative domain owner |
| Lore entry | Body, authored revision, selectors, disclosure policy, enabled state | Book membership, title, references, import provenance | Lore |
| Memory | Observer, kind/body, provenance, status, revision | Target, simulation time, confidence meaning | Memory |
| Psychology | Model/config identity, bounded state, evaluated time and coverage | Directed attitudes, descriptive interpretation | Psychology module/owner |
| Processing coverage | Observer/source/processor, consumed boundary, expected revision | Pending/failed diagnostics, configuration identity | Processing owner |

### Locations scenes and spatial representations

A location names a place independent of any camera or scene. A scene names an
interaction/context at a place. Two scenes may share a location without sharing
participants, weather overrides or progress. Do not duplicate a location for every
scene. If a location can contain other locations, define cycle checks and whether
containment determines routing; do not infer one from the other.

For the first public example, use one Scene→Location tagged relation as the
canonical location binding. Do not also maintain an independently editable
`scene.locationId`. A DTO can derive it. Use JSON references instead for confidential
scene topology, with an owner index if required. The chosen representation is part
of that domain schema; do not support two interchangeable encodings at random.

A 2D tile grid, 3D transform/navmesh or external game entity mapping is a separate
extension's representation. It declares units, coordinates, parent frame and which
movement authority it follows. A renderer does not acquire movement rights by
knowing a transform. High-frequency transient presentation frames need not become
one durable World transaction per frame. Durable checkpoints and reconciliation
belong to the concrete spatial simulation; this proposal does not specify a physics
engine or claim the single writer is a real-time physics scheduler.

### Facts evidence and beliefs

Use a direct property for a simple current fact such as gate state. Allocate an
independent fact/event object when its identity, provenance, temporal validity,
visibility or correction history is referenced. Avoid storing every property twice
as both current state and a generic triple record. A committed outcome is deliberate
historical evidence, not a second mutable copy of the current world.

Example: possession says the courier has the key now; an old observation says the
guard saw it in a drawer; the courier says it remains there; the guard believes
that claim. All four records can be valid simultaneously. Moving the key does not
rewrite the guard's recollection. A lie does not write possession. Evidence that a
claim was uttered is not evidence the claim is true. Time and perspective resolve
many apparent contradictions without deleting records.

Lore that describes a mechanical rule cannot silently become a second rule engine.
If the key opens the gate mechanically, that condition is owned by Scenario. A
Lore entry can reference/explain it or remain fictional author context. If authored
text and rule state disagree, the author inspector shows the discrepancy; a model
does not reconcile it by changing mechanics.

### Participation and deletion

For public participation, use one Scene→Actor relation with a Scenario-owned tag
and bounded state. Rejoining can reactivate it; create an appearance object only
when distinct visits must be separately referenced. Secret presence uses protected
references; graph adjacency is not confidential.

Character owns authored identity data, while another owner may attach state.
Archiving an actor is the ordinary lifecycle: stop new participation/behavior while
keeping references readable. Physical deletion is a separately authorized cleanup
with a known plan for each owner. Cross-owner JSON references can outlive the item;
foreign relation properties can block cascading deletion. Do not bypass either
case by removing permissions or force-deleting SQLite rows.

A retained memory may refer to a missing/archived actor as historical identity.
It must not resolve the reference to a newly created same-name actor. Owners report
missing references in inspection and reject commands needing the live target.
Retention distinguishes removing a processor's derived record, removing source
history, and erasing all knowledge about an actor; the last is not one cascade.

## 4. Source continuity for incremental processing

The source owner supplies a small source interface to admitted consumers. It is
an extension contract implemented by Chat and the chosen Scenario, not a Core
log/queue service. The minimum semantic operations are:

| Operation | Input | Output or error |
| --- | --- | --- |
| Source state | Stream identity, admitted perspective | History epoch, bounded readable boundary, retention state |
| Read window | Captured epoch/head, cursor, record/byte budget | Ordered authorized records, next cursor, explicit coverage/truncation |
| Check evidence | Bounded source IDs/revisions or captured source position | Valid, changed, missing, unavailable or denied |

Source state is a coherent owner read. Each read window verifies the captured
source epoch and eligible range. Appends after the captured head are excluded;
earlier edits reject as `source-changed`. A window never claims to cover records
it skipped due to limits. A continuation cursor is valid only for the source,
epoch, perspective and bounds that issued it. Clients cannot use it to change
perspective. Exposing raw sequence gaps/counts can leak activity: private-reader
interfaces use a filtered/opaque cursor where that metadata is confidential.

Sequence is assigned in the same owner transaction as a new observation and stream
head. It is logical commit order. UUIDs allocated before inference may commit much
later than newer UUIDs; resuming after the greatest seen UUID can permanently skip
them. Do not use relation/item ID order as the incremental processing checkpoint.
Current linear Chat protects increasing entry IDs by rejecting an append whose
entry ID is lower/equal to the head. Thus this generic counterexample does not prove
an existing Chat skip bug. Keep that distinction when writing regressions. Branch
ancestry also cannot be reconstructed by globally sorting UUIDs. A source adapter
needs a defined contiguous committed order; do not add a generic Core sequence
service merely to replace an adequate owner ordering contract.

A separately listed chat fork is a separate stream even when immutable prefix nodes
are shared. Capture its own head/version and permissions, never copy the original
processor checkpoint without an explicit verified-prefix bootstrap. Selecting a
variant chat changes the selected source; it does not mutate the original stream's
epoch. A source correction within one chat changes that source's history version.
Automatic extraction must name which chats are observations of the simulation;
processing all alternative transcripts as facts leaks incompatible histories into
one actor's memory. Physical World forks remain separate Worlds. See the foundation
for history storage, deletion retention and membership rules.

This is not an event-sourced World. Store only source events that an actual consumer
needs. Existing transcript messages and meaningful outcomes are sources; no copies
of every property mutation, frame, token or Host command are required. Notifications
wake a processor but carry no durable delivery promise. Reconnect reads source
state and catches up from retained domain records.

### Witnesses audience and knowledge over time

Distinguish an event's witnesses, current read authorization and an actor's learned
knowledge. They are not one `visibility` field. A new participant cannot acquire
an old whisper merely by joining its scene. Leaving does not erase what was heard.
An observation stores the perceivers determined at the occurrence, or references
an immutable source-owned witness decision. The first small-scene implementation
can use a bounded explicit actor set; large populations need a concrete source
partition/policy that avoids enormous arrays, not a rule that only a few actors
may ever exist.

For dialogue, public means observable by the eligible participants at that moment,
not all past/future inhabitants of the World. A whisper names its permitted
recipient(s); a private thought is available to its actor and authorized author
inspection; a director instruction is not actor speech. An ordinary source read
can return only the caller's permitted view even when the original source record
contains a larger audience. Future source policy changes increment the affected
history epoch; there is no retroactive undisclosure of already delivered text.

Memory of a place/event is actor-owned experience and can outlive the scene. Scene
ID is provenance/retrieval context, not an automatic lifetime or universal filter
that prevents remembering elsewhere. Shared teaching is a new speech/observation
with provenance, not an uncontrolled copy of another actor's private memory.

### Immutable observations and corrected source text

A source can be immutable by policy or editable by owner operation. An in-place
correction, semantic deletion or change of observation audience increments that
stream's history epoch atomically with the change. Chat's fork-preserving edits
instead create a different source identity and leave the original source unchanged;
selecting that new chat is not an epoch change in the original. Appending an
unrelated new record does not invalidate all past memories. A monotonically
increasing ordinary revision cannot distinguish these cases alone.

The first implementation conservatively invalidates all derived records tied to
the old epoch of an in-place edited stream. Each stream has one captured history;
it may recompute more than necessary. It is not an excuse to permanently limit
world size: measure reprocessing cost, then add targeted evidence invalidation only
if demonstrated. Do not build a general dependency graph before it is needed.

Old-epoch records stay inspectable as historical/superseded; they are excluded from
current retrieval without first rewriting each row. Capturing a new active epoch
is one small control change. A fresh retrieval always checks the relevant source
state. A missed notification therefore does not leave known-obsolete memory in
use indefinitely. If a required source cannot be checked, return unavailable;
an optional consumer may omit those memories with an explicit degraded result.

The check defines an as-of view, not global atomicity. A source may change just
after it was checked. Consumers capture the checked epoch and revalidate required
inputs before committing their own output; only the destination owner's own guard
is atomic with that commit. Strict cross-owner confidentiality revocation after
already disclosed data is not provided. Remote adversarial scenarios need stronger
admission and disclosure decisions before being supported.

### A concrete edit race

1. Memory reads Chat epoch 4 through sequence 20 and begins extraction.
2. The author edits message 7. Chat commits epoch 5 and its new revision.
3. Memory may still commit a derived batch marked epoch 4 because its System cannot
   atomically read Chat's foreign property.
4. Retrieval checks Chat state, sees epoch 5, and excludes all records dependent on
   epoch 4. It returns lag/omitted status rather than pretending there are no memories.
5. A new batch derives epoch 5. Old batch data is historical and may later be purged.

A second remote `get-message` before step 3 reduces the race but does not remove it.
This design does not depend on perfect invalidation notifications or a distributed
rollback. An actor may use the explicitly captured epoch 4 view before the edit;
it cannot claim that view reflects later state.

## 5. Memory records and processing

### Minimal record shape

This illustrative payload shows semantic fields, not a ready Host command. `source`
identifies the domain interface and stream; it never grants access. World-local IDs
are deliberately distinct from display names. Final schemas must bind the interface
to an admitted provider, validate IDs and define field budgets.

```json
{
  "observerId": "019a0000-0000-7000-8000-000000000001",
  "kind": "observation",
  "body": "The courier said the key would be returned at sunset.",
  "evidence": [{
    "source": {"contract": "example.dialogue", "version": 1,
      "streamId": "019a0000-0000-7000-8000-000000000005"},
    "epoch": "1",
    "sequence": "12",
    "recordId": "019a0000-0000-7000-8000-000000000006",
    "recordRevision": "1",
    "interpretation": "heard-claim"
  }],
  "status": "active",
  "revision": "1"
}
```

The source payload may describe who spoke, but this memory does not assert the
promise was fulfilled. Confidence is optional and must name its meaning: extraction
confidence or belief strength, not a magic truth score. A subject/target reference
is optional and differs from the observer. Narrative text is useful content, not
the entire schema; other consumers may use source, kind, identity and structured
fields without parsing prose.

For the first processor use observations and reflections, with summary/diary as
optional derived forms. Keep a bounded evidence list. Create processing control records only for active
observer/source subscriptions, not the Cartesian product of every actor and stream. Summaries of large windows
reference a source epoch and covered sequence range instead of listing thousands
of IDs or copying the transcript. A range claim is valid only if the processor
actually consumed that bounded source window. Summaries spanning several streams
carry each dependency within a configured bound; otherwise split them.

Reflections retain the source continuity they depend on. Either flatten a bounded
set of original source dependencies or limit the first processor to reflecting
direct source windows. Never validate only a derived summary's own revision while
ignoring that its original evidence was edited. General arbitrary chains of memory
about memory are outside the first schema, not silently treated as verified facts.

### Control record and one batch

For each active observer/source/processor combination, keep a small owner control
record: source epoch, last committed consumed sequence, processor/config identity
and its own revision. Add current error/status only if useful for persistent
inspection; in-flight jobs themselves remain RAM operations. There is no durable
queue of every future LLM job.

1. Read source state/window and capture the control revision plus any records that
   may be corrected. Overlap can be supplied as context, but only the new range is
   consumed. Do not interpret context overlap as new events.
2. Apply deterministic extraction first where possible. Optional inference produces
   a bounded proposal. Validate output, evidence membership, source coverage, actor
   IDs, number/size of records and allowed kinds before submission.
3. In one Memory System, check control/record revisions and that the proposed range
   extends the current checkpoint without a gap or duplicate. Write records and
   checkpoint together. A no-change batch can advance coverage without new records.
4. Any required validation/write failure rolls back both. A source-read failure,
   truncated unacknowledged window or failed extractor must not advance coverage.
5. On restart, read the committed checkpoint and source state. Unfinished LLM work
   is interrupted; an explicit scheduled next processing step can recompute only
   unconsumed source. No in-memory pending status is replayed as a completed batch.

Idempotence here is successful application once for a source range within its epoch
and processor configuration. Concurrent batches captured at the same checkpoint
cannot both advance it. It is not exactly-once inference/billing. Reprocessing with
a different model/config is explicit replacement/rebuild, not silently treating
old source as new experiences. Keep processor and embedding model identities
separate: changing the embedder only requires a retrieval index rebuild.

Canonical observations are retained independently of successful optional enrichment.
If extraction is deterministic identity-preserving copying of an authorized source
observation, commit that before/without LLM reflection. If one batch combines
required extraction and optional diary, either commit the required portion with a
separate diary coverage state or fail the whole declared batch; never report the
diary processed when only the observation was stored. Independent processors can
have independent checkpoints; do not put every toggle behind one global watermark.

### Corrections and source retention

A manual edit increments the record/control revision checked by pending processors.
Use an explicit replacement status/link for meaningful corrections, preserving the
source record until the selected retention policy removes it. Mark author-inserted
memory as authored, not as an event witnessed by the character. A user can knowingly
create a false belief; a processor cannot present it as verified world truth.

Deleting a derived memory and deleting its source are different operations.
Processor reprocessing may recreate a deleted derived item unless the operation
also records the intended suppression/correction within that observer's processing
policy. For the first slice retain a bounded superseded/suppressed record rather
than promising permanent forgetting after physical deletion. Never keep an
unbounded hidden tombstone history without a retention requirement.

Planned pruning of old raw history differs from a semantic correction or an erase
request. A source may advance a declared retained-from boundary without changing
history epoch if its contract attests that earlier occurrences were not rewritten.
Existing memories retain their provenance and permitted excerpt and mark the raw
source as pruned; they do not claim full original evidence is still readable.
A new participant still does not inherit those memories. Pruning is an explicit
retention action with an author-visible effect, not an automatic consequence of
summarization or embedding-cache eviction.

A lagging processor whose checkpoint precedes the retained boundary receives
`history-not-retained`; it cannot advance over the missing range as if it processed
it. An explicit accepted summary/checkpoint import can establish a new basis,
labelled with its provenance and coverage loss. Edits below the retained boundary
cannot silently reconstruct missing source: reject unsupported editing or require
an explicit retcon/rebuild policy with visible lost coverage.

Semantic deletion, audience correction or erase requests do change history epoch
and exclude affected derived records as described earlier. Ordinary raw pruning
does not make a long-lived NPC forget everything learned. Destroying the whole
source without a retained source-state attestation makes evidence unavailable;
current retrieval may omit it or use explicitly configured historical-belief mode,
which labels unverifiable evidence and never claims a fresh source check. Retaining
an excerpt is not a second transcript and must follow the source's disclosure and
erasure policy. Never resolve a missing reference through a name search.

## 6. Privacy and authorization

There are three separate boundaries:

1. Simulation perspective: which facts an actor should know.
2. Extension trust: which installed components may receive protected data.
3. Human authorization: which authenticated person controls a given actor.

This slice implements the first through trusted owners and explicitly admitted
consumers, preserving the existing second. It does not invent the third from a
browser-provided actor UUID. A trusted omniscient director and an untrusted mod
are different callers. Give each the minimum read/mutation operation it needs.

Memory/Lore/Scenario may each hold many actors' confidential data. Their APIs check
attested caller, World, operation and permitted perspective before returning text,
IDs, counts, snippets, citations, diagnostics or private assets. An actor parameter
selects a view only after admission. A public author inspector or source export
cannot be reused through an unprivileged method to bypass the filter.

Retrieval first narrows eligible records, then ranks them. Rank-all-then-filter can
make hidden records displace valid top-k results and leak information through
scores/counts. Do not return similarity vectors, hidden matches or source prompts
to untrusted consumers. Inference and remote embeddings transmit data externally;
permission for local access does not automatically approve a new destination.

### Topology and assets

A private Memory object stores observer/source/subject IDs inside protected JSON,
without an actor→memory graph edge. That avoids a specific adjacency leak; it does
not promise hiding all object existence, timing or traffic metadata against a
privileged local inspector. Public book membership, routes and scene participation
may use tagged relations when their existence is intentionally public.

Public identity is not unrestricted public state. An actor can have a public name
and confidential beliefs/inventory. Mechanical rules must not expose hidden
possessions through public relation topology if the experience requires secrecy.
Use protected owner records and an appropriate retrieval path instead.

Current CAS reads check the component's asset-read permission and the supplied
asset identity, not actor knowledge or per-message audience. A digest is not a
secret-access token. Keep confidential textual data inside protected properties
and filtered responses; do not move it to a globally readable plaintext asset to
avoid property limits. Large private audio/documents need a separately specified
protected resource/disclosure path before that feature is shipped. This does not
require broad Core changes for the initial textual example.

Logs and author diagnostics must distinguish their audience. Store IDs, bounded
error codes and permitted counts for routine processing; do not routinely log full
private prompts, credentials or raw provider bodies. This is a consequence of the
actual data flow, not a claim of general multiplayer security.

## 7. Retrieval without an accidental database framework

### Existing query primitives

Storage provides exact property-key pages, object/relation reads and directional
adjacency filtered by an owned tag. Pages allow at most 256 records, batches at
most 64 item IDs and 64 property keys; a native property page also has an 8 MiB
limit. Guest/transport/encoded replies impose smaller limits. Record count alone
cannot prove a reply fits. Implement bounded response sizing and explicit paging.

There is no current extension API for arbitrary JSON predicates, SQL, FTS, vector
similarity or sorting by importance. Calling `find-properties(memory)` and filtering
all Worlds' memories per actor request is not an indexed actor lookup. Restrict
all caches and requests to their actual World/owner instance.

### Recommended initial strategy

Use graph adjacency for genuinely public relationships and private typed records
plus an owner-local RAM index for confidential memory/lore. Build the private index
once per activation in bounded pages, then update it after successful owner writes.
The index maps observer/scope/status to IDs and stores only necessary search metadata.
Load bounded bodies for selected candidates; do not eagerly duplicate every large
body and every model vector in RAM.

Capture a canonical owner data revision around initial scan. During scan, detect
intervening owner mutations and reconcile them through a bounded in-memory delta
list or restart the scan. Do not publish a partially built index as complete. The
simplest first implementation may briefly reject/defer that owner's new mutations
while its bounded page scan completes, without holding the World writer or blocking
other owners. If that delay exceeds the measured startup budget, implement bounded
concurrent reconciliation; do not permanently cap actors for convenience.

This revision is maintained in a small owner control record by every relevant
mutation, including author corrections. A read checks the cache's corresponding
version. A missed notification, worker restart or direct admitted owner command
must not leave an apparently current index. No raw SQLite path exists in a WASM
extension; a sidecar index service requires an actual permitted lifecycle.

Index memory has a hard byte budget. On exhaustion, evict derived bodies/vectors,
return an explicit indexing/limited-search status, or use a bounded candidate scan
with honest coverage. Do not silently return top-k as if the full eligible domain
was searched. A user-selectable recent-only mode is legitimate when labelled as
such. Reading one small actor view should not rescan every large record each time.

Lexical search plus recency and explicit references is the first sufficient
retrieval baseline. Use deterministic tie-breaking by stable ID. Optional embeddings
are keyed by content revision, model identity/config and dimension; changing any
invalidates those vectors. Do not compare mixed embedding spaces. Filter by
perspective before vector ranking. Index loss never deletes canonical memory.

No ANN/vector service is mandatory. Add one only after the actual corpus and
latency/quality target justify its native/service packaging, updates, recovery and
permissions. A persistent derived index is allowed if it is recoverable and has
one canonical source; it is not automatically supported by the current WASM API.

### Pagination and consistent author views

An owner API returning several pages captures an owner revision and checks it on
each subsequent page. On change, return a stable stale-page error and restart the
view; do not combine old and new pages and describe them as one snapshot. For a
source window, the specialized epoch/head rule permits later appends while excluding
them from that window. Source edits still invalidate it.

Do not expose a database cursor unrelated to the caller's filter or disclosure.
Bound work as well as returned size: a page of five eligible memories may otherwise
require scanning a million unauthorized/irrelevant rows. Expose continuation/coverage
and stop when the work budget is exhausted. A single huge property that cannot fit
one authorized response needs its own bounded content contract, not endless page
size reductions that can never succeed.

### When to change the shared query boundary

Measure cold initialization, hot actor lookup, update/invalidation cost, memory and
WASM/transport overhead. If private owner queries remain impractical, compare:

- owner-local metadata/index improvements;
- public adjacency where disclosure is acceptable;
- a permitted rebuildable local index;
- a narrow owner-scoped shared query/index primitive.

A new Core primitive needs a concrete multi-domain query, exact input/output and
bounds, permission checks, an index/schema compatibility plan and failure evidence.
Four tables do not imply every query must be a scan, but the current DDL allowlist
also rejects arbitrary extra indexes. Do not secretly add expression indexes or
FTS tables, a SQL passthrough, arbitrary JSON query language or dynamic schema-per-
actor naming to work around that decision. A justified small shared improvement
is preferable to every extension implementing its own fragile storage engine.

## 8. Time rules and autonomy

A Scenario owns its simulation clock: step count, explicit elapsed duration or a
chosen real-time policy. Define units and source before adding decay/statuses.
UUID timestamps, message count, render frames and Host monotonic milliseconds are
not interchangeable clocks. Host monotonic time is only meaningful inside an
activation and cannot be resumed as persisted world time after restart.

The first scene is explicit-step based and freezes when stopped. Psychology receives
that time or a configured conversion with units, not implicit wall-clock minutes.
A real-time companion can opt into capped elapsed updates with wall-time rollback
and suspend handling. It computes one bounded catch-up update; it does not enqueue
all missed ticks. No Core universal scheduler is needed.

A status effect belongs to the ruleset that knows its application, stacking,
expiration and removal. Clock progress/threshold outcomes commit under the same
owner when they must agree. A plot arc is authored scenario state, not a guarantee
that the world will force the planned ending. Hidden arcs are filtered before NPC
planning. Narrator can suggest consequences but cannot mark an arc complete by
printing a tag.

Autonomy selects opportunities under finite call/time/spend and per-actor budgets.
Pause/mute/World stop prevent new admissions. Define whether newly applied evidence,
explicit elapsed time or a user action wakes it; its own reflection/diary is not
fresh external evidence. A fictional promise is domain state, not a durable OS job.
If a concrete due goal must survive restart, store that goal and define skip/catch-up
semantics in its owner; do not introduce a universal durable task service.

## 9. Authoring operations and template examples

The first useful Studio supports these operations through domain owners:

| Operation | Required check | Result |
| --- | --- | --- |
| Create/import actor | Valid authored data/assets, allocated ID | Durable Character or scenario actor |
| Rename/edit actor | Expected authored revision | Same item ID, updated revision |
| Create location/route | Valid endpoints and domain rules | Bounded named place and connection |
| Create scene/participation | Valid IDs, lifecycle, chosen owner | Inspectable scene and participants |
| Add fact/lore | Appropriate author capability and visibility | Versioned content with explicit owner |
| Preview actor knowledge | Admitted perspective, source state | The same filtered view the actor consumer receives |
| Inspect/correct memory | Evidence, expected record/control revision | Explicit authored correction/suppression |
| Execute test action | Expected Scenario revision and valid intent | One inspectable committed outcome |
| Archive actor/scene | Owner lifecycle checks | Retained identity/history without active behavior |
| Export World | Existing stopped/portable export contract | Exact packages, complete assets and no effective local grants |

These are domain operations, not a new universal CRUD API. Lists expose stable IDs,
names and bounded summaries; detailed bodies use separate reads. Conflict responses
include enough permitted information to refresh, not a silent last-writer-wins merge.
A trusted author operation is still validated; it may perform an explicitly named
rule override but cannot accidentally bypass rules by editing a foreign property.

### Required example worlds

1. Empty scene with two same-name actors, one location, an explicit clock and no AI.
2. Public possession plus a private fact known by only one actor; demonstrate the
   different read paths and absence of private graph links.
3. A gate action with one committed roll/result, failure before/after narration,
   a character leaving and returning with the same memory identity.
4. A memory correction/history edit demonstrating source epoch and stale exclusion.
5. A small survival or exchange example controlled without Chat, proving the data
   model is not a set of disguised transcript fields.

For every example include schema/owner map, create/read/mutate commands, expected
values and failure cases, then an actual portable World template after the slice
is implemented. Illustrative JSON is not an importable package. Do not ship a
hand-edited seed that bypasses owner invariants and call it a working SDK example.

A copied World carries domain data and declared asset closure. Do not persist local
filesystem paths, device handles, operation tokens, external connection secrets or
component activation IDs. Portability metadata is generated from actual schemas.
Cross-world ID references require explicit source disclosure/destination acceptance;
they are not FK, authority or atomic transfer. A copy is an independent fork.

## 10. Error semantics and acceptance

Use existing operation/lifecycle envelopes where adequate. New domain errors need
stable meanings; exact wire names are frozen per slice, with at least these cases:

| Meaning | Required caller behavior |
| --- | --- |
| Invalid shape/identity/range | Reject before mutation; explain bounded field issue |
| Denied operation/perspective | No private existence/value details |
| Revision or source changed | Refresh explicit view; do not silently update captured guards |
| Missing source/reference | Show missing/historical state; no name-based substitution |
| Index warming/limited coverage | Display unavailable/degraded retrieval, not falsely empty knowledge |
| Limit exceeded/busy | Bounded rejection/backoff; no unbounded invisible queue |
| Provider failed | Preserve committed domain state; no incomplete structured mutation |
| Unknown admission/commit | Reconcile by retained domain identity; no automatic replay |
| Stopped/retired activation | No new publication; existing committed records remain |
| Unsupported model/schema | Explicit refusal or opted-in conversion, never silent reinterpretation |

Source-level and deterministic acceptance must cover:

- Schema-invalid and schema-valid-but-domain-invalid seeds; rename and same-name IDs.
- Duplicate source range, overlap, no-op batch, extractor failure, atomic checkpoint
  rollback, delayed batch after correction, source edit between read and commit.
- Observer visibility across all read paths, including diagnostics, attachments,
  translation, summary, embeddings and tool/model requests.
- Relations versus private references, delete cascades/denial, retained missing IDs.
- Two independent Worlds and multiple streams for one actor, no global selected-user
  leakage, preserved actor on departure/return.
- Cache loss/rebuild, model change, intervening mutations during paginated scans,
  bounded result bytes and work, delayed old-ID commit after newer-ID processing.
- No AI/media installed; rule action and canonical observation storage still work.
- Stop/restart and template round-trip, exact optional dependencies and fresh grants.

Performance datasets should include 1,000, 10,000 and 100,000 realistic memory/lore
records with varied body sizes, 10/100/1,000 observers, skewed hot actors and concurrent
updates. They are test workloads, not arbitrary platform capacity limits. Measure
cold/warm p50/p95, peak Store/Host memory, examined rows/bytes, host calls, writer
occupancy and stop responsiveness. Run retrieval without LLM first. Then measure
an end-to-end fixed-provider scene separately, including quality on contradictory
facts and multilingual queries. Set product latency budgets before accepting the
slice; raw SQLite timing alone cannot establish them.

## 11. Minimality checks

Every proposed durable record solves identity, provenance, lifecycle or an atomic
invariant. Every new read boundary prevents unauthorized disclosure or avoids
copying canonical data. Every counter above distinguishes a concrete race. Remove
fields, APIs or packages whose consumer cannot be named in the accepted slice.

Do not trade total complexity for a cosmetically small Core: repeated private index
engines, duplicate authority registries or per-provider transfer tunnels warrant a
shared fix when demonstrated. Equally, extensibility does not require a framework
that predicts every possible simulation. The initial contracts preserve ordinary
extension freedom and provide complete examples that work within measured limits.
