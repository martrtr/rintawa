# Simulation experiences and companion extensions

Status: proposed architecture, revised 2026-11-25. New contracts require owner
acceptance. The accepted Chat revision slice is preserved below; accepted
streaming and interrupted-text retention remain governed by the
[AI amendment](ai-multiple-providers-streaming.md). This proposal does not authorize
product implementation, change the four-table Core, or claim feature completion.

## Purpose and scope

Rintawa should support actors that remember, form beliefs, act under rules and
appear through different interfaces. Text conversation is one interface. An
economy, survival simulation, spatial environment or external game can use the
same storage and extension foundation without Chat, Narrator, human characters,
voices, or a universal simulation ontology.

The [Soul-of-Waifu reference](https://github.com/jofizcd/Soul-of-Waifu/tree/747048b3b3ad7d321a667630018f7ffc04eb5f6d)
is a source of useful scenarios, not a mandatory implementation or dependency
list. Adopt memory, perspectives, temporary NPCs, lore, rules, psychological
state, authoring, and optional media. Exclude its Python plugin loader and arbitrary
script execution. Do not require every voice engine, provider brand, personality
preset, desktop tool or external client to deliver the simulation foundation.

The companion [data and observation contract](simulation-data-and-observations.md)
defines storage recipes, source continuity, retrieval, editing and performance
acceptance. Together these documents specify the proposed architectural semantics.
Each delivery slice must record its actual JSON Schemas, method names and numeric
budgets beside its implementation before coding the corresponding public interface.
Later platform gates below remain deliberate design work, not unspecified tasks
for an implementer to solve by inventing Core mechanisms.

## 1. Current foundation and preserved guarantees

| Boundary | Existing behavior | Consequence |
| --- | --- | --- |
| World | Object/relation items, owner properties, immutable schemas, CAS references | Domain types do not require new SQL tables |
| System | One owner-scoped pinned snapshot, serialized evaluation and atomic changes | One owner can enforce a complete transition; separate owners cannot share this guarantee implicitly |
| Property ownership | Exact extension instance; all active versions of a property name have one owner | Same schema or interface does not grant another package access |
| Graph | Object endpoints, shared relation topology; values remain owner scoped | Hidden text does not hide connected actor IDs |
| Guest reads | Bounded pages/batches; separate calls can see different committed states | A multi-page read needs an explicit owner consistency policy |
| Transport | Attested immediate caller, directed requests, activation-owned resources | Discovery is neither consent nor user authentication |
| Character | Card import, original source asset, portrait and greeting | Editable author data and actor-specific private views need extension work |
| Chat | Linear messages, conversation/message revision, guarded assistant append | No existing multi-actor audience, edit history or complete branching contract |
| Narrator | Reads recent Chat and Character, calls AI, appends against captured Chat revisions | Text orchestration is not a universal world engine |
| AI | Shared connections, selected Narrator admission, bounded streaming | Independent workload authorization and per-call connection choice need amendments |
| Templates | Portable World data and exact dependencies, fresh local approvals | Templates are the initial distribution format for example worlds |

Source boundaries: [snapshot](../../crates/world-runtime/src/snapshot.rs),
[atomic apply](../../crates/storage/src/sqlite/apply.rs),
[query](../../crates/storage/src/sqlite/query.rs),
[provider resolution](../../crates/extension-engine/src/composition.rs), and
[guest resources](../../crates/extension-engine/src/runtime/wasm.rs).
A `Single` provider can be selected automatically. Never interpret selection as
explicit authorization to spend credentials or disclose private observations.

The current local Chat read methods do not provide actor-level confidentiality.
Before storing private thoughts or whispers, implement the read policy below on
all list/page/recent/get/export/action paths, not just on prompt construction.
Current actor IDs are domain identities, not authenticated human identities.

### Preserved Chat revision contract

### Prerequisite revision slice (accepted by the owner, 2026-11-24)

Before adding editing, Chat stores `revision` as a canonical unsigned decimal
u64 string on conversation and message properties. Existing active records
without the optional field read as `"0"`, without writing or migrating on reads;
new empty conversations start at `"0"`, messages at `"1"`. Each append increments
the conversation counter in the same transaction as its items/relations. Overflow
rejects as `revision-exhausted`, with no partial changes. `lastEntryId` retains its
ordering purpose. No branch, edit or alternative UI is introduced in this slice.

Narrator captures conversation and triggering message revisions from Chat.
`begin-assistant-append` additionally requires `expectedConversationRevision` and
`expectedUserRevision`; Chat's System validates both in its pinned snapshot along
with the existing boundary. A mismatch rejects as `stale-context`. Missing fields
reject before admission. New Chat/Narrator packages must be updated together;
existing Worlds keep their exact installed artifacts. There is no old unchecked
append fallback in the new implementation.

`get-message` takes `{conversationId,messageId,expectedRevision}`. It verifies
Chat-owned relation membership in this World and returns
`{conversationId,messageId,revision,conversationRevision,role,text}` only for the
matching message version; wrong membership/missing data rejects, stale revision
returns `stale-context`. This bounded owner read supports contextual action
fixtures, without granting write authority or storing a second message copy.

Acceptance: atomic counter/message creation, canonical string/overflow rejection,
legacy active records readable without rewrite, fail-closed missing/stale expected
revisions, generation held while another append commits, restart/export/import,
and a contextual action using the right message while the UI changes selection.


Accepted streaming includes saving a known broken stream's nonempty validated
prefix as an ordinary assistant message through those same guards. Such a message
has no persistent failure badge and is ordinary history. Memory cannot later infer
whether it was complete. Stale context, unknown write outcome, invalid snapshots,
activation stop and failed empty output do not become successful messages.
Structured action extraction requires a complete validated result; partial JSON
must never become an executable action merely because partial prose is retainable.
The owner subsequently selected explicit Chat Stop retaining received text: Narrator
freezes its validated prefix, cancels inference and saves through the same guards.
This is distinct from retiring an activation or detaching a UI observer. Once a
Chat transaction is submitted, reconcile its real result rather than promise it
was undone. See [foundation](package-and-conversation-foundation.md) for the new
proposed lifecycle, independent chat forks and the revised delivery order.

## 2. Responsibilities and packaging

A boundary is justified by distinct authority, lifecycle or independent replacement.
A class, editor tab or feature name does not require a separate RTW. Multiple
components of one admitted extension instance may share its property ownership;
transport identity is still checked at each public method. Sharing a Rust library
shares code, not authority or durable state. Modules in one instance share a trust
boundary: exposing separate interfaces does not sandbox them from each other.

| Responsibility | Canonical data or service | Explicit exclusions |
| --- | --- | --- |
| Character | Authored actor description, imported source, greeting/examples and default presentation references | Current inventory, private memories, universal biological state |
| Chat | Transcript, message identity/revision, conversation ordering and supported presentation alternatives | Executing the actions described by prose, world rollback |
| Scenario | Locations/scenes, participation, rule-bearing state, action outcomes, observation eligibility and simulation time for its concrete ruleset | Universal engine for all possible worlds, every actor's learned memories |
| Memory | Observer-relative recollections, beliefs, summaries, diary, correction and source coverage | Deciding objective world truth or inventing what an observer perceived |
| Psychology | A selected fictional model of drives, affect and subjective attitudes | Another copy of objective health, all personalities or avatar animation |
| Lore | Authored contextual knowledge and bounded selection over authorized inputs | Random world events caused by reads, runtime truth inferred from prose |
| Narrator | Text context assembly and description of a captured situation or committed outcome | Core simulation clock, rule authority, direct access to all private data |
| AI Provider | Inference adapters, capabilities, selected connection/model, operation lifecycle and spending admission | Character memory, prose-improvement features, action execution |
| Presentation | Avatar, environment view, HUD and playback presentation | A second canonical actor or simulator |
| Voice | Speech synthesis/recognition, optional conversion and live session coordination | Mandatory dependency of Chat or Character |
| Studio | Author workflows calling domain owners and rendering their editors | Universal raw-property writer, bypassing owner rules |
| Native/tools/client adapters | Explicit external effects, devices and authenticated gateways | Implicit Host privileges or another plugin scripting system |

### Recommended initial packaging

Keep existing Character, Chat, Narrator and AI packages. Add one concrete Scenario
package for the first social/survival example, one Memory package, and one Lore
package. Implement Psychology as an independently disableable module of Memory's
first package if they share source processing and correction; do not split its
transaction into a remote call merely to obtain another package name. Its read
contract and records remain distinct so a later standalone implementation is
possible through an explicit ownership transition. It is also valid to ship a
standalone Psychology package initially if independent selection is an immediate
product requirement; that variant must accept independent source coverage.

Scenario is a responsibility, not a new mandatory framework called `Simulation`.
A different ruleset may replace this scenario, own different schemas and still
consume Memory/Lore/inference. Do not force an economy to use HP, mana, quests,
turns or an anime personality taxonomy. Do not make all mechanics plugins of one
large Campaign RTW: independent mechanics are ordinary extensions where their
invariants permit independent commits.

Keep the rules for a genuinely atomic action under one owner. A trade transfers
both possessions/balances under the trading rules owner. A mood service may offer
advice, but cannot independently own the balance that the trade must atomically
check. If exhaustion must forbid movement, the action owner must own that
mechanical resource. Psychology may derive fatigue from it without becoming a
second writer. An external game remains the authority for its physical outcomes;
its Rintawa adapter records acknowledged outcomes and handles unknown execution.

Lore and Memory share private retrieval code only when their algorithms genuinely
match. Do not introduce a generic knowledge service or merge their meanings:
author-written canon, an observed event and a character's mistaken belief differ.
Studio is initially a panel with explicit configured owner integrations. Extract
a reusable editor contribution contract only when a second real editor needs it.

### Replacement and coexistence

A contract can have alternative implementations. Replacing a provider does not
transfer its properties, secrets or trust. For a new World choose a provider and
its schemas in the template. For an existing World require an explicit owner-aware
export/import or conversion, with validation and rollback of the conversion itself.
Do not invent automatic schema migration or pretend that an interface name grants
access to the previous implementation's records.

Two packages cannot independently register the same property name and both own it
in one World. Use one owner serving many actors/books/scenes, or different domain
schemas and explicit composition. Do not generate a schema name per actor, scene,
mod instance or memory. Multiple views and independent compatible consumers need
no duplicate storage. A missing optional package disables its feature visibly;
removing it does not silently erase its persistent records or reassign them.

## 3. Actors and authored content

An actor is a World object used by the selected experience. A Character card is
optional authored information about that object, not proof of agency, humanity,
knowledge, permission or a separate immutable universal entity type. A logistics
agent or building can acquire memory without importing a roleplay card.

Character owns original bytes and bounded editable fields. Preserve original card
metadata for round-trip export. The effective authored description is an explicit
revision, not the original CAS file silently patched in place. Selecting another
initial greeting is an explicit create action; restart never emits it again.
Personas describe the local participant's fictional role, not a Host principal.

Distinguish `public presentation`, `author description` and `actor perspective`.
A card can contain secrets about its own actor or future plot. Sending the whole
card to every NPC or renderer is incorrect. Character exposes selected fields to
admitted consumers; scenario-specific secrets belong to Scenario/Lore, not a
public avatar descriptor. The local author inspector can view all authorized
content, but that view must not be reused as the input for an actor planner.

Editable overlays are facts of a scenario: current role, injury, disguise or arc
stage. They do not rewrite the base card. Scenario owns mechanically relevant
state; Character owns authored edits. A renderer asks for the effective public
view rather than merging unrestricted properties itself. If an authored trait is
used as a hard rule input, Scenario explicitly accepts a revision into its rule
configuration; later Character edits do not secretly alter an in-flight action.
The authored description and the rule-effective configuration have different
meanings, with their relationship visible to the author.

Author notes, examples and prompt presets belong to the experience using them.
Narrator can order text sections and expand a finite allowlist of substitutions.
Unknown macros remain text or produce an explicit validation error; no evaluation,
recursive expansion, filesystem reads or implicit queries. Imported lore cannot
self-promote to trusted instruction priority. AI-assisted authoring produces a
draft; applying selected fields is the owner's normal revision-checked operation.

## 4. Actions observations and one coherent scene

### Three modes of experience

1. Free conversation: Chat records speech, Narrator answers; no physical rule
   outcome is implied by descriptive language.
2. Author-directed fiction: an admitted author explicitly accepts a proposed
   world change through Scenario. The proposal itself has no effect.
3. Rules-driven simulation: Scenario validates an intent and commits its outcome;
   narration and animation describe that outcome.

A World may expose more than one mode, but a request names the intended operation.
A string such as `[SYSTEM DIRECTIVE]` cannot switch authority. Say, act, private
thought and director instruction are structured intents. `Continue description`
does not advance the simulation. `Advance scenario` may. A suggested choice is a
draft intent bound to the situation that produced it, not permission to execute.

### Rules-driven example

A courier tries to open a locked gate. A guard can see the attempt; another actor
is elsewhere. The same interaction must work from Chat, a Studio test panel or a
future spatial controller.

1. The caller sends the intent, actor/target IDs and the Scenario revision used to
   choose it. Scenario authenticates the immediate component and its allowed
   operation. Author-only overrides are separate from actor actions.
2. Scenario reads its own rule state, rejects invalid identities/targets and
   chooses the permitted action. A model may propose parameters but cannot supply
   an authoritative die roll, success flag, costs or final inventory.
3. The Scenario backend obtains bounded Host entropy outside System evaluation,
   if the rule requires it, and submits an internal command with the captured
   intent and expected Scenario revision. The System validates the transition and
   stores the roll with its outcome. Host randomness is unavailable inside the
   current pinned System callback. Never import networking or entropy there.
4. One Scenario transaction commits the outcome, mechanical deltas, simulation
   time progression and the evidence/observer information that must agree with it.
   A stale revision fails without applying any part. Discarding entropy for a
   rejected attempt is harmless; committed results are never rerolled.
5. Renderers display the committed outcome. Narrator may describe it. If prose
   fails, the outcome is still inspectable and can be described again.
6. Memory processes authorized observations. The guard learns about the attempt;
   the absent actor does not. The guard need not learn the hidden lock mechanism.

Backend command submission is restricted to the registered owner component by
Host; a public endpoint never forwards a client-supplied internal command payload.
Randomness is not a replay promise, proof of fairness or multiplayer anti-cheat.
Record the actual result and relevant rule configuration. Require deterministic
rule evaluation for fixed state, intent and supplied entropy in acceptance tests.

### Commit identifiers and uncertain results

Allocate a stable outcome ID before submission. Store the normalized input identity
and outcome under the Scenario owner. On uncertain commit, read that ID; an already
present matching outcome is a successful reconciliation, not a new action. A
mismatching payload under the same ID rejects. While admission may still be pending,
absence is not proof of failure and does not authorize replay. No automatic retry
of an external game/tool action or billable request follows an unknown outcome.

Retaining the domain outcome enables duplicate detection for its retention lifetime.
It does not introduce generic durable command receipts or exactly-once execution.
If a game requires retries after outcomes are deleted, that game must specify its
retention/identity rule before exposing the retry operation.

Attaching a description to an outcome is a Chat operation keyed by outcome identity
and description variant. Chat checks its own revision and duplicate attachment in
one transaction. A failed attachment leaves a valid outcome without prose. It
cannot roll back Scenario. Do not silently rerun the action to repair presentation.

### Observations are distinct from global truth

The source owner determines what occurred and which perspective it can disclose.
Memory determines what the observer remembers or infers from it. An observation
may be partial, mistaken or hearsay. `B said the key is in the drawer` is evidence
of B's assertion, not evidence of the current key location.

Scene membership is a candidate set for perception, not proof every actor heard or
saw everything. Whispers, walls, line of sight and sensors are rules of the source
scenario. Source APIs provide actor-specific observation records or filtered views.
Do not create a universal Core sensory bus. Chat can serve dialogue observations;
other simulation owners serve their own outcomes under the same bounded source
continuity semantics described in the data contract.

A truthful event and its visible description can share one source identity without
sharing their full payload. An observer receives the permitted excerpt and source
reference. A director may see all facts, but director context is never recycled
as a private actor's context. Once sent to an external model, data cannot be
retracted; choosing a cloud connection is also a disclosure decision.

### Temporary actors and participation

Temporary means participation or activation policy, not weak identity. An NPC keeps
the same object ID when leaving, returning, being renamed or becoming recurring.
Two identical names remain different actors. Participation has active/left/archive
state and belongs to a scene. Use a separate appearance identity only when a
consumer needs distinct visits; do not require a new actor for each entrance.

Draft NPC proposals stay transient until accepted. Creating a Character description
and Scenario participation crosses owners: create/validate the Character first,
then activate participation, with a visible incomplete authoring draft on failure.
Do not promise atomic multi-owner creation. A headless Scenario can create an actor
without Character metadata if that is sufficient for its rules. Actor deletion is
an explicit lifecycle operation; ordinary departure and panel close never delete it.

## 5. Memory psychology and lore

### Memory

Memory supports episodic observations, claims about others, reflections, summaries
and optional diary. Records have stable IDs, observer identity, content, provenance,
source continuity and explicit correction status. One observer's record does not
become another's through a shared name, selected UI character or common scene.

Processing receives only authorized source windows. It can return no significant
change. Canonical observations remain available without embeddings or an LLM;
optional extraction enriches them and cannot be a prerequisite for storing an
accepted observation. Summarization does not delete its source automatically.

Use separate processing settings for extraction, diary and psychological updates.
A preset is a convenient configuration, not a hardcoded mode enum limiting new
processors. Manual means no automatic inference. Batching limits records, bytes,
actors, calls and elapsed time; missed notifications are recovered from source
state and owner checkpoints. State/checkpoint changes commit together. See the
data contract for edits, source epochs, filtering and rebuild rules.

Manual corrections have an explicit human-authored provenance and revision. A
processor working from an older version must not overwrite them. Semantic
similarity suggests duplicates; source identity proves duplicate processing.
Contradictory beliefs may coexist, especially across time. `superseded` points to
an explicit replacement and does not erase the original evidence. Reflection and
diary do not feed an autonomous loop as fresh external observations.

### Psychology

Psychology is an optional fictional model selected by the experience. It may model
mood, drives, subjective trust or an agenda; it must not claim a validated human
psychology. Its state is not an unconstrained LLM rewrite of the character seed.
The model defines variables, finite ranges, update inputs, time basis and how
outputs influence an actor's choices. Core knows none of these names.

For the first numeric model, deterministic update receives prior state, a bounded
list of new observations and explicit simulation elapsed time. Each source window
is applied at most once within retained coverage. Reject nonfinite numbers,
unknown variables, negative elapsed time and invalid units. Bound model-proposed
deltas before they become state. Decay computes from time, not retrieval frequency,
UI frames or how often an avatar asks for an emotion.

If memory and psychology are in the same owner, one transaction can apply their
joint output and checkpoint. If separately owned, expose independent coverage and
allow bounded lag. A planner captures the psychology version it used; it cannot
claim the newest mood at a later rule commit. Hard rule preconditions therefore
belong to Scenario, not an independently updating Psychology owner.

Subjective A→B trust differs from B→A trust and from objective debt/alliance.
A text emotion classifier estimates expression; it does not overwrite simulated
mood. The avatar maps a public expression hint to available motions and falls back
to neutral. Physiological-looking variable names are optional model vocabulary.

### Lore

Lore stores authored entries/books, selection settings and disclosure policy.
Keyword/exclusion, observed tags, references, recent text and optional semantic
similarity are selectors. A spatial simulation can query by authorized observed
objects/tags without manufacturing a Chat transcript.

Selection filters authority and audience before ranking or returning excerpts.
It is a pure domain read: repeating the same inputs and source version cannot
advance clocks, accumulate tension, consume a random chance or fire an event.
Private caches may change without changing canonical state. A preview in Studio
uses the same selection semantics without committing anything.

Always-on means mandatory candidate selection within its declared budget; it does
not override audience or turn a record into a trusted instruction. Exclusions take
precedence over keyword inclusion. Empty keywords do not mean match everything.
Depth zero means no older transcript, while the explicit current observation still
counts. Text matching uses a documented Unicode normalization/case policy.

Delay, cooldown, sticky and chain can mean either context relevance or game state.
For context relevance use an explicit source-time/window input and deterministic
selection. When they cause events or persistent unlocks, Scenario owns their
progress and records each firing once. Lore reads the resulting eligible state.
A bounded dependency graph rejects cycles and missing required entries during
validation. There is no evaluator of arbitrary code in a lore entry.

Imported `character_book` is preserved content until explicitly imported/selected
into Lore. Do not maintain a second independently editable live copy in Character.
Remember import provenance, and make later reimport a reviewable merge or new book,
not an automatic overwrite of authored entries.

## 6. Narration planning and context composition

Narrator's public responsibility is text generation for an experience. Scenario or
a small experience controller chooses when an actor should act and which action
outcome to narrate. Do not generalize Narrator into an obligatory scheduler for
all simulations. The existing direct Chat→Narrator path remains usable.

For a group scene, admit one bounded scene step with a captured Scenario revision
and finite actor list. Choose speakers explicitly or by a replaceable selection
policy. A default social example can allow at most one reply per selected actor
per step. Group ordering is defined by the controller, not racing network replies.
A failed second speaker does not erase the first actor's committed action. A
private actor planner receives that actor's filtered situation; a GM planner is
an explicitly trusted different perspective. Reusing one hidden omniscient prompt
for all actors is prohibited.

The controller can be deterministic, model-assisted or manually driven. A separate
planner, executor and router LLM call are not mandatory. Start with one decision
call or direct user action when adequate. Bound total calls, tool steps, actor
responses, output bytes and deadline for the entire scene step. One user action
cannot recursively cause unlimited agent replies. New initiative is a new admitted
step subject to pause, spend policy and fairness between Worlds.

### Directed context reads

A text consumer configures a finite ordered list of exact source providers. Initial
integrations can use their ordinary owner APIs. If a shared context interface is
needed, it is a Narrator extension contract, not a new Core discovery registry.
Discovery contains public descriptions only. Resolve and pin the actual provider,
then send a directed request containing only the data that provider needs.

The semantic request consists of the intended perspective, source scope/position,
query observations and result budget. The semantic reply consists of bounded
blocks, their provenance/version, truncation/coverage and availability. A provider
must not need the entire unrestricted conversation or all secrets to supply mood.
Do not hardcode a closed global enum of lore/memory/rules into Core or AI; Narrator
can classify configured blocks for its own prompt policy.

Each block is data unless the experience's trusted configuration explicitly marks
that source and field as authored instruction. Provider-supplied priority cannot
promote itself. Required sources and optional enrichment are distinct. Missing
required input fails the generation; missing optional memory/lore permits a visibly
degraded result if that experience allows it. Never silently label missing memory
as an empty memory or missing observations as omniscient access.

Budget encoded bytes and model context separately. Retain mandatory rules,
current authorized input and output reservation. Drop optional blocks in configured
order with diagnostics; do not truncate structured rules or individual UTF-8/JSON
records into a different meaning. Begin within current 24 KiB content, 16 KiB answer
and 60 KiB encoded inference limits. A model token limit may be lower. Use a matching
tokenizer when available or a conservative estimate with explicit overflow failure;
byte limits alone do not prove token fit. Raising limits is a measured slice.

### Consistency of a generated response

Capture exact Chat revisions, Scenario revision/outcome, Character authored
revision and source positions used. Cross-owner reads are not a single snapshot.
Revalidate required inputs before requesting commit; this detects known changes
but cannot close the final race across owners. Chat guarantees its own boundary
atomically. Scenario guarantees its own action state atomically.

The product guarantee is a response based on the disclosed captured context, not
latest-everything-at-commit. A concurrent mood/card edit affects the next generation
unless detected before this one is committed. No foreign owner revision is quietly
refreshed to make a stale result fit. If a feature requires stronger atomicity,
move its invariant into the responsible owner or propose a specific shared primitive;
do not advertise a recheck as a transaction.

Memory retrieval is fresh relative to a checked source epoch as detailed in the
data contract. New observations after the captured head may be absent; a historical
edit detected during generation rejects dependent output. Private recipient policy
must be checked at every disclosure; no protocol can unsend already disclosed data.

## 7. Conversation history and editing

### Transcript foundation before private observations

The [package and conversation foundation](package-and-conversation-foundation.md)
supersedes the earlier proposed linear-only edit/alternative slice. The owner wants
forks listed as separate chats and edits isolated from the original, including
shared prefix messages. Chat owns immutable-history semantics, edit/delete/fork,
regeneration, membership and migration of the active format. A UI arrow selects
an independent variant chat and its own continuation; it is not permission to
rewrite a shared message or attach a stale suffix to another answer.

Each chat exposes one selected transcript. Incremental processing binds to chat
identity, captured history and admitted perspective. A new fork has independent
source state; do not copy the original checkpoint or treat every alternative as
a factual event in one simulation. Source history changes invalidate derived
knowledge for that source. Append and traversal order must be defined by Chat;
Core UUID allocation is not a universal commit sequence. The existing linear Chat
rejects lower/equal entry IDs, so the generic late-ID counterexample is not proof
that today's Chat silently skips such a committed append.

Before multi-actor private communication, add explicit speaker, message kind and
audience. `user`/`assistant` are model transport roles, not in-world identities.
Messages can represent speech, narration, private thought or a director note;
the source adapter decides which are observations. Provider reasoning is not an
in-world thought. Basic edit/Stop/persona work need not wait for this richer policy.

Editing is a transcript correction. It does not undo inventory, clocks, dice or
already spoken audio. Regenerating a description cannot reroll a committed action.
A continuation extends a named answer or creates a message through an explicit
Chat operation, never a forged user event. Full alternate physical histories remain
deferred: copying a stopped World through templates creates an independent World
with its own identity. A separate chat is not a separate simulation or world rollback.

### Observation-aware Chat admission

Reads and writes for new private message kinds verify the immediate caller and
its granted perspective. Public local transcript reads can remain available for
ordinary public conversations, but must not return private rows, counts, snippets,
attachments or IDs through alternate endpoints. Author inspection is a distinct
trusted capability, not `author:true` supplied in JSON.

In the local single-user slice, trusted controllers may select an actor perspective;
this is narrative isolation, not user-level anti-cheat. An untrusted extension has
no private-read capability. All privileged operations use explicit admission;
registering as a provider or becoming the default provider grants none. A future
remote client requires authenticated session/controlled-actor binding separately.

Translation, Speak and Illustration capture message ID/revision and perspective at
invocation. They never resolve the currently selected UI message later. Their
results do not replace the canonical original or silently train memory. Showing
an alternate language is presentation; recording a spoken translation as an actual
new utterance is a separate explicit domain action.

## 8. Inference and external effects

AI Provider remains generic: text/structured output, image input or embeddings are
capabilities added when used. It does not implement `remember`, `plan-npc`,
`improve-prose`, `update-psychology` or world mutations. Each consumer owns prompts,
output validation and the consequences of using the result.

Preserve current selected-Narrator access. Additional consumers require an AI-owned
explicit local grant for exact World scope, extension instance/component, allowed
connection set and capabilities. Trusted Settings administers these grants; default
deny for new callers. Installation, discovery and contract registration do not
create grants. At runtime match the attested current caller/activation; operation
status and release remain private to the originating activation.

Allow multiple explicitly admitted consumers, not a single universal coordinator
that every Memory/Lore/Studio extension must impersonate or proxy through. This
is a bounded policy list in existing local settings, not a new Core ACL database.
Configuration of the API that edits it must itself be privileged. Importing a World
can request capabilities but cannot import effective grants, endpoints or secrets.

Per-call connection/model selection snapshots the existing shared catalog entry;
it never changes global primary. No arbitrary URL, header or secret path arrives
from the consumer. Check capability support, request/output size, caller quota and
live connection permission before starting work. Keep provider/Host limits; quotas
are overload bounds, not reservations or promised throughput. Distinguish an
unavailable connection from an unavailable optional capability. Do not silently
fall back to a cloud service or different model.

Text streaming retains the existing bounded cumulative snapshot protocol. Generic
inference failure remains failure even if the text consumer elects to persist a
validated prefix. Structured extraction/tool intents require validated complete
output; no parsing partial streams into writes. Reasoning output is distinct from
public content and never enters TTS/memory by default.

Inference cancellation prevents further provider output and retires resources
under their owner lifecycle. Explicit Chat Stop may persist Narrator's already
frozen validated prose through ordinary guards, as specified in the foundation;
activation retirement must not start such a write. It cannot undo remote cost or
a committed System transaction. Preserve pending-release accounting. The current
HTTP worker can remain blocked on a silent read until its timeout; full-duplex
media and prompt cancellation require measured behavior and, if needed, a narrow
generic HTTP fix. Do not claim closing a handle proves immediate socket termination.

### Tool use

A tool provider exposes named typed operations, effect classification and admission.
An orchestrator proposes a call; the provider authorizes exact arguments. UI
confirmation, where the actual effect requires it, binds that exact provider and
payload. Tool output is untrusted data, never a new grant. A finite controller
budget spans all tools and model calls. Unknown outcome stops automatic repetition.

Use existing extension interactions for Rintawa actions. MCP is an optional adapter
for external tools, not the extension mechanism or a way around ownership.
Screen/clipboard/files/browser/media tools are separate optional capabilities with
concrete platform rules. Python/PowerShell execution, arbitrary shell tooling and
a second user scripting loader are excluded from this delivery objective.

## 9. Media and multiple presentations

### Avatar and environment presentation

An avatar refers to an actor and a portable asset bundle. Static/GIF, Live2D and VRM
are selectable renderers. The same actor can appear in several views without
copying personality or memory. Missing presentation does not disable the actor.
A VRM renderer does not supply 3D physics, navigation or a complete world runtime.

A model bundle names an entry asset and a closed relative-path-to-AssetRef map.
Reject path traversal, undeclared external fetches, missing resources and excess
decoded resource budgets. Original CAS bytes remain immutable; Live2D expression
configuration is an overlay descriptor or a new derived asset. Declare the entire
asset closure for template export. No private knowledge is stored in a publicly
readable model descriptor.

Motion/expression names are model-specific mappings, not 28 hardcoded Core states.
The experience maps public expression hints to supported model capabilities;
missing mappings use neutral. Lip motion follows actual playback position and
stops with playback. Animation frame rate does not advance psychology or rules.
Background/ambient settings belong to scene presentation, with explicit local
overrides; changing a scene retires its old loops and resources.

### Voice as optional installable capability

TTS consumes authorized immutable text and voice configuration. STT consumes a
bounded audio session. Voice conversion is an optional stage. Install each engine
and model independently; disabling TTS at runtime is insufficient if every engine
is still mandatory at install/import/startup. Start with one functioning engine.
The reference's Qwen3/XTTS/Kokoro/Silero/Edge/ElevenLabs/RVC families are independent
integration candidates, not prerequisites or a claim of current compatibility.

First deliver Speak on one committed message and push-to-talk producing an editable
or explicitly submitted transcript. Neither needs a continuously running mic or
voice-driven action loop. A TTS request pins source revision, actor/voice selection,
World and operation. Text export to a remote voice service follows the same private
content disclosure policy as inference.

A live call owns capture, STT, text generation, queued speech, playback and avatar
synchronization. Define max utterance duration, encoded/decoded bytes, segment
duration, queued audio duration and processing slots across the complete path.
Bounding only the final audio queue leaves an unbounded text/decoding queue upstream.
Backpressure stops admission or drops explicitly designated provisional output;
it never silently drops canonical domain actions.

Every segment belongs to one session/generation and sequence. Barge-in stops old
playback, retires queued segments and cancels old inference best effort. Late output
cannot restart retired speech. Explicit policy decides whether audio continues
when a panel closes; stopping the World always stops capture/playback and closes
resources. Device loss yields an observable terminal state, not endless pending.

Live audio is transient by default. Recording is explicit and produces an asset
with retention/disclosure policy. Machine device names, credentials, process
handles and temporary buffers are not portable actor state. Generic CAS read
permission is not per-actor asset authorization; do not encode confidential
recordings in freely shared assets and claim a private message flag protects them.

### Image generation

Image generation is an optional action/provider capability. Capture source
message/outcome revision and authorized context; generate/import bytes before
attaching through the owning domain operation. If the source changed, retain an
explicit unattached result or discard per policy, never attach to current UI
selection. Unknown billable outcome is not retried automatically. Automatic1111
and native ComfyUI workflows are different protocols; an A1111-compatible endpoint
labelled ComfyUI does not prove native workflow support.

### Native and remote delivery gates

GPU workers, desktop overlays, model installation and OS integration need a concrete
native launcher/adapter design for the chosen platform. A separate process improves
failure containment but does not sandbox it. Pin executable discovery, authenticated
local IPC, working directories, inherited environment, process-tree ownership,
stop/timeout and crash behavior before delivery. Do not kill unrelated existing
model servers or place native workers inside Host for convenience.

Existing HTTP streaming, CAS chunks and loopback resources are candidate primitives,
not proof of a bounded low-latency device path. Measure capture→recognition→AI→speech→
playback, including silence, disconnect and stop. If a common transfer fix is needed,
amend the shared primitive once instead of per-engine base64 tunnels.

A local model manager is optional: download with identity/integrity, cancel, disk
space checks and atomic publication; retain the previous usable backend on failure.
Weights are machine resources by default, not copied into every World export.
Backend launch/update is explicit and constrained, not arbitrary scripting.

Remote browser/Discord use an authenticated gateway to admitted owner APIs. Bind
external account/channel/session to World and controlled actor; separate DM from
public audiences, deduplicate external messages where the adapter supports it,
and reject loops. Never publish the privileged local root bridge as a remote API.
GIF/image input is not temporal video understanding. Genuine multiplayer,
Wayland overlays and native device security need their own concrete platform
acceptance; they do not block local scene/memory delivery.

## 10. Authoring and delivery boundaries

Studio first assembles a useful World from ordinary owner operations: actors,
locations/routes, scene participants, lore/private fact, memories and one rule
example. It shows IDs only where needed for linking/diagnosis, with editable names
for normal work. It has separate author and actor-preview views. Preview must call
the same filtered read path used by that actor's consumer.

Each owner exposes a small editor/read/mutation surface with revision conflicts,
validation errors and explicit archive/delete semantics. Studio never writes a
schema-valid but semantically invalid record through raw generic setters. JSON
Schema is necessary for shape, not sufficient for invariants or authority.
Multi-owner creation shows partial progress and retryable individual steps. It
does not introduce a generic distributed authoring transaction.

Deliver typed examples before a universal form language: two same-name actors;
a location and route; a key transferred under rules; a hidden fact; differing
memories; an NPC leaving/returning. Include a second non-chat controller or a
headless acceptance fixture invoking the same rules, so simulation independence
is demonstrated rather than promised by naming.

### Proposed increments

| Increment | Complete useful result | Depends on |
| --- | --- | --- |
| Product foundation | Local package management, Stop, independent chat forks, edit/delete/regenerate, message slots and persona | Foundation contract; package management and Stop can proceed independently |
| Data and authoring | Small Scenario, typed examples, owner forms, restart/template round-trip | Existing Core/Character; no AI or media; examples can proceed beside the foundation |
| Observations and memory | Two perspectives, durable observations, correction, bounded retrieval | Stable Chat branch/source semantics and source continuity/admission; optional inference for enrichment |
| Lore and psychology | Read-only selection, one model, clear source coverage | The same observation path; no obligatory agent loop |
| Rules and group experience | Atomic outcome, NPC continuity, directed actors, choices and Narrator descriptions | Scenario authority, Chat private kinds, inference grants if used |
| Presence | Static avatar, committed Speak, push-to-talk | Message revision/action path and one optional engine |
| Advanced adapters | Live2D/VRM, calls, more engines and image generation | Measured media and package boundaries |
| External environment | Native models/tools, remote clients, external game adapters | Explicit platform/admission decisions |

These are dependency boundaries, not a requirement to create seven new frameworks.
Deliver a narrow vertical slice and its real failure paths before widening providers.

### Acceptance scenarios

| ID | Required observable result |
| --- | --- |
| A01 | Create/read/rename/restart/export a scene; identities and owner boundaries survive |
| A02 | Two same-name actors never merge; leave/return preserves memories |
| A03 | Gate action atomically updates possession/resource/clock/outcome or none |
| A04 | Reject stale action; failed narration preserves outcome; unknown commit is reconciled without a second roll |
| A05 | Same rules invoked without Chat/Narrator/AI/TTS produce the same fixed-input outcome |
| A06 | Capture actual actor requests: hidden fact/whisper/director content is absent from unauthorized prompts and reads |
| A07 | Memory accepts an observation without embeddings; extractor failure cannot advance its checkpoint |
| A08 | In-place source correction excludes old-epoch memory after a fresh check; a Chat fork keeps the original source intact and cannot inherit its checkpoint implicitly |
| A09 | Concurrent correction and delayed processing cannot overwrite the correction; duplicate source processing creates no new memory |
| A10 | Repeated lore/psychology previews do not advance time, change state or consume randomness |
| A11 | Unauthorized inference consumer and automatic provider selection cannot spend credentials; permitted independent workloads can select distinct models |
| A12 | Existing interrupted-prefix retention remains guarded and ordinary; malformed partial structured output never executes |
| A13 | Speak/Translate/Image acts on captured message revision; late results cannot attach to a different selection |
| A14 | No voice package installed: import, rules, memory and Chat still work without voice imports/downloads |
| A15 | Stop/disconnect/device loss cannot resurrect playback or commit from retired operations; pending release retains accounting |
| A16 | Rebuild/drop retrieval cache without losing canonical data or exposing hidden topology |
| A17 | Template copy has fresh World/grants, preserves item links and complete declared assets, excludes credentials/handles |
| A18 | A provider implementation swap never silently acquires old owner data or secret grants |
| A19 | Large-source pagination detects intervening revisions; UUID allocation order cannot cause skipped observations |
| A20 | Native/external adapters prove their specific platform, caller and process boundary before being marked supported |

Use deterministic fake inference for state/ordering failures, then separately assess
behavior quality on fixed scenes. Performance tests report dataset, bytes, machine,
queries, cold/warm cost, p50/p95 and peak memory. They must include multiple Worlds
and a slow consumer. Passing a source probe or a SQLite microbenchmark does not
establish end-to-end Host/WASM/media throughput.

### Presentation and everyday authoring behavior

HUD fields describe values owned by the selected ruleset: bounded integer/number,
boolean, text or lists with explicit units, labels and allowed edits. A renderer
can display them without defining a second database schema or accepting arbitrary
`state_update` text from the model. Manual adjustment uses a named rules-owner
operation. New presets are ordinary content/configuration; no mandatory eleven
personality categories or hardcoded Core variable names.

Keep ordinary Chat usability independent of simulation scope: paged history,
conversation titles/folders where useful, copy/export, light/dark theme, accessible
controls, localized labels, visible progress and explicit stop/retry. A filtered
export observes the same audience as its source read; an author export is a
separate admitted operation. Markdown rendering treats content as untrusted and
does not turn embedded HTML/URLs into extension privileges. Remote card catalogs
are optional import providers; offline local import remains available.

Attachments preserve source AssetRef and media type with bounded decoding, not an
unbounded base64 copy in the transcript. Vision input requires explicit provider
capability and perspective filtering. Ambient audio, translations and generated
illustrations are derived presentation unless an explicit owner action makes them
in-world observations. A scene's folder or UI grouping does not change its identity,
clock, participants or access policy.

## 11. Decisions requiring acceptance

The recommended design needs owner acceptance of these new contract changes:

1. Scenario owns atomic mechanical transitions; independent Memory/Lore and
   optional Psychology expose filtered, versioned views with bounded lag.
2. Source continuity/history epochs and private Chat semantics, reconciled with
   the separate-chat fork product decision in the foundation; transcript changes
   affect derived knowledge without automatically undoing world actions.
3. Explicit extra-consumer inference grants and per-request model selection;
   provider discovery is not spending authorization.
4. Actor-aware observation/read policy, including limited local trust guarantees
   and separate future authenticated client semantics.
5. Initial memory schemas/coverage and owner-specific authoring APIs under the
   four-table data contract, plus optional installable media.

No new Core table, universal event ledger, distributed transaction, generic workflow
scheduler, omniscient knowledge registry or scripting language is proposed.
A measured query/cancellation/media limitation may justify a narrow shared Core
amendment; it must preserve authority and cleanup and demonstrate why local owner
code is insufficient. Do not shift repeated platform problems into every extension
merely to keep the Core diff at zero.
