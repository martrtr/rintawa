# Native local secret configuration

Status: accepted by the owner, 2026-11-23: CLI stdin into platform credentials and
local exact secret-read approvals; implement within the boundaries below. This supports the already accepted Narrator/AI
path; it does not authorize browser/guest secret writes.

## Grounded gap

Engine has `SecretManager::system().store/delete`, manifest `secret-read` requests,
`grant_requested_secret_read_for_instance` and guest `secrets.read`. Native CLI
currently exposes none of these configuration operations, and Host composition
persists runtime/HTTP grants but no secret grants. Consequently an ordinary CLI
user cannot make the current provider authenticate after Host restart.

## Accepted amendment

- Native `secret-set <path>` reads bounded UTF-8 secret bytes from stdin only,
  without command-line value, environment value, printing or logging the value.
  Refuse a terminal stdin to prevent accidental echoed entry.
  Store through existing platform credential vault; no plaintext file fallback.
  Bound at 64 KiB (existing Host message scale); empty/oversize/invalid UTF-8 is an
  explicit error. Preserve bytes, including whitespace; examples use printf.
- Native `secret-delete <path>` uses the same existing vault, no read-back command.
- Native `grant-secret-read <instance> <component> <exact-path> --scope <scope>`
  and `revoke-secret-read` edit only the selected local composition under its
  existing file lock/atomic writer. No wildcard CLI grants in this slice. Validate
  exact canonical SecretPath and a live installed instance/component's declared
  manifest request covers that path. Grant may precede storing the value.
- Add optional `secret_read_grants` metadata rows (scope, instance, component,
  exact path) to Host composition; default empty, omit empty serialization. This
  extends the current local profile shape, without a new persistent service,
  per-package database or retired-profile migration. Bound rows at 256/profile;
  reject duplicate/foreign/missing component rows and undeclared permission.
- Apply exact persisted rows when bootstrapping the selected activation, after
  manifest validation and before start. World teardown stops and unregisters instances, using existing component
  secret-grant revocation. Bare Engine stop retains approved registration policy;
  it is not the World stop/unregister/re-register sequence. Edits take effect on next orderly activation/restart,
  like native CLI runtime grants; do not imply live permission reload.
- These local grants never enter RTW template descriptor, requested runtime grants,
  World SQLite or exported package settings. A copied World needs fresh approval.
  Secret values stay in the platform vault; `--home` is not a separate vault.

Provider derives `ai.world_<WorldId without hyphens>.api_key` at runtime; the World
and instance IDs cannot be supplied by a remote query caller. Its static manifest
requests `ai.*`, while the operator approves only that World's one exact path.
A clone has new World/activation identity and therefore a different key binding.
It cannot reuse the source World's secret merely by copying its portable settings.

Missing vault service/locked keyring/read grant/value is an explicit actionable
failure. Never fall back to storing a key in the exported provider config or URL.
Guest and browser API permissions are unchanged; no WIT secret-write import.

## Required acceptance

Test stdin bounds/no-value diagnostics; manifest-restricted exact scoped grant
persistence and revocation; bootstrap/restart; copied Worlds get no grants or
source keys; export includes portable endpoint/model but no secrets/local grants.
Use an in-memory credential vault in deterministic automated tests. Do not modify
real user keys or transmit private cards while testing.


## Acceptance checkpoint

Implemented within the accepted scope. Native Host profile/bootstrap tests cover
persisted exact approvals across new engines, wrong-World filtering, stop/unregister
revocation, absent/undeclared components, duplicate/quota rejection without writes,
fresh materializations without grants and explicit revoke. CLI tests/probes cover
exact stdin bytes and rejection of empty/oversize/non-UTF-8/terminal input without
value diagnostics or plaintext files. Existing Engine in-memory-vault tests cover
value storage, owner-scoped reads and deletion; no real credential value was touched.

The actual browser export/clean-home scenario installs a source World with a local
secret-read approval but no secret value. Both same-home fork and clean-home World
preserve AI settings, derive a fresh key binding and have no copied approvals. Raw
logs and hashes: `/home/mart/Desktop/rintawa-evidence/narrator-ai-2026-11-23/`.

## Shared-connection amendment for new exact AI RTWs

The generic native CLI/vault/grant behavior above is unchanged. New AI RTWs use
`ai.connection_<32 lowercase hex digits>.api_key`, derived from a backend-generated
shared connection ID, rather than a World-derived path. Store once per connection;
approve the exact path separately for each consuming World backend. The baseline
catalog does not contain keys and needs no read grant. Old exact RTWs keep their
original path and portable config. No automatic copying/deletion of stored keys.
See [the shared extension configuration contract](extension-settings-and-ai-connections.md).
