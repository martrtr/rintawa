# WASM transport fixtures

These are auditable Rust sources and real Wasmtime Component Model binaries.

The provider declares a shared queryable and creates an ephemeral Zenoh
subscription during `guest.start`. It handles queries through the optional
`transport-handler` export and can publish notifications and poll the
subscription in the callback. The consumer queries the providers using a
regular WIT import without implementing any provider export.

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
