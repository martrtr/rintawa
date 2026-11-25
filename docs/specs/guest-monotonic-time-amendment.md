# Guest monotonic time for bounded protocol lifetimes

Status: accepted by the owner on 2026-11-22, including the OS-suspend clarification
below. Narrow amendment discovered while implementing P10. No UI-specific Core code.

## Concrete gap

The accepted browser contract requires a five-second unauthenticated connection
deadline, a two-minute single-use bootstrap lifetime and a thirty-minute idle
session lifetime. The current `rintawa:engine@0.0.2` WIT exports periodic tasks,
but no clock or elapsed time. The Core linker does not provide WASI clocks.
Its native `Instant` deadlines are private to transport/task implementations.

Counting periodic callbacks cannot enforce those real-time lifetimes: the pump
can pause or delay callbacks, and task scheduling skips rather than replays missed
intervals. Browser-submitted timestamps are untrusted. A protocol model can receive
`now` as private input, but the production WASM adapter needs an authoritative source.

## Minimal proposed contract

Add to the existing general `host` interface:

```wit
variant clock-error { access-not-active, unavailable }
monotonic-millis: func() -> result<u64, clock-error>;
```

The value is elapsed milliseconds from a private process-monotonic epoch. It never
decreases within one Host process, is shared across its Stores, and has no wall-clock
or persistent meaning. Equal consecutive values are valid. Restart invalidates all
protocol sessions/tokens; timestamps must never be persisted for later reuse.
The underlying platform clock may stop during OS sleep; TTL suspension during sleep
is explicitly allowed. A delayed pump while the OS is running does not stop this
clock. Check expiration before using a credential or processing an authenticated
request, not just during periodic cleanup. No cross-platform wall-time sleep
accounting is promised.

Availability matches `host.random-bytes`: start and ordinary/delegated callbacks,
including tasks, are allowed; registration, System evaluation and teardown are
denied. No network, guest re-entry, waiting or mutable Store acquisition. Conversion
overflow returns unavailable. Clock failure fails credential admission closed;
never fall back to a guest counter or browser time.

This supplies the time observation only. Extension code owns timeout policy,
session state and cleanup. No Core session service, scheduler or UI TTL is added.

## Alternatives and acceptance

Callback-count timers and client timestamps cannot meet the accepted lifetimes.
Adding WASI clock linking or a generic native bridge introduces a larger capability
boundary for this one requirement. Exposing only elapsed intervals on task callbacks
changes more task surfaces and still does not cover other active callbacks.

Check nondecreasing ordinary/delegated observations and time advancing across a
delayed pump; registration/System/stop denial; failed clock/conversion behavior;
regenerate WIT consumers. Browser tests must expire auth/bootstrap/session state
using this time even after a pump pause. Existing D8 admission deadlines stay owned
by Core and are not replaced by guest timestamps.
