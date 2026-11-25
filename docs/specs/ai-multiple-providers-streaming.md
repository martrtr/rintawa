# Concurrent AI connections and streaming

Status: proposed amendment, 2026-11-24. The owner requested this slice for the
current work. It changes the existing rtwKit inference contract and its
Narrator-only admission; it does not change Core's permission or transport model.
Only the accepted Chat streaming slice below is authorized for implementation.
Other proposed changes still require owner acceptance. Companion documents:
[narrative](narrative-and-companion.md), [settings and AI](extension-settings-and-ai-connections.md).

## Accepted Chat streaming slice (2026-11-24)

The owner's request to make streaming work in Chat authorizes the streaming path
only. Concurrent connection selection, extra consumer admission, named presets and
explicit cancellation below remain proposed and are not part of this change.
Core and World storage are unchanged.

AI Provider and Narrator `status` may include optional cumulative provisional
`text` while pending. It is a non-destructive, append-only UTF-8 snapshot, bounded
to the existing 16 KiB answer limit and 60 KiB encoded response envelope. Existing
consumers ignore pending fields; no strict pending DTO was found in the active
path. Missing text means no new snapshot. Narrator retains the prefix while its
validated final answer is being committed. Only terminal `committed` makes the
answer canonical; completion, failure, unknown outcome and cleanup retain their
existing meanings. Status/release ownership and admission quotas are unchanged.

Chat displays one transient assistant row tied to its exact operation,
conversation and future message ID. Before any text, show one typing indicator;
during text, use only a cursor. Errors occupy one aligned row with an explicit
retry action. No duplicate Working/Generating captions or visible field label
while busy. Unknown write outcomes still require reconciliation without replay.

### Interrupted text retention — accepted by the owner, 2026-11-24

A broken stream with a nonempty validated prefix is saved as an ordinary assistant
message, using the existing assistant append command and its captured conversation
and user-message revisions. No new property, Core API or permission is introduced.
Provider terminal failure may include optional cumulative `text`, within the same
16 KiB/60 KiB bounds. Narrator validates that it extends any observed prefix; when
absent, it may retain the last valid pending snapshot. The failed provider result
remains failed; Narrator's operation becomes committed only after the Chat write
actually commits. A failure without usable text remains an error. Invalid snapshot
progression, write limits, stale context and inactive authority still reject.

Stored interrupted text is ordinary conversation history and survives reload,
restart/export. It receives no persistent failure badge. The UI replaces its
preview with the canonical message and does not show a second failure notice after
successful retention. Explicit retry remains available for an empty failed reply;
regeneration/editing of an already stored answer is outside this slice. Stop/trap
cannot start a later write from a retired activation. Unknown admission/commit
results are not reclassified as interrupted success and never replay automatically.

Acceptance: incomplete SSE, network failure and timeout after valid text retain one
normal message; failed empty streams retain none; repeated snapshots do not duplicate
text. The retained prefix is captured by its owner, not accepted from arbitrary UI
payloads. A concurrent conversation change still prevents the write. Pending release
keeps admission accounting; stop/restart preserves committed history without replay.

Existing Host limitation found by the silent-upstream stop check: dropping an
owned HTTP handle sets cancellation immediately, but the worker checks that flag
between blocking `response.read` calls. A silent read may retain its worker/socket
until data arrives or its request timeout expires. Stopped World contexts cannot
commit a partial answer. Prompt interruption of silent I/O needs a separate Core
fix; this streaming slice and a heartbeat-based test do not establish it. See
[HTTP worker](../../crates/extension-engine/src/runtime/wasm/http_request.rs).

## User outcome

A World can run a Chat response while a simulation requests decisions for several
actors, each using a chosen connection and model. No request changes the global
primary. Text appears during generation. AI Provider alone owns network/key access;
the simulation submits inputs and receives text, and its rules validate decisions.

## Current behavior and scope

The pre-slice rtwKit `packages/ai-provider/src/wasm.rs` admitted up to 16
operations, polled HTTP asynchronously and parsed SSE, but returned accumulated
text only at completion. The accepted Chat slice now exposes pending snapshots. `begin-generate` accepts messages, uses primary's configured
model, and admits only the World's exact selected `rintawa.narrator` principal.
Released pending operations retain their slot until finished. Catalog and secrets
remain baseline-owned, with existing World-scoped grants. Do not replace this with
a new scheduler or another streaming service.

