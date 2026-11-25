# WASM transport fixtures

These are auditable Rust sources and real Wasmtime Component Model binaries.

The provider declares a shared queryable and creates an ephemeral Zenoh
subscription during `guest.start`. It handles queries through the optional
`transport-handler` export and can publish notifications and poll the
subscription in the callback. The consumer initiates queries from a callback after activation completes;
queries and subscriptions are rejected during its start callback. All fixtures
use `rintawa:engine@0.0.2`. Query callbacks receive the Host-attested immediate
caller separately from payload bytes.

The integration test verifies fan-out from two WASM providers, same-scope
WASM/native publish-subscribe delivery, World isolation, invalid names,
unauthorized handles, stop/unregister cleanup and restart. Register/stop
cannot publish; a failed start cannot leak a notification claiming success.

Notifications use Zenoh's bounded ring (16 samples per subscription), so
slow consumers may lose older samples. The payload has no replay contract.
The consumer must re-read authoritative World state for reconciliation.

The provider fixture also exercises an infinite WASM loop: transport callbacks
use the short callback fuel budget. A trapped Wasmtime Component Model instance
cannot be re-entered; Host discards the damaged instance, then an explicit
stop/unregister/reload restores the extension without restarting other scopes.

Run `bash build.sh` to rebuild `provider.wasm` and `consumer.wasm`
against the current `engine.wit` using wit-bindgen 0.57.1 and rintawa-dev.

D6 probes additionally check same-name instances, spoofed application claims,
nested caller identity, stale activations/handles and generic selected-provider
lookup, including the existing shell contract. The provider can forward a query
for the Host's restricted domain-owner acceptance probe; this is fixture behavior,
not a Core domain protocol.
