# Narrative worlds and companion applications

Status: proposed product and extension contract, not an assertion of implemented
features. Requires owner approval for new contracts. The existing Core remains
generic; Character, psychology, Chat, rules and avatars are extension concepts.

## Outcome and scope

One World can support a private conversation, a campaign with several actors,
and another presentation of those actors. A separate World can implement an
economy, programming harness or Minecraft integration without loading these
extensions. Installing the roleplay distribution must not make its ontology a
requirement of Core.

The target includes the capability families listed in the
[Soul-of-Waifu reference](https://github.com/jofizcd/Soul-of-Waifu/tree/747048b3b3ad7d321a667630018f7ffc04eb5f6d):
conversation, memory, psychology, RPG, avatars, voice, tools, local models and
external clients. It is functional parity, not reuse of that project's internal
architecture or a claim of equal output quality. The coverage table below fixes
the scope; future changes to its README do not silently expand this contract.

The current rtwKit baseline provides Character import, persistent Chat and one
Narrator→AI→Chat reply path. It does not yet implement the complete target.
The existing `lastEntryId` check protects append ordering, not arbitrary edits,
multi-actor knowledge, cross-extension atomicity or authenticated multiplayer.

## 1. Ownership and the smallest useful decomposition

| Owner | Authoritative data and behavior |
| --- | --- |
| Character | Imported source, editable character description, presentation asset references |
| Chat | Conversations, messages, alternatives, branch membership, selected continuation |
| Narrator | Turn orchestration, context budget, chosen speaker, generation policy |
| Memory | Evidence-linked recollections, beliefs, diary and summaries |
| Psychology | Optional bounded mood/drive/relationship state and its update rules |
| Campaign | Rules, clocks, inventory, scenes, NPC participation and hidden scenario facts |
| AI | Provider adapters and inference operations, independently selectable capabilities |
| Media/tools | Their jobs, external resources and presentations; no general World writer |

These are responsibilities, not a requirement for eight RTWs or eight processes.
Keep coupled small features in one extension until independent replacement or
authority warrants separation. The important rule is one authoritative writer per
fact. Relationships used by game mechanics belong to Campaign; Psychology may
derive a subjective attitude, but must not keep a competing authoritative copy.

Use current items, relations, properties, CAS and atomic Systems. Do not add Core
Character/NPC types, a global mind service, a second database or a universal tick.
Register portability for every persistent asset-bearing property. Local secrets,
running jobs, microphone buffers and machine paths are not portable World state.

## 2. Durable records and revision semantics

Chat adds an owner-managed conversation revision and per-message revision. They
are opaque monotonic decimal strings, advanced transactionally on relevant edits.
`lastEntryId` remains the append boundary. Revisions detect editing/deletion,
participant changes and branch switching that leave the last entry unchanged.
Readers receive both; writes provide the expected revision and reject conflicts.
No Core global revision or generic multi-object compare-and-swap is required.

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

A conversation selects a branch/continuation. Regeneration creates an alternative;
it does not silently overwrite the previous answer. Older edits make descendants
available as another continuation or explicitly invalidate them. UI shows which
continuation is current. Sending, editing, regenerating and deleting are owner
operations, not arbitrary property writes by a rendering extension.

Every memory record identifies its subject actor, kind, body, evidence references,
source revisions, perspective/audience, and active/superseded status. Evidence
references use World-local item IDs; externally sourced observations additionally
record their origin. Confidence is a model estimate, not truth. Corrections retain
the replaced record's identity/provenance until an explicit retention policy removes
it. Do not rewrite factual history merely because an LLM claims a contradiction.

Temporary NPCs are ordinary character objects with Campaign-owned participation
and lifecycle state. Leaving a scene deactivates participation; it does not delete
referenced memories or dialogue. Promotion to a recurring character preserves
identity. Cleanup explicitly distinguishes unreferenced drafts from story history.

Psychology records carry a model version, bounded numeric variables and a last
evaluated simulation time. Hormones are fictional simulation variables, not a
scientific claim. Formulas, caps and decay policy belong to that extension. A
campaign clock and real elapsed time are different modes. Monotonic Host time
cannot be persisted across process restarts: use it only within an activation.
Offline progression uses an explicit wall-time policy with capped elapsed time,
or freezes while stopped. Default is frozen, not replaying every missed tick.

## 3. One turn, several extensions

The turn context contains World scope from native admission, conversation ID,
branch/revision, triggering entry ID, speaker ID, mode and a request ID. Modes are
say, act, private thought and director instruction. Private thought is not
automatically observable by other characters; director text is not user dialogue.

Narrator admits one active generation per conversation continuation. Admission
captures its own configuration, Character revision, Chat boundary and provider
identity. It obtains authorized context, applies one explicit budget, calls the
selected AI provider, then asks Chat to commit against the captured boundary.
Changing the Chat-owned participant set or continuation before commit produces
`stale-context`. A source card or Campaign revision observed changed during final
validation also rejects. A new generation is an explicit action.

Cross-owner reads are not one database snapshot. Owners return source revisions;
Narrator revalidates the required inputs before its commit request. This gives a
bounded best-effort context check, not an atomic cross-owner compare-and-commit.
Chat guarantees its own revision at commit. Product behavior allows a settings or
memory edit concurrent with generation to affect the next turn; it must not claim
the response used the newest settings. Capture and display source revisions.

If a rules resolution must be atomic, Campaign owns its complete state transition
in one System. Dice result, inventory delta and clock delta commit there before
narration. Narrator describes that committed result; an AI/network failure does
not reroll dice or reverse the rules. Chat attaches the Campaign result ID. Rules
outcomes remain inspectable even when prose is missing. Do not scatter an atomic
game action across independent owner commits and call it a transaction.

If a future game truly needs atomic rules plus transcript in one transaction,
design that narrow ownership boundary then. Distributed rollback/outbox is not
required for this product. Two owners can agree on idempotent result attachment
using the existing result ID and Chat's uniqueness check.

### Context sources

Narrator defines `rintawa/narrator/context/v1` as sourced Multiple discovery, using
only `{method:"describe"}`. Each response identifies its context kind and read
point. No private turn content is broadcast during discovery. Context reads go
only to configured trusted sources on points pinned to their attested owners by
existing authoritative-provider policy. Filtering replies after fanout would be
too late to prevent disclosure.

Read request: `{method:"context",turn,budgetBytes}`. `turn` contains only the selected
speaker's authorized observations and identifiers, never all secret GM state.
Response: `{blocks:[{id,kind,text,evidence,sourceRevision}],truncated:boolean}`.
Kinds are `lore`, `memory`, `psychology`, `rules` and `author-note`; no executable
macros. At most 16 blocks per source, 8 KiB per reply and the existing 64 KiB wire
ceiling. Aggregate usable context remains within the selected model's context
window after reserving output and required instructions.

The Narrator config explicitly selects sources and precedence; an untrusted
provider cannot promote its text to a system instruction using a high priority.
Stable order is configured source order then local block ID. Required rules and
current user input cannot be silently dropped. Optional blocks are omitted with
diagnostics when over budget. A missing required source fails the turn; a missing
optional source is visible but permits generation. Start with exact byte limits;
token-aware allocation uses the selected model tokenizer where available and
otherwise a conservative estimate, with visible overflow errors.

Lore implements keyword/semantic selection, multiple books, bounded dependencies,
unlock conditions and passive/directive modes. Dependency cycles cannot recurse
indefinitely. Directive mode is a trusted author setting, not a label a downloaded
card can use to grant itself authority. Memories and retrieved pages are data.

Context providers are read-only for this request. Post-turn Memory/Psychology
processing reads committed turns and applies its own updates. A slow or failed
optional processor does not block ordinary Chat. Sources do not call Narrator
recursively. One actor does not obtain another actor's private context because
both appear in the same scene.

### Operations and inference

Reuse begin/status/release, caller activation ownership, bounded slots and explicit
terminal errors. Cancellation prevents later publication/commit but cannot undo
network cost or an already committed result. Retained released-pending work counts
against quotas until completion. After restart an unfinished generation is
interrupted; show the durable conversation and offer explicit retry.

AI's existing selected-Narrator-only policy must be amended explicitly for Memory,
Psychology and tool workflows. Use exact selected workload owners, a common
bounded inference API and per-caller quotas; do not impersonate Narrator or give
all extensions inference access. Each workload may select a model from the existing
shared catalog, with primary as default. Model selection does not copy credentials
into World state or bypass each caller's native access policy.

Optional provider capabilities include tool calls, images, embeddings, sampling
controls and incremental output. Advertise supported settings; do not silently
ignore unsupported DRY/XTC/min-P/dynamic temperature or reasoning controls.
Thinking text is distinct from public dialogue and never implicitly becomes a
character memory or spoken response.

Streaming is provisional. A generation identity and increasing chunk number bind
text to one turn; a gap requires re-reading bounded accumulated status. The final
Chat commit remains authoritative. Stop, stale-context or failure discards the
uncommitted draft. Do not persist each token or invent durable streaming events.

## 4. Memory, knowledge and multiplayer are different problems

Memory processors checkpoint the committed source boundary together with their
own updates in one transaction. Resume by reading Chat, not replaying an event
queue. Source IDs/revisions prevent double learning. Edits/branch changes invalidate
derived memories whose evidence no longer belongs to the selected history. Make
the affected state unavailable for retrieval until recomputed or marked historical.
The checkpoint alone is not enough when older messages can change.

Semantic indexes are rebuildable derivatives keyed by model and source revision.
Keep canonical memory in World properties. Start with bounded lexical retrieval
and summaries; introduce a local index only when measured retrieval needs it.
Do not place an unbounded vector matrix in a property. Losing an index must not
lose memories. Budget background work per World/actor; no free-running LLM loop.
User-selectable full/balanced/light/diary-only modes control which processing runs.

Distinguish three protections:

1. Narrative perspective: construct different prompts for different characters.
2. Extension authority: only admitted readers/writers receive protected properties.
3. User/item authorization: an authenticated player can control one actor, not all.

The current component/property authority provides the second, not a finished
implementation of the third. A prompt saying “you do not know this” cannot enforce
either. One trusted Memory/Campaign owner can hold multiple actors' private state
and expose filtered queries. Do not give a potentially hostile mod broad property
read access and expect its query filter to protect the data it can already read.

Multiplayer requires authenticated session→player→controlled-item bindings checked
by the authoritative owner on every operation, including media and nested queries.
Browser-supplied `playerId` is never evidence. This is a later admission contract;
the current local browser bridge must not be exposed to LAN/Discord as a public
multiplayer API. This release plan includes designing that adapter at its delivery
gate, rather than declaring multiplayer supported by generic item IDs.

## 5. Media and voice

Avatars are presentations of an actor: static/GIF, Live2D and VRM backends resolve
asset references and actor presentation state. A model package declares its entry
asset and a closed relative-path→CAS resource map. Reject traversal, undeclared
remote URLs and missing dependencies. Export collects the whole declared bundle.
No assumption that a model is one small file or fits one transport message.

Emotion/motion labels are extension data mapped to installed model capabilities.
An absent animation falls back to neutral. Preserve an editable mapping, including
the reference's 28 emotion categories, without hardcoding them into Core.
Lip synchronization follows playback time, not token arrival time. Backgrounds,
ambient loops and generated images use the same asset ownership/portability rules.

TTS consumes an authorized immutable text revision and voice settings; produces
bounded audio segments with sequence, format and duration. STT consumes a live
audio session and returns partial/final transcripts. Voice conversion is an
optional media stage. Adapters cover Qwen3, XTTSv2, Kokoro, Silero, EdgeTTS and
ElevenLabs, plus RVC; installed providers declare actual availability/capabilities.
No engine is required to be embedded in every distribution.

Begin with committed-message Speak and push-to-talk. Then full-duplex calls add
VAD, interruption, echo handling, device selection and sentence streaming. A call
owns its generation, capture, playback and queued segments. Barge-in cancels the
old turn, stops playback and prevents late audio from restarting it. Switching a
view does not silently move a call to another World; user choice determines whether
background playback continues. Stopping that World always retires the call.

Transient audio/video is not written into CAS by default. Recording is explicit
and produces portable assets. Keep bounded buffers and backpressure; never queue
an entire unbounded recording. Existing 64 KiB control messages and buffered HTTP
are not proof of adequate real-time media throughput. The voice gate must measure
the complete capture→STT→inference→TTS→playback path and define segment/queue limits.
If current transfer APIs cannot support it, propose a bounded generic stream/asset
amendment once, rather than per-engine base64 tunnels or limitless quotas.

Image generation supports Automatic1111/ComfyUI and provider adapters for NovelAI,
DALL-E and FLUX. Results become CAS assets with provenance, then a Chat owner
operation attaches them to the intended unchanged message/branch. Unknown remote
outcome is shown explicitly; no automatic billable retry.

## 6. Desktop tools, native providers and model management

WASM is a confinement choice; “native” describes executable integration, not a
feature category or automatic sandbox. The existing release contract explicitly
does not implement supervised external executable packaging. Do not pretend
Live2D in a browser supplies Wayland overlays, OS input control or process management.

First native delivery uses an explicit desktop launcher/embedding that starts the
Host and optional companion services. It does not replace the entry point from a
running World. Keep GPU/model engines and OS integration out of the Host process
where practical; this improves crash isolation but is not security isolation.

A trusted native adapter exposes only its named tools/services through existing
attested provider mechanisms. External workers never receive raw Host/SQLite
access or an unauthenticated Zenoh session. The adapter owns caller checking,
request IDs, bounded queues, deadlines and process cleanup. A worker restart
invalidates pending calls; no automatic replay of OS actions. Initially launch
known executables explicitly; generalized native-package installation is not a
prerequisite for the first companion slice.

The platform contract must be frozen before implementing a native release:
supported OS/compositor, executable discovery, authenticated local IPC, process
tree termination, working directories, limits and unsupported-action errors.
On Wayland, screen/input/overlay access is platform-dependent and must be proven
on the declared compositor. Android uses platform application/IPC permissions;
arbitrary APKs do not inherit Rintawa authority. These are explicit release gates,
not an unspecified catch-all “native extension” escape hatch.

Tool providers describe an argument schema, result schema, effects and approval
policy. The orchestrator has finite step/time/token budgets and stops on unknown
outcomes. Approval binds provider, action and exact arguments; an LLM cannot approve
itself. Persist ordinary setup grants; require per-action confirmation where an
OS effect needs it. Expired confirmation denies the action. Tool output is untrusted
context, never authority to grant another tool. MCP is an adapter to this policy,
not permission to bypass it.

The tool set covers screen/clipboard observations, search/browser interaction,
input control, files, code execution, media control and system vitals. A scratch
directory and timeout do not sandbox arbitrary Python or shell code. Execute such
code only under an actual declared OS isolation policy or label it trusted native
execution and require corresponding consent. No blanket “safe because subprocess.”

Local model management is an optional native service: search/download, integrity
verification, progress/cancel, disk checks, backend installation and server lifecycle.
It serves normal inference adapters. Downloads resume only with verified content
identity. Failed updates leave the last usable backend selected. GPU acceleration,
KV cache and sampling capabilities are provider settings, never Core APIs.

## 7. Multiple presentations and multiple Worlds

Chat, desktop avatar and a game adapter can refer to the same actor in one World.
Each uses the same owner APIs and subscriptions; none keeps a second canonical
character. Minecraft maps external entities to World item IDs, validates observations
and sends intents to game-owned actions. Building requires an explicit bounded
plan, permission and progress/cancel; World properties alone cannot enact blocks.

A cross-world reference is `{worldId,itemId}` plus provenance, not a foreign key or
permission. Start with explicit copy/import or an authorized read adapter. An
adapter checks both source disclosure and destination acceptance. It cannot use
baseline fallback to touch a World where its component is disabled.

One continuous identity across Worlds requires choosing an authority: one home
World owns the character, while other Worlds hold references and local observations.
Offline copies are forks, not automatically synchronized people. Context from one
World is shared only by explicit policy; memory should not leak a secret campaign
into another. Cross-world moves/trades cannot claim atomicity across SQLite files.
Defer distributed transfer until a real transaction needs a recoverable protocol.

Keep these useful historical scenarios; discard universal identity registries,
projection trees and generic saga infrastructure that are not needed to implement
the explicit reference/copy workflow.

## 8. Functional coverage and delivery gates

Each row is required for the full target, not for the already working text MVP.
Provider integrations are independently installable. A row is complete only with
its real user path and failure path, not a manifest or placeholder component.

| Slice | Deliverable | Acceptance boundary |
| --- | --- | --- |
| Conversation | Cards, personas, greetings/examples, prompt ordering, macros, lore, edit/regenerate/alternatives, group turns | Continue after restart; Chat edit invalidates an in-flight answer; changed external source detected at revalidation; no duplicated assistant append |
| Memory | Recollections, relationships, diary, deduplication, correction, retrieval and processing modes | Restart without double learning; edit/delete source; rebuild index; two actors keep separate knowledge |
| Psychology | Drives/mood, decay, subjective relationships, optional autonomous initiative | Bounded updates/cost, pause/stop, deterministic decay inputs, no feedback loop of self-generated observations |
| RPG | GM, temporary/recurring NPCs, say/act/think/director, whispers, dice, inventory/status, objectives/clocks, hidden arcs, events and scene organization | AI cannot alter a committed roll; NPC does not receive hidden arc; failed prose leaves rules inspectable |
| Presentation | Static/GIF/Live2D/VRM, emotion/motion, lip sync, background/ambient scenes, typed configurable HUD and presets | Missing model asset, light theme, resize, isolated nested contributions, export/import complete bundle |
| Voice | Listed TTS families, conversion, VAD/STT, calls, interruption and sentence playback | Real devices; disconnect, late segment, stop/restart, bounded queues and latency measurements |
| Images | Local/cloud generation and contextual prompts | Correct message attachment, cancel/unknown outcome, portable output assets |
| AI settings | Multiple provider/model profiles, provider-specific controls and reasoning separation | Real local plus remote adapter; unsupported settings explicit; credentials excluded from export |
| Models | Repository browsing, GGUF download/cancel, backend install/update, supported acceleration | Disk-full/corrupt download, failed upgrade, child crash, no unbounded Host memory |
| Companion/tools | Overlay, observation, proactive conversation, planning, OS/browser/code/files/vitals/MCP tools | Exact approval, finite task, revoked access, process crash, platform-specific verification |
| External clients | Mobile browser and Discord, per-person conversations, image/GIF input | Authenticated client isolation, reconnect, filtered private knowledge; never expose the local root bridge |

Provider coverage includes existing Ollama/OpenAI-compatible endpoints and adapters
for Anthropic, Gemini, OpenRouter, DeepSeek, Grok, Qwen, Mistral, Z.AI and Player2.
No promise that one compatibility adapter implements every provider-specific option.
HUD supports bounded integer/boolean/list/text values and preset configurations;
the distribution should include the reference's 11 preset categories. Presets are
assets/configuration owned by the relevant extension, not Core code branches.

Implement in this dependency order:

1. [Responsive UI and composition](extensible-experience.md), persistence diagnosis,
   then Chat revision/alternative semantics and contextual actions.
2. Narrator context contract with lore and one Memory consumer; prove private
   knowledge and stale-context before expanding the number of processors.
3. Psychology and Campaign rules; prove one multi-actor scene with hidden knowledge.
4. One avatar and one TTS path; then additional adapters and full-duplex voice.
5. Native launcher/tools/models, with explicit OS and media release gates.
6. Authenticated external clients, then Minecraft and cross-world references.

Detailed DTO/schema authoring within these accepted semantics belongs to Sol,
adjacent to each extension implementation. Before each slice, freeze its exact
wire format, numeric budgets and errors; test the smallest complete scenario.
The native platform, real-time media and authenticated-client gates require a
narrow architecture decision based on concrete evidence. This document does not
authorize inventing those missing guarantees or representing them as finished.

## Complexity audit and limits

There is no new Core table, ontology, workflow engine, UI registry, universal
plugin hook or durable event bus. Revisions and provenance are necessary because
edits and deferred model output otherwise corrupt the story. Owner-specific APIs
are necessary because generic writes cannot enforce narrative/rule authority.
Shared transport helpers are justified where every consumer needs the same cleanup.

Do not create a new package merely for each row in the coverage table. Do not copy
Soul-of-Waifu's internal file layout or use its marketing claims as guarantees.
The current basis is suitable for incremental extension work; it is not yet a
proof of native isolation, multiplayer, low-latency voice, unlimited World size or
conflict-free cross-world interaction. Those guarantees need measured and reviewed
boundaries, not additional speculative layers today.
