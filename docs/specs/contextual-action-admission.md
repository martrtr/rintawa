# Directed queries for contextual actions

**Accepted by the owner, 2026-11-24.** Add one transport
import so a consumer can submit private context or effectful work to the same
live principal that answered discovery. Chat's message action is the motivating
case. Its descriptor, invocation schema, revision checks and provider-side
caller authorization belong to the extensions. [Experience proposal, section 2](extensible-experience.md#contextual-actions)

## Existing boundary

At Core `1e5b82e`, Host composition installs a component reference per scope
and point. `Transport::select_authoritative` excludes conflicting live
declarations. Both declaration styles enforce that policy. Separately,
`runtime-context.selected-providers` resolves contract ID/version for a
consumer. An extension may advertise another component's point in its payload.
The discovery reply source attests only the actual responder. [Host policy](../../crates/host/src/lib.rs) · [Transport](../../crates/extension-engine/src/transport.rs) · [WASM imports](../../crates/extension-engine/src/runtime/wasm.rs)

`Transport::begin_query` admits work without checking a recipient.
`run_work(Work::Begin)` releases the transport lock before Zenoh
`get(...).target(All)`. The private attachment carries caller and deadline.
Callback declarations capture their executing context. Polled declarations
queue a request in `admit_incoming` and release its payload to the guest in
`next_query`. `poll_query_sourced` checks sources after delivery. Looking up a
pin before an ordinary query, comparing reply sources afterwards, or checking
a claimed recipient inside guest code all leave a window for disclosure or
effects. [Nonblocking transport](../../crates/extension-engine/src/transport/polled.rs)

## Accepted contract

```wit
begin-query-to: func(point: string, payload: list<u8>, expected: principal) -> result<u64, error>;
```

1. Derive caller and scope from the same ordinary or hosted execution context
   as `begin-query`. Under the existing transport state lock, require the
   **exact** `authoritative[(caller.scope, point)] == expected.component()` and
   a live `ExecutionContext { scope: caller.scope, principal: expected }`.
   Use nonblocking validation on guest paths. Missing or different pin returns
   `not-selected`. A matching pin with a stale, absent or foreign activation
   returns `unavailable`. Preserve `access-not-active` for an inactive caller,
   `invalid-name` for an invalid point, `limit-exceeded` for size/capacity
   and `unavailable` for lock contention. Bound the combined UTF-8 ID bytes
   by the existing 4096-byte reply-source limit before retaining the target.
   Any error returned by begin means zero dispatch and no handle. A handle
   admits work even if publication of the matching declaration is pending.
2. Keep expected on the admitted request through the bounded D8 worker and
   carry it in Host-authored private query metadata. Under the state lock at
   dispatch, recheck pin, live target and availability of a matching published
   declaration. Failure here completes the handle with `unavailable`.
   The same 500 ms deadline covers the worker, registration and handoff.
   Ordinary senders omit expected and keep existing D8 semantics.
3. The **recipient handoff** decides whether guest code can see the payload.
   For callback declarations, compare the captured executing principal with
   expected, validate both live contexts, deadline and current point pin under
   the transport lock immediately before admitting the captured callback.
   Release the lock before running extension code. WASM callback entry retains
   its Store-level activation check in `begin_transport_query_execution`.
   For polled declarations apply the same conditions before queueing and at
   `next_query` checkout, under the lock shared with revocation and pin
   changes. A queue entry is Host-private and bound to its original
   declaration. On a failed checkout discard/reject the entry through the
   existing bounded disposal path, without returning its bytes. Never rebind
   an old request by looking up the new activation of the same component.
4. If stop or pin loss wins the handoff race, no provider code receives the
   request. If handoff wins, the original activation may already have acted.
   Stop/replacement and receiver close cannot roll back that work. Keep D8
   quotas, expiry, ownership, poll/close and terminal consumption. A receiver
   rejection with no successes yields `unavailable`. No reply by deadline may
   yield an empty list. Sourced polling still rejects missing or retired
   sources. A contextual action accepts exactly one sourced reply matching all
   three expected fields. Empty, ambiguous or failed completion after begin
   leaves the effect unknown. Never repeat the invocation automatically.
   [D8 lifecycle and limits](mvp-api-and-client-boundaries.md#d8-nonblocking-transport-for-hosted-and-ordinary-components)

An optional recipient in the existing attachment keeps one Zenoh request and
one set of handles. A separate activation-specific Zenoh key would also need
publication, teardown and a current pin check at handoff. Direct delivery
would duplicate the existing query/reply route. Mounting each provider's own
view remains appropriate for self-contained controls and would change the
headless action contract. No new point registry, grant or resolver is needed.

## Extension and browser use

An ordinary WASM consumer passes the principal from sourced discovery directly
to the new import. The provider declaring the action point must be the same
component and activation that answered discovery, even for two components in
one package. The principal is scoped to a live Host process. Discard discovered
actions when their session disconnects and rediscover after a Host restart.
Only installed composition supplies pins. The provider separately authorizes
the attested caller, selected message and revision before starting its effect.

In Kit (base `4b66f41`), mirror the WIT import and expose
`transport.begin-query-to` with strict arguments `{point,payload,expected}`.
Use the existing base64 bounds and `{instanceId,componentId,activation}` shape
with a canonical decimal u64 string on the wire. The only principal the
caller supplies is the recipient. `RootBridge.#callFrame` already forwards
transport RPCs on the bound frame channel. The web-runtime delegated task
issues the import under that channel's owner and tracks the result as
`Kind::Query`. Share the bridge's existing sourced-poll/close loop. Preserve
the bridge outcome convention: local rejection before WIT is `not-started`,
an error from the invoked WIT import carries its code with `unknown`, and a
lost acknowledgement is `unknown`. A native begin error still guarantees zero
dispatch. Existing Kit source
locations are `web/bridge/src/index.ts`,
`packages/web-runtime/src/{wire,wasm,session}.rs` and `wit/engine.wit`.

## Acceptance evidence required for implementation

- In `transport/polled/tests.rs`, run two discovery responders and count actual
  recipient entries. Cover absent pin, a responder claiming the other's
  pinned point, a stale activation under a matching pin, a different component
  of the same instance, and a pin in another World. Assert the exact begin
  error, zero entries, and full quota after repeated rejection.
- Gate the existing worker before dispatch, then replace the provider with
  the same component reference. Gate callback handoff after dispatch and
  clear its pin or replace the provider. For polled delivery, wait until the
  entry is queued, then clear the pin or stop/redeclare before checkout.
  Assert neither old nor replacement receives the bytes. With handoff
  allowed first, observe one effect, drop its reply, and observe no replay.
  Keep an identical point in a second World usable throughout.
- Extend the real `wasm_transport_test` consumer/provider fixtures for directed
  calls to callback and polled providers, including exact echoed caller and
  one sourced reply. Exercise the hosted consumer through a real sandboxed
  panel, Root, channel and web-runtime WASM with a harmless counted action.
  Assert wrong/stale targets produce zero effects, foreign-channel polling is
  denied, and losing the acknowledgement shows unknown without a second begin.
  No RP feature or external service is required.

The owner accepted this additive WIT operation and its exact point pin and
handoff semantics on 2026-11-24. Core and Kit implement the contract; the
implementation checks include Native and real-WASM directed delivery and a
real-browser contextual action fixture. These checks do not add a TTS provider.
