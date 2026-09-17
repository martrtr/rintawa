# WASM target-provider fixture

`component.wasm` is the release-size Component Model fixture used by
`artifact_loader_test.rs` to exercise the generic execution-target WIT boundary.
`guest.rs` is its auditable source.

The fixture is generated against `crates/extension-engine/wit/engine.wit` with
`wit-bindgen 0.57.1`, compiled for `wasm32-unknown-unknown`, and wrapped with
`wit-component 0.247.0`. It intentionally publishes `test.wasm-target@1` from
its normal `start` callback and reads `payload.bin` only through the borrowed
`artifact-source` resource before returning a provider-local component handle.

The checked-in component must be regenerated whenever the provider WIT ABI
changes. It must not gain filesystem, CAS-path, network, or Web-specific APIs.
