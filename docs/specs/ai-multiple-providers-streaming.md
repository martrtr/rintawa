# Concurrent AI connections and streaming

Status: proposed amendment, 2026-11-24. The owner requested this slice for the
current work. It changes the existing rtwKit inference contract and its
Narrator-only admission; it does not change Core's permission or transport model.
The accepted Chat streaming slice below is the implementation baseline. The owner
has additionally selected Stop retaining received text and requested implementation
of the foundation. Delivery begins with that Stop path. Unrelated consumer/model
selection proposals remain outside this implementation slice. Companion documents:
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

## Explicit Stop product decision (2026-11-25)

The owner selected saving the received text when pressing Stop. This updates the
previous unaccepted proposal to discard a prefix on explicit cancellation; it does
not change the accepted network-failure retention block above. Narrator freezes
its validated prefix, cancels inference and saves through ordinary Chat revision
guards. Empty output saves nothing. See [foundation Stop semantics](package-and-conversation-foundation.md#3-explicit-stop-with-retained-text)
for races, admission, resource accounting and the difference from World stop.
Implementation adds owner-bound Narrator `stop` and Provider `cancel`; no native
API amendment is included. The existing worker timeout limitation remains explicit.

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

Proposed revision, 2026-11-25: use explicit AI-owned caller grants, not a new
single `rintawa.inference.consumer` role. Core Single resolution can automatically
choose the first eligible provider; registration/selection is discovery, not user
consent to spend credentials. A mandatory coordinator would also force unrelated
Memory, Studio and simulation extensions through one privileged intermediary.

Preserve existing exact selected-Narrator admission. Default-deny additional
callers until trusted Settings explicitly grants the exact World scope, extension
instance/component, allowed connection IDs and inference capabilities. Store this
bounded policy in existing AI-owned local preferences, never World properties or
an imported self-authorizing manifest. Define the privileged settings method and
limits before implementation. Local permission state must be remapped or explicitly
reapproved when instance/World identity changes; an imported template grants none.

Multiple independent consumers may be admitted. An allowed consumer chooses only
from its allowed connections; choosing primary resolves it first, then checks the
resolved connection against that policy. Changing global primary cannot silently
give a caller a different paid/cloud endpoint. Connection/model/capability checks
remain request-scoped and preserve the shared catalog and secret authority.

Check attested immediate caller, World, component and active execution at admission.
Do not accept a caller identity or authorization scope from request JSON. An actor
ID is domain context, not caller authentication. Baseline callers remain denied
unless a separate accepted administrative operation explicitly permits them.

Consumer catalog listing returns only permitted connection IDs, labels/default
models and primary indication, without endpoints/secrets. Existing trusted sibling
Settings administration retains its separate catalog-management authority. Status/release/cancel and streaming
data remain scoped to the operation's original exact caller activation. A different
World, restarted activation or guessed operation ID gets invalid-operation.

Use bounded per-caller admission within the existing total operation/HTTP budget;
no unbounded waiting queue or claim of reserved throughput. A trusted settings
change is atomic at the policy owner. Revocation prevents new admission immediately
at the next checked operation, retires affected local pending work/publication
under existing live policy, and cannot undo remote cost or already disclosed input.
Define and test the cancellation race rather than promising instant upstream stop.

Acceptance must use real WASM callers: an unapproved self-registered provider is
denied; two explicitly admitted independent consumers can use different connections;
wrong scope/component, primary-switch escape and stale activation reject. This
changes the proposed extra-consumer design only. It does not broaden the previously
accepted Chat streaming slice.

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

Explicit inference cancellation is distinct from Narrator's user-facing Stop.
Narrator freezes its own last validated prefix when Stop is accepted, ignores later
provider growth and propagates cancellation before releasing AI. It deliberately
retains a nonempty frozen prefix through the ordinary captured Chat append guards;
the cancelled provider result itself does not claim successful generation or save.
If an append has already been submitted, reconcile its actual outcome instead of
submitting another. Stale context and unknown outcomes remain explicit failures or
uncertainty, never a reason to replay or force a write. Empty Stop creates no message.
Repeated Stop is idempotent while retained. World/activation stop cannot initiate
this retention path. Existing known-network-failure retention remains unchanged.


Concurrent processing must be fair and bounded: preserve the total 16 operation
limit, existing HTTP and per-poll byte/time limits; a slow operation cannot prevent
others from making progress. Reject overload clearly; never queue unlimited work.
Confirm provider/Host ceilings in source and report actual observed concurrency;
16 is admission capacity, not a guarantee that 16 remote requests succeed.

## Chat path

Narrator forwards pending AI snapshots via its own operation status, bound to the
captured conversation and operation. Chat displays the prefix as a provisional
assistant response. Successful completion, accepted known-failure retention and the
proposed explicit Stop path all require the existing `expectedLastEntryId`,
conversation revision and triggering-message revision validation before persistence.
Switching the visible chat detaches observation; it does not redirect a pending
operation or implicitly issue Stop. World/activation retirement prevents new writes
from retired work; stale context rejects through ordinary guards. A late response
cannot replace newer UI state. The user can press Stop from Chat; Narrator freezes
and retains eligible text, propagates cancellation to AI and releases operations
according to their real completion and resource lifecycle. Persisted messages
remain readable after restart;
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
empty failures create no assistant. Show two independently admitted consumers making concurrent explicit
model requests without network/key capabilities. Retain focused command, revision,
exit status and raw logs. Optional real OpenRouter/Z.AI smoke requires an explicitly
provided credential and consent to any paid use; lack of it must be reported, not
represented as provider certification.

Sol: inspect current working trees first and do not overwrite another worker's
uncommitted changes. Implement in rtwKit with only a demonstrated minimal Core
fix if existing primitives cannot support it; an accepted Core contract change
requires a separate owner decision. Do not bundle unrelated CSS/transport edits.
