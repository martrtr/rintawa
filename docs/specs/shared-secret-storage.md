# Shared local secrets

Status: accepted by the owner, 2026-11-25; Linux implementation and Web UI
acceptance completed. Reconciles
the generic-secret direction in local-credential-entry.md with the accepted
host-secret-configuration.md. Supersedes the fixed-AI-path prompt design. Do not
implement two competing credential managers. The owner-accepted
[Web UI amendment](secrets-web-ui-amendment.md) replaces the original native
input boundary, prompt quotas and window infrastructure described by earlier slices.

## Product contract

A user creates a named secret once, selects it in any extension's settings and
explicitly authorizes that consumer. Rotating the value updates all authorized
consumers on their next read. Removing one consumer does not delete the secret.
Secrets are local installation configuration, never World data or template assets.
The first use is API keys; other supported values are passwords, multiline private
keys and arbitrary bounded UTF-8 strings. Binary blobs, OAuth flows, synchronization,
password generation and a master-password manager are outside this slice.

Reference behavior: Obsidian documents named shared secrets and a create/select
control (https://docs.obsidian.md/plugins/guides/secret-storage). VS Code documents
encrypted extension secret storage, independent of workspace and not settings-sync
(https://code.visualstudio.com/api/extension-capabilities/common-capabilities).
We adopt reusable selection and separation from ordinary settings; these references
are not proof that Rintawa implements their encryption or security model.

## Current foundation and concrete shortcomings

SecretVault already abstracts store/load/delete; SecretManager checks component
read grants and rechecks revisions after vault I/O. Native stdin CLI and persisted
exact read grants work. SystemSecretVault uses service `rintawa` plus SecretPath,
without home isolation. WASM exposes read only. AI derives a path from connection
ID, so selecting a shared secret requires an explicit new binding, not duplicating
its value into several derived paths. Existing names/paths are not secrets, and
knowing one is not read authorization.

The pending native-input draft is useful but hardcodes a consumer-selected physical
path. Do not let extensions propose replacement/deletion of arbitrary shared values.
Selecting a value for use and administering its lifetime are distinct operations.

## Chosen model: entries and consumer slots

Keep the existing SecretPath WIT read argument as a consumer slot. An extension
continues requesting `ai.connection_<id>.api_key` or `mail.account_password`;
manifest secret-read coverage still applies. The Host resolves a local binding:

`(scope, instance, component, exact slot) -> secret entry ID`

An entry has immutable random ID, mutable user label, revision and bounded value.
The label is display text, not an authorization name or routing key. Different
consumers and different slots can bind the same entry. Labels may duplicate; the
trusted picker disambiguates them. Renaming never changes bindings. IDs are never
reused deliberately after deletion.

Extend existing exact secret-read approval metadata with an optional bound entry
ID; do not add another grant registry with independent allow/deny state. A bound
approval is both the selection and the exact read approval. Unbinding removes it.
Legacy approval rows without a binding retain their current direct-path semantics
for the existing backend. New picker-created rows use entry IDs. This compatibility
is for current credentials, not retired experimental World schemas.

After a read starts, revoke/rebind/stop must be checked against the same policy
revision before returning bytes. A captured old binding cannot authorize a later
read of its replacement. Rebinding is an atomic policy update, persisted and live;
success means both. Rotation may race with an already completed read: old bytes
cannot be recalled from an admitted extension or an HTTP request already sent.
Never claim otherwise.

Entry reads expose neither global metadata nor a list of other secrets. An approved
reader receives the actual value and must be trusted with it. Native extensions
running outside confinement are not controlled against direct OS access by this API.

## User experience and authority

Settings show a shared secret control: Unconfigured / Selected, Select or create,
Disconnect. The normal flow is one trusted picker showing the consumer, scope,
requested slot and selected entry, followed by one explicit “Use this secret”.
This creates the binding and read approval together; no second restart/consent for
the same grant. Network access remains its existing separate capability and can be
reviewed in the normal grouped setup flow.

Secret creation, value input and global management occur in a modal owned by the
explicitly authorized administrative shell. Ordinary settings iframes request a
picker with `secret-prompt`, receiving only admission and terminal outcome. The
shell separately requires baseline `secret-manage`; selecting a shell or claiming
identity in JSON does not grant it. The shell and web runtime are trusted with
submitted bytes; requesting iframes receive neither values nor the global catalog.

Host derives requester scope, principal, artifact and activation from delegated
execution, verifies its sibling and exact manifest-covered slot, and retains the
existing `SecretPromptAuthority`. Administrative imports use the shell's own
execution identity. Core owns native handles by scope/principal/activation; runtime
binds browser references to authenticated connections and channels. The amendment
defines exact limits, expiry, strict DTOs and retirement semantics.

Return only cancelled / bound / unavailable / stale / unknown. Native operations
run on bounded workers outside guest execution; metadata/results contain no values.
Cancel/release need ownership rather than a still-granted capability. Stop/trap
cleans up independently. Native commit admission orders cancellation against writes;
a previously admitted write may finish. Unknown writes are never automatically replayed.

The user opens global Manage Secrets from trusted Host UI/CLI, not via arbitrary
extension edit/delete intents. It lists entries and all persisted consumers across
baseline and installed Worlds, including inactive ones. It supports rename,
replace, disconnect consumer and confirmed delete. Replacement warns that all
readers will use the new value. Delete leaves existing references visibly missing,
never reassigns them to an entry with the same name; user can disconnect or rebind.
Deleting a World/package removes its approvals but never deletes a shared entry.

New value is masked by default; optional reveal requires direct user action.
Support multiline text without trimming, and paste without copying old values to
clipboard. Empty input is invalid, not deletion. Clear input on dismissal; never
log widget contents, secret bytes, backend raw errors, request bodies or CLI values.
Do not promise clipboard-history erasure. Check current SystemSecretVault error
logging: sanitize backend diagnostics rather than assuming Display cannot contain
sensitive material.

## Storage decision and compatibility

Keep storage behind SecretVault; CLI, runtime and prompt must receive the same
explicitly selected backend. Never silently fall back from a locked/unavailable
keyring to plaintext. Existing installations continue using system credentials
until the user explicitly changes backend. Do not silently move or erase entries.

The earlier owner direction permits a Linux file backend for installations without
a credential service. Deliver it as an explicit `local-file` choice, labelled
“not encrypted at rest; protected by OS permissions”. Preserve `system` as the
existing default until a separate default-change decision. This avoids accidentally
weakening existing installations while supporting keyring-free startup immediately.
Do not encrypt with a key stored adjacent to the encrypted file and claim protection.

Local-file entries live only under the selected home. Use one bounded record per
random ID containing label, revision and value so replacement is one atomic write.
No separate catalog database: enumerate bounded records in the trusted manager.
Maximum 256 entries, label 128 UTF-8 bytes, secret 1–65536 UTF-8 bytes, total decoded
value quota 16 MiB. Enforce encoded record bounds accounting for JSON escaping.
Existing profile grant quota remains in force; do not silently raise it.

Require private directory 0700, regular owner-owned files 0600 from creation,
symlink-safe directory-relative operations and safe parent ownership. Serialize
mutations across processes with existing lock patterns. Exclusive temp creation,
write/sync/atomic rename/directory sync; readers see complete old or new records.
Delete unlinks and syncs. Fail before publication without losing old value; after
publication sync failure reports durability unknown. Safely reclaim abandoned temp
files under the writer lock. Enforce bounds before allocations, reject corruption
and insecure permissions rather than treating them as an absent secret.

System-backed entries use random physical paths in the existing vault with local
nonsecret metadata required for listing. Stage value creation before publishing
metadata/bindings. Failure can leave an orphan but must not expose a partial entry;
report cleanup failures. Value replacement uses backend semantics; do not promise
cross-store atomicity. Existing direct CLI paths remain supported and can be
explicitly imported into managed entries by trusted native code. No browser readback,
automatic bulk migration or two-way synchronization. A backend switch without
migration produces unavailable/missing bindings, not an automatic credential copy.

New managed metadata/bindings are home-local. Legacy system direct-path behavior
remains account-global and must be labelled as legacy rather than represented as
home-isolated. Clone/template import carries neither bindings nor values. Full-home
copy/backups of local-file storage contain secrets; they are not portable World
exports. Exclude credentials from RTW exports, diagnostic bundles and telemetry.

## AI integration

AI keeps its derived connection slot and existing secrets.read call. Its settings
control selects an entry for that slot and exact backend. Several connections may
use the same entry, as may unrelated approved extensions. Endpoint/key-mode changes
already rotate connection identity: the new slot starts unbound; no implicit reuse
of the old credential at a new endpoint. Model changes within the same connection
preserve the binding. Removing a connection does not delete its shared value.

Do not put physical secret IDs, key paths or values into portable AI presets.
Consumers see only readiness and their existing logical slot. Missing secret,
denied read, unavailable vault and upstream rejected credential have different
recovery actions. Successful picker completion refreshes readiness immediately,
without restarting browser, World or Host.

## Implementation and acceptance

Sol must first reconcile this proposal with local-credential-entry.md and accepted
contracts; no competing prompt implementation. Keep Core changes generic: vault
selection, managed entry administration, binding/grant persistence and read resolution.
Desktop Host owns prompts; web runtime carries bounded intents with attested callers;
AI owns only the slot/control. Use a maintained native UI dependency suitable for the
Linux package; document its deployment requirements and prove real Wayland entry.
Headless Host offers native stdin CLI, never a fake successful GUI action.

Implement and verify in slices:
1. Backend injection and managed entry/binding lifecycle with fake vault, including
   persistence, atomic writes, two homes, corruption and revoke/rebind during I/O.
2. Native manager and picker with an injectable driver plus real graphical smoke.
3. Bridge intent admission and AI control; two different extensions read the same
   sentinel, an unapproved extension cannot list/read it; restart, rotate, revoke,
   delete, recreate same label and clone/export preserve the specified boundaries.

Test raw values never appear in bridge frames, catalog, World DB, exports, logs,
command arguments or normal errors. Exercise stale activation, revoked permission,
concurrent prompts, lost reply, missing graphical session, locked vault, write/disk
failure and all resource cleanup. Never modify real user secrets in tests. Supply
exact commands/revisions/logs; tests prove observed paths, not parity certification.

## Alternatives and complexity audit

Directly binding every extension to one shared global path is smaller, but forces
unrelated manifests to agree on that path and conflates a consumer slot with the
user's credential identity. Copying values into each slot breaks rotation and
sharing. One Host-owned binding at the existing approval boundary solves both;
no extra World table, cross-extension secret broker or plugin-to-plugin key sharing.
A shell-owned picker reuses the existing Web UI while preserving isolation from
the requesting settings iframe. Its shell/runtime trust boundary is explicit.

Per-model policies, master passwords, encrypted sync, remote secret managers,
secret leases and automatic OAuth refresh are not prerequisites for this slice.
The local-file backend does not satisfy an encrypted-at-rest requirement. Choosing
that stronger requirement means using an available system vault or separately
specifying a user-unlocked encrypted store, not hiding a new crypto design here.

## Implementation checkpoint — 2026-11-25

The original native input slice was superseded by the accepted Web UI amendment.
The browser acceptance below completed before removal of GTK/Python, child-process
orchestration and native-window APIs. Storage, bindings, CLI and policy fences remain.

- `SecretVault` is a trusted native injection boundary. Bound reads use
  `load_bound`, while legacy approvals still call direct `load`. This distinction
  preserves *all* existing system paths, including names resembling managed entry
  keys, without letting a managed binding bypass its home-local metadata check.
- `<home>/credentials/backend` holds the explicit selection; absent selection is
  `system`. A runtime captures its selected vault at startup. Changing selection
  affects newly constructed managers; a running Host must be restarted. Switching
  does not copy values, metadata or approvals between backends.
- `credentials/local-file/<random UUID>` holds one bounded JSON record including
  the value. `credentials/system/<random UUID>` holds nonsecret metadata and byte
  count, with its value in the platform vault. There is no catalog database.
  Linux uses directory-relative `rustix` operations, private modes, parent/owner
  validation, a stable writer lock and sync/rename/sync publication. Abandoned
  temporary files are reclaimed under that lock. An admitted reader may retain
  the complete old unlinked inode during replacement; hardlinks remain rejected.
- Native `HostRuntime::{secret_consumer_target,bind_shared_secret,
  disconnect_shared_secret}` validate exact persisted/live identity and update the
  existing exact approval rows. The separate `secret-administration` and
  `secret-prompt` imports expose bounded operations under explicit permissions. Offline legacy grants retain their documented next-activation
  behavior; the CLI does not pretend to perform a live managed rebind.
- `secret-backend [system|local-file]`, `secret-create <label>`, `secret-list`,
  `secret-rename <id> <revision> <label>`, `secret-replace <id> <revision> --confirm`
  and `secret-remove <id> <revision> --confirm` provide trusted administration.
  Creation/replacement use the existing bounded non-terminal UTF-8 stdin flow.
  `secret-import <legacy-path> <label>` explicitly creates a managed system entry
  and leaves the original intact. Direct `secret-set`/`secret-delete` remain
  system-only; local-file consumers use managed entries rather than slot copies.

Regression coverage includes fake system storage failures, no plaintext fallback,
two homes, quotas/escaping, stale revisions, corruption, insecure modes/links,
concurrent processes, pre/post-publication failures and abandoned-file cleanup.
Real RTW loading with a synthetic native target verifies two different extensions
sharing/rotating/rebinding a value, an unapproved third reader, restart, disconnect,
missing references after deletion/recreation and World deletion retaining entries.
Template acceptance verifies managed IDs/labels/values and approvals stay out of
exported contents and fresh same-home/clean-home copies. Existing Engine regressions
exercise revoke/rebind during vault I/O and stale activation admission. These are
observed native paths, not graphical/WASM/browser parity certification.

Reproducible check: `bash scripts/check.sh` at `e63ed0f` plus the recorded worktree
hashes. Evidence and raw logs are under `/tmp/rintawa-shared-secrets-20261125/`;
`check-evidence.json` records the command, toolchain, exit status and source hashes.
Filesystem checks run outside the Codex sandbox because its mapped root ancestors
appear owned by `nobody`; production ownership validation is not relaxed for tests.
All values are synthetic and all credential mutations use scratch homes/fake
vaults. The storage checkpoint below predates the native slice; its transport
failure is retained as evidence, not silently rewritten by subsequent checks.

Storage checkpoint validation: scan/style/fmt/Clippy, rustdoc with warnings denied, CLI build
and 24 stdin CLI probes passed. That historical unfiltered `scripts/check.sh` was
not green: the existing
`nonblocking_tests::test_hosted_same_store_query_reply_is_scope_bound_and_keeps_pump_available`
timed out in parallel runs, while its isolated rerun passed. The remaining
workspace suite passed (532 tests) with that exact test explicitly skipped; see
`remaining-evidence.json` and `transport-recheck-evidence.json`. Its root cause
remains unresolved; no transport code was changed. A single-thread diagnostic
attempt was stopped because the existing Storage subprocess test expects an exact
`writer-ready` stdout line that the serial test harness prefixes with its test
name; see `serial-check-evidence.json`. This is not a full-suite pass claim.

## Web UI checkpoint — 2026-11-25

Core provides write-only asynchronous administration and caller-attested prompt
imports. The web runtime executes them under original delegated owners, checks
channel ownership and retains cleanup work. The shell renders the picker/manager;
AI Provider requests selection instead of showing terminal commands. Creation and
Use remain separate commits; successful Use installs the exact read grant live.

A real Chromium/RTW/WASM run passed in `/tmp/rtwkit-secrets-wpiGU2/`: shell without
secret-manage denied; forged iframe administration and cross-channel prompt refs
denied; masked input persisted in the selected local-file vault, absent from the
requester iframe/browser storage/logs; two consumers shared and rotated a value;
disconnecting one preserved the other; permission revoke and Host restart worked.
The reproducible script is `rtwKit/scripts/shared-secrets-smoke.mjs` with
`RINTAWA_BINARY` and `RTWKIT_SMOKE_ARTIFACTS`. Later final logs supersede these
intermediate worktree results; no real credentials are used.

Final validation for this slice is recorded in
`/tmp/rintawa-web-secrets-20261125/`: both repository `scripts/check.sh` runs passed;
Core ran 547 tests, plus the separately enabled real-RTW template test; rtwKit ran
105 Rust tests, 35 bridge/UI tests and its Story recovery checks. The final
Chromium run passed eight scenarios (`browser-release.log`, raw browser evidence
`/tmp/rtwkit-secrets-WXAG7V/`). That run adds actual requester teardown before
confirmation, lost admission acknowledgement without replay, Unicode/CRLF paste,
manager rename/delete and restart to the earlier checkpoint. Metadata and native
worker tests cover expiry, stale artifact/revision, foreign cleanup, permission
revocation, disabled World consumers and blocked admitted I/O retention.

GTK/Python window infrastructure and its dependencies/API/flags are absent from
production sources. The normally ignored template fixture was updated only to
include the current required Character `greeting` field. Linux local-file browser
acceptance uses synthetic secrets in scratch homes; system-vault failures are
covered with injected vaults, without modifying real user credentials.

The final browser run also rejects an oversized encoded request before native
admission and offers native CLI stdin, without truncation. The final rtwKit check
record is `rtw-release-check-evidence.json`; Core is `core-final-check-evidence.json`.
All production source hashes stayed unchanged throughout their check runs.