## Connection and model

Extend `begin-generate` with optional `connectionId` (canonical UUID) and `model`
(nonempty UTF-8, maximum 160 bytes, no control characters). Omitted/null
`connectionId` resolves the current primary; omitted/null `model` uses that
connection's saved model. Explicit model overrides only this operation, never saves
settings or modifies primary. Require an exact catalog ID, never accept a URL,
secret path or arbitrary HTTP headers in a request. Unknown/deleted connection,
missing primary, invalid model and unavailable catalog return distinct stable errors;
no fallback to another connection/model. Existing requests remain valid.

At admission capture the selected catalog record, endpoint, model and sampling
configuration in the work item. Concurrent edits affect later requests only.
Normal secret and origin grants are checked for the captured connection in that
World; selecting a connection never grants its credentials or network access.
Admission failure creates no operation. Secret values and raw upstream error bodies
must not appear in status, logs or UI. Revocation follows the existing live-policy
contract even for in-flight requests. An accepted request may incur remote charges
regardless of later cancellation or timeout.

Provide an owner-filtered catalog description to authorized consumers: connection
ID, label, default model and primary flag; no key, endpoint or unrelated settings.
Model discovery may reuse the existing Settings path; callers can supply a model ID
without requiring discovery to succeed. A failed model-list response must not be
confused with an empty list. Do not change shared preference authority to expose
raw catalog directly to arbitrary consumers.

## Consumer admission

**Source-review correction (2026-11-25):** Core Single resolution can automatically
choose the first eligible provider without an explicit preferred selection.
Consequently, the selection proposed below does not itself prove user consent.
Before implementing extra-consumer admission, specify an AI-owned explicit
scope/instance/component-to-connection allow policy using existing preferences and
trusted Settings administration. Default-deny extra consumers. Contract registration
alone must never authorize spending a connection's credentials. The following
selection mechanism is discovery, not a replacement for that permission check.


Keep exact selected Narrator admission. Additionally allow a World's explicitly
selected single provider of a new `rintawa.inference.consumer@1` contract. This
is an application-level selection, resolved through existing selected-provider
machinery, not a Core role, self-declared authorization or a blanket grant to every
installed component. The selected simulation coordinator is the consumer; its
agents need no AI, HTTP or secret permissions. Recheck the attested immediate
caller, World, component and activation for every operation. Baseline remains
denied. Selection and manifest setup are part of composition; document exact
configuration and errors, including zero/ambiguous selection. Verify this use of
existing registration/selection actually works in real WASM before relying on it;
if it cannot, stop and propose the smallest admission amendment. This first slice
supports one extra selected coordinator per World; do not invent a generic ACL or
claim unlimited simultaneous independently authorized consumer packages.

Catalog listing is available only to those same admitted callers and the existing
sibling settings principal. Status/release/cancel and streaming data are scoped to
the operation's original exact caller activation, not merely to a contract name.
A different World, restarted activation or guessed operation ID gets invalid-operation.

## Operations, streaming and cancellation

Keep begin/status/release. Extend `status` compatibly with cumulative provisional
`text` while pending; completed retains `{status:"completed",text}` as today. Status
is a non-destructive snapshot: repeated reads return the same prefix until new text
arrives. UTF-8 text is append-only per operation and limited to the existing 16 KiB
answer ceiling; a bounded full snapshot fits the existing 60 KiB JSON envelope even
with escaping, otherwise fail `answer-too-large`. No token queue, offsets, durable
log, pub/sub delivery guarantee or extra storage. Polling is sufficient for the
existing short answers and recovers missed polls. Each operation's SSE decoder,
HTTP handle, accumulated text and captured connection are independent.

