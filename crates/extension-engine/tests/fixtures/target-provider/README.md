# WASM target-provider fixture

`component.wasm` is the compact Component Model fixture used by
`artifact_loader_test.rs` to exercise the generic execution-target WIT boundary.
`guest.rs` is its auditable source.

The fixture is generated against `crates/extension-engine/wit/engine.wit` with
`wit-bindgen 0.57.1`, compiled for `wasm32-unknown-unknown`, and wrapped with
`wasm-tools 1.258.0`. It intentionally publishes `test.wasm-target@1` from
its normal `start` callback and reads `payload.bin` only through the borrowed
`artifact-source` resource before returning a provider-local component handle.

The checked-in component must be regenerated whenever the provider WIT ABI
changes. Its task-capable authoring world also exercises D6: when granted,
provider/dependent tasks report their actual scope, World and activation through
Zenoh; foreign subscription/upload handles fail. The two-World integration probe
checks stop isolation and invalidation of shared clients after a hosted task trap.
Tasks require each dependent's own grants. The fixture has no filesystem or
CAS-path access, raw host resources, or Web-specific API.

Run `bash build.sh` to regenerate it.
