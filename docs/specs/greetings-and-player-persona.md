# Alternate greetings and player persona (F3b)

Status: proposed concrete amendment to sections 5–6 of
[the foundation contract](package-and-conversation-foundation.md).
The product goals are authorized; the new persistence and command contracts below
need owner acceptance under the repository's architecture policy.

## Grounded starting point

In rtwKit, `packages/character/src/model.rs::Card` requires an imported source
asset. `Greeting` stores only one validated greeting, and `Import` and its schema
reject unknown fields. `character/src/wasm.rs::Stage::Normalize` discards alternate
greetings from the runtime record; the character-library parser retains them in
the CAS source. Imported cards currently have no editing command or revision.

`packages/chat/src/model.rs::Conversation` contains characterId, title, revision
and lastEntryId. Its schema is strict. `history.rs` creates independent membership
relations sharing immutable bodies. `BeginCreate` admits selected Character
workflow callers as well as Chat UI; other mutations have narrower admission.

`packages/narrator/src/wasm.rs::advance` reads recent Chat context, then Character
and source asset. `model.rs::card_text` substitutes `You` for `{{user}}`.
`append_request` retains the original conversation and user revisions. Generation
already has a 24 KiB prompt budget and a separate encoded request bound.

## Scope and invariants

Deliver selection of imported primary/alternate greetings before creation and
navigation between their independent persisted chats; create, edit, duplicate,
delete and select a player preset in the World; use its captured name/description
in Narrator. No Core, WIT, SQLite table, AI Provider, package-manager or generic
preset registry changes. No character-card editor in this slice.

Keep all existing stored @1 schemas unchanged. New domain properties use the
existing property registry and owner permissions. Missing optional properties
mean legacy behavior. Malformed present properties fail explicitly.

All mutations use existing activation-owned operations, caller-known result IDs,
bounded preparation, ordinary atomic World commands and canonical reconciliation.
Never retry a mutation because its outcome is unknown. No durable jobs or receipts.

## Character ownership and storage

Add `rintawa.character.persona@1` on ordinary object items:
`{revision, name, description, portrait}`. Revision is the existing canonical
decimal u64 string convention; creation starts at `0`, edits increment with
overflow rejection. Name is nonblank, at most 512 UTF-8 bytes; description at most
8 KiB UTF-8; portrait is null or a validated PNG AssetRef using existing limits.
Check encoded property/response bounds too. This is authored fictional character
data within Character, not a second identity system. It does not require the
imported `card@1` property. Imported-card listing and persona listing are explicit
queries; neither mistakes the other kind for an imported source card.

Persona create/update/duplicate/delete are admitted only from selected Character
UI. Update/delete require expectedRevision. Duplicate captures a specific read
revision and creates a caller-known new object; source remains unchanged. Delete
removes only the persona property and retains the object, its foreign properties
and relations. An object without the persona property is absent from persona
list/get. This avoids cross-extension deletion authority and cascading data loss. Chat stores a
non-owning UUID reference rather than a relation so a deleted preset can be
reported as unavailable. Do not garbage-collect portrait assets here.

Expose bounded list/get-persona reads through Character's existing service point
with selected-provider identity checks and existing read admission. List summaries
omit description; paginate by item ID. Get returns one complete validated revision.
No arbitrary caller-provided data is accepted as an authoritative Character read.

Greeting candidates are immutable for the current imported card. Their revision
is the exact persisted source AssetRef digest. Candidate IDs are `primary` and
`alternate:<canonical original ordinal>` scoped by card item ID AND source digest.
An ordinal alone is never a durable identity; duplicates remain distinct choices.
There is no need to invent per-candidate UUIDs or a mutable card revision counter.

Persist normalized candidate text as an immutable JSON CAS asset, referenced by
new Character-owned `rintawa.character.greetings@1`:
`{sourceDigest, candidates: AssetRef}`. Upload before publishing its reference.
The asset contains the ordered candidates and each candidate's validation result,
using existing 16 KiB text / 48 KiB encoded-message limits. Empty and oversized
entries remain identifiable but disabled; never silently truncate text or reorder
indices after filtering. The serialized derived asset is capped at 8 MiB; construct with a bounded writer
and reject overflow as `greeting-index-too-large` before publishing the import.
This explicit import limit affects pathological cards with enormous candidate
counts; no candidate is silently dropped. Measure peak memory and fuel on the
limit and on many tiny candidates before declaring completion.

This avoids placing an entire card's potentially multi-megabyte greeting list in
one 64 KiB RPC or 1 MiB World transaction. Character owns paginated summaries and
one-candidate retrieval, with bounded per-activation asset reads and cleanup.
Page responses are bounded by encoded bytes as well as item count; summaries
contain ID/status and a bounded preview, not full candidate texts.

Existing cards without the new property keep their persisted primary greeting.
Do not silently write during reads or require reimport. An explicit Character UI
action may derive/persist alternatives from the exact existing CAS source; source
unavailable/corrupt is an explicit failure, retaining the original primary path.
Original source bytes and normalized asset both travel with World export/import.

## Chat ownership and storage

Add optional Chat-owned `rintawa.chat.context@1` to conversation objects:
`{personaId, greeting}`. personaId is null or a UUID. greeting is null or an
immutable provenance record containing groupId, cardId, sourceDigest, candidateId,
resolved persona ID/revision/name and original greeting message ID. A missing
property means no persona and no recorded greeting family. No full persona
description or whole candidate list is copied into every conversation.

