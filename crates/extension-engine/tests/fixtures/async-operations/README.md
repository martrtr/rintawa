# WASM async-operation integration fixtures

Two auditable Rust guest sources, provider.rs and consumer.rs, are compiled to
real Component Model binaries producer.wasm and consumer.wasm.

The consumer declares example.operation-stream@1 and calls the provider through
the normal service router. The provider issues a read-only lease bound to the
authenticated immediate service caller. The consumer verifies progress, ordered
byte chunks, pending, terminal completion, cancellation, explicit failure,
rejected owner-only operations and lease closure.

Integration tests load provider and consumer through separate WASM runtime
engine factories associated with the same ExtensionEngine, stop both components,
then start them again.

Regenerate after changing the WIT contract or fixture source:

    bash crates/extension-engine/tests/fixtures/async-operations/build.sh

The build script uses the repository rintawa-dev Component Model builder,
wit-bindgen 0.57.1, a committed Cargo.lock, and a generated workspace under the ignored Core target/ directory. It detects wasm-ld in the Nix store without modifying the system
toolchain. No active Rintawa profile or World is modified.

The fixture uses only generic service routing and ephemeral operation APIs.
No inference models, canonical World events, or durable effect jobs are involved.
Host unit tests cover cross-scope access, idle expiry and owner/reader stop.