A pending snapshot is provisional. The accepted interrupted-text amendment above
allows a known broken stream with a validated prefix to enter the ordinary
revision-checked Chat append. Provider failure itself never proves a World commit. Completion requires the existing
validated HTTP/SSE termination, nonempty final text and bounded payload. Malformed
SSE, upstream disconnect and timeout must not appear as successful final output.
Existing callers that only inspect pending/completed must be checked for strict DTO
parsers; update both sides and tests together. If adding fields breaks an existing
strict wire consumer, introduce an explicitly versioned status method and retain
old status, rather than silently changing the old shape.

Add `cancel {operation}` for the original owner: close the HTTP handle promptly,
transition to terminal `failed` with `cancelled`, retain the result until release.
Cancel after completion returns the existing terminal state and cannot undo a
completed result; repeated cancel is idempotent while retained. `release` keeps its
existing semantics: release pending does not imply cancellation, remains charged
against quota until work terminates; terminal release frees the slot. Stop/trap
retires owned handles and no further output/commit may be published. Cancellation
is best effort for remote execution and billing. A lost begin reply has unknown
outcome: do not automatically retry. No automatic replay of billable requests.

Concurrent processing must be fair and bounded: preserve the total 16 operation
limit, existing HTTP and per-poll byte/time limits; a slow operation cannot prevent
others from making progress. Reject overload clearly; never queue unlimited work.
Confirm provider/Host ceilings in source and report actual observed concurrency;
16 is admission capacity, not a guarantee that 16 remote requests succeed.

## Chat path

Narrator forwards pending AI snapshots via its own operation status, bound to the
captured conversation and operation. Chat displays the prefix as a provisional
assistant response. It commits only after Narrator receives successful completion
and existing `expectedLastEntryId`, conversation revision and triggering-message
revision validation succeeds. Switching conversation,
World stop and stale context prevent new writes from the retired/stale operation.
A known generation failure may retain validated text as specified above; it must
never attach to a different conversation. Explicit cancellation remains proposed. A late response cannot replace
newer UI state. The user can cancel from Chat; ensure Narrator propagates cancel to
AI and releases both operations. Persisted messages remain readable after restart;
unfinished generation is not replayed; a retained prefix survives only after its
ordinary Chat transaction commits.

## Provider choices

In Settings add OpenRouter and Z.AI as named presets over the existing
OpenAI-compatible adapter, alongside Ollama and custom OpenAI-compatible. Presets
prefill verified official base URLs, require API key by default, leave model
editable, and do not make a request or grant access on selection. Verify endpoints
and SSE/model-list compatibility against current official provider documentation
before implementation; record exact URLs/source and test evidence. If one provider
needs different protocol semantics, implement only its minimal adapter and document
it; do not claim generic compatibility without evidence. Keep existing catalog
serialization and connection IDs: named presets are UI defaults, not new persisted
protocol kinds unless a proven protocol difference requires a separate amendment.
Do not hardcode a changing model catalog; allow manual model ID and existing list.
No third-party SDK or universal adapter framework is required for two presets.

## Checks and delivery

Use deterministic local fake OpenAI-compatible servers, not paid credentials, for
regression evidence. Exercise two connections and distinct models concurrently;
verify exact request destinations/model bodies and that primary never changes.
Cover changing/deleting a connection mid-flight, disabled/denied connection,
missing/rotated credentials, primary absent, invalid/unknown IDs, cross-World and
foreign activation rejection, operation exhaustion/fairness, stop/restart, pending
release and cancel races. Exercise fragmented SSE/UTF-8, empty/malformed stream,
HTTP error, disconnect, oversized/escaped data and a missed status poll.

Run the real browser Chat → Narrator → provider path against a delayed fixture:
observe a prefix before completion, then one committed answer; a known stream interruption retains a validated prefix through Chat COMMIT;
empty failures create no assistant. Show simulation coordinator making concurrent explicit
model requests without network/key capabilities. Retain focused command, revision,
exit status and raw logs. Optional real OpenRouter/Z.AI smoke requires an explicitly
provided credential and consent to any paid use; lack of it must be reported, not
represented as provider certification.

Sol: inspect current working trees first and do not overwrite another worker's
uncommitted changes. Implement in rtwKit with only a demonstrated minimal Core
fix if existing primitives cannot support it; an accepted Core contract change
requires a separate owner decision. Do not bundle unrelated CSS/transport edits.