Add explicit context-aware creation and persona-selection commands rather than
changing strict @1 commands. Chat UI and the selected Character workflow can
create; only Chat UI can change a conversation's selection. Selection checks
expected conversation revision/head, validates a present persona, writes context
and increments the existing conversation revision atomically. Clearing an invalid
or deleted reference is explicit and permitted. Same-selection requests are no-ops
after validation. A new selection cannot silently refresh a pending answer's
revision guard. UI asks users to finish/Stop before selection while generation is
known pending; authoritative stale-context validation still handles races.

Fork construction copies context in the same pinned snapshot and transaction as
the transcript. Each fork owns its selection. Shared persona edits affect future
reads by all selecting chats, while Duplicate provides independent authored data.
Stored message text remains immutable; no new historical speaker-label schema is
needed in this slice: old and new transcript labels keep `You` behavior, while the
selected preset is shown in the conversation settings and used for narration.

For greeting creation, Chat resolves a candidate from the selected Character
provider and captures one persona revision before macro expansion. Resolve only
known literal `{{char}}` and `{{user}}` tokens in one pass; replacements are never
recursively interpreted. Unknown tokens remain literal. Validate expanded text
against existing message limits before publishing anything. No-persona uses `You`.
Store the resolved result as an ordinary assistant message. Character-owned data
remains descriptive; no tools or World authority result from macros.

Cross-owner preparation is a captured snapshot, not a claim of a multi-service
transaction. Source cards are immutable; persona edits after capture do not
invalidate this captured greeting. Chat's own source revision/head is rechecked
by the System on its pinned snapshot before branch publication.

Initial draft arrows preview candidates and create no chats. First creation makes
a groupId equal to that new conversation ID. On a persisted greeting, arrows
select a saved family member for that candidate, or create a new chat containing
only its newly resolved greeting, with no old suffix. New siblings preserve the
family's captured persona identity/revision/name for greeting expansion; a missing
persona description does not prevent using that captured name. Each new chat's
future persona selection starts as the source chat's current selection.

Family navigation must not silently choose among multiple edited continuations.
Only explicitly created greeting siblings are navigation targets. Ordinary F2
forks retain descriptive greeting provenance but are not registered as canonical
family targets; editing/deleting the greeting makes its controls unavailable in
that resulting chat. Record family target lookup as a separate optional Chat-owned
property on each canonical sibling, keyed by groupId/card/source/candidate, with
no mutable map duplicated across every family member. A bounded paginated scan
of that property is sufficient initially; no new database index service/table.
The authoritative command checks for an existing target in its pinned snapshot,
so two clicks cannot publish duplicates. A read never creates a family member.

Deleting the original chat does not own/delete siblings. If a canonical sibling
is absent, a later explicit choice may recreate it with a new known ID. Corrupt
duplicate targets fail with `ambiguous-variant`, never pick an arbitrary chat.
No card edit API is introduced; a future editor must define a new source revision
and cannot silently remap existing families to current indices.

## Narrator and UI

Chat recent reads include the captured optional persona selection alongside the
existing conversation revision. Narrator reads one complete persona revision from
Character before inference and retains that immutable value. A missing/deleted
selected persona fails `persona-unavailable` before inference; user can explicitly
clear/change it. No-persona follows the current path. Persona edits after capture
affect future requests, not an in-flight prompt. Selection changes increment Chat
revision so the existing append guard rejects an old answer.

Use the captured name for `{{user}}` in card prompt fields. Include the description
once as labelled descriptive player context in Narrator. Preserve existing prompt
budgets; an oversized combined context fails explicitly, without truncating the
preset or silently dropping the current user message. AI Provider receives the
same ordinary inference message contract.

Use Chat-owned previous/next controls on the first greeting, with candidate label,
disabled invalid candidates, and visible busy/error state. No new widget write
authority. Character's existing app owns the compact preset list/editor with
Create, Duplicate and Delete. Chat's New conversation and settings select a preset
or explicit None; the selected name is visible before generating.

## Required acceptance

1. Primary/alternates, duplicates, empty/oversize/escaped text, paginated large
   lists, macro expansion overflow, missing assets and explicit legacy extraction.
2. Draft arrows create nothing; saved choice creates one listed sibling; revisit
   restores its continuation. Concurrent choice, stale source, lost acknowledgement,
   forged candidate/source and family duplicate rejection publish no partial chat.
3. Edit/delete original greeting hides invalid controls; ordinary forks never
   become ambiguous canonical targets. Deleting original cannot erase siblings.
4. Two presets, edit conflicts, duplicate independence, no-persona, missing preset,
   property-only deletion preserving foreign properties/relations.
5. Persona edit during inference uses one captured revision; Chat selection race
   rejects stale append. Stop retains its normal guarded-prefix behavior.
6. Existing @1 chats reopen; new properties/normalized assets survive Host restart
   and portable World roundtrip. Final real WASM/browser Chat, widget and streaming
   regressions pass with exact package hashes and raw evidence.

The new Chat choice command carries sourceConversationId, expectedConversationRevision,
expectedLastEntryId, candidateId, sourceDigest, caller-known conversationId,
messageId and entryId, plus the validated captured greeting/context. The ingress
owns Character resolution; arbitrary clients cannot submit this internal command
as proof of resolution. The System checks source/family provenance, existing
canonical target, result ID collisions and final encoded transaction size on one
snapshot. Existing targets are returned by preparation; a concurrent winner makes
the admitted command fail `variant-exists`, followed by a safe canonical read.
No duplicate target is published. Persona selection carries conversationId,
expectedConversationRevision, expectedLastEntryId and personaId (nullable).
Unknown fields, malformed UUIDs/revisions and non-owner callers are rejected.

Implementation may choose private stages/structs and batch sizes inside these
contracts. If real WASM measurements cannot fit the derived index or family scan
within current fuel/memory/deadline limits, report the measured case and narrow
this proposal before changing shared budgets.
