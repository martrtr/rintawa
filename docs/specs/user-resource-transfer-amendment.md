# Activation-owned user-resource handoff

Status: accepted by the owner, 2026-11-22. Narrow addition discovered during
the accepted P10 Character import implementation and reviewed statically by Astra.

## Concrete requirement

An admitted import may wait behind another import while retaining only a completed
upload reference. Stopping or trapping the backend must return those resources'
byte/count quota while the Host remains alive. Current completed resources are
Host-lifetime bearers without activation ownership. `guest.stop` has no live Host
access, and quarantine can dispose of a Store without invoking guest cleanup.
Opening general imports during stop cannot satisfy trap cleanup safely.

## Proposed optional acquisition

Add to existing `user-resources`, reusing its records/error variant:

```wit
take-resource: func(reference: resource-ref) -> result<resource-ref, error>;
```

An active ordinary component/delegated task with effective `user-resource-read`
may consume one exact, unclaimed, completed reference. Under the existing broker's
synchronization, invalidate that original reference and return a fresh reference
to the same bytes/metadata, owned by the exact Host-attested consuming activation.
No file copy, quota increase, queue, database, permission or numeric handle type.
The operation fails without modifying the resource if input, live identity,
permission, ownership or fresh reference allocation cannot be validated.

Only that activation, with its current read grant, may read/release the returned
reference. A second take, including by the same activation, fails. Unclaimed
references keep existing transferable bearer/Host-lifetime behavior. Transfer is
optional and terminal; there is no onward transfer operation in this amendment.
The returned reference stays private to the backend. A delayed discard using the
old UI reference cannot destroy a successfully admitted import after reply loss.

No acquisition during registration, starting, System evaluation or teardown.
Inactive identity returns access-not-active; denied grant returns permission-denied;
unknown retired references return not-found; mismatched/claimed references fail
with invalid-resource/rejected according to the existing boundary's mapping.
Temporary internal inability to perform the transition returns unavailable.

## Native lifetime and race guarantee

Retirement of that exact activation disposes of its acquired entries, independent
of guest cooperation and current grants: stop, rollback, quarantine/trap, drop and
effective read-permission revocation. Dependent activations in a provider Store
remain distinct. Stale-generation cleanup must preserve replacement activations
and other Worlds' entries. Untaken resources remain Host-owned.

Acquisition and retirement are ordered: a racing callback cannot install a claim
after cleanup has completed. Reuse existing activation retirement and cleanup
boundaries. Do not match only a reusable component name or expose privileged guest
execution at stop. Metadata work is bounded by the existing 32-resource broker;
failed/busy cleanup retains its obligation for retry before reporting completion.

Accepted World commands retain D2 behavior: source retirement does not cancel or
undo them. Input/assets already serialized into the writer remain independent.

## Consumer and acceptance

Character checks attested own UI, selected authority, bounds and a free operation
slot before taking the resource in its ordinary polled task. It stores only the
returned ref and replies pending after successful acquisition. Process one file
at a time, release its resource after reading, and release on preparation error.
Pending operation release still retains work/slot through completion. A lost begin
reply is unknown, never an automatic retry or blind discard of acquired state.

Verify actual upload → backend pending → World/component stop before queued read
→ byte/count quota recovery in the same Host, including released-pending inputs.
Also test task trap without guest.stop, rollback, effective permission revocation,
two Worlds, stale activation cleanup, false/foreign/duplicate refs, delayed UI
discard, the acquire/revoke race, bounded cleanup retry and already-admitted writes.
No fresh read/upload/World work is available in cleanup. Broker tests alone do not
establish the product path.

## Alternatives and compatibility

Cooperative stop-only release is smaller but cannot reclaim resources after trap
or grant revocation and requires new destructive authority during teardown.
Host-lifetime retention needs no code but relaxes P10 cleanup and can strand the
entire byte budget after two 16 MiB accepted inputs. A same-ref claim requires
every consumer's discard endpoint to guard accepted refs against delayed requests.
The replacement reference keeps that shared guarantee at the broker boundary.

No browser bridge endpoint is added. Core and rtwKit authoring WIT must agree on
the accepted addition; build/release against the matching pinned Core revision.
World persistence and existing untaken resource semantics do not change.
