# WASM service-routing fixtures

`provider.wasm` and `consumer.wasm` are Component Model fixtures for the
cross-extension unary service path. Their auditable Rust sources are
`provider.rs` and `consumer.rs`.

Both fixtures are generated against
`crates/extension-engine/wit/engine.wit` with `wit-bindgen 0.57.1`, compiled
for `wasm32-unknown-unknown`, and wrapped with `wit-component 0.247.0`.

The provider declares the feature-neutral service contracts
`example.echo@1` and `example.secure-echo@1`. The ordinary echo route returns
`pong` for `ping`; the consumer requires it and calls it from its normal
`start` callback. The secure route additionally requires the exact generic
secret grant `service.credentials.fixture` and reads that secret only from its
service callback.

The fixtures therefore cover both the ordinary cross-extension route and the
combined service/principal/secret execution path:

```text
WASM consumer
  -> rintawa:engine/services.call
  -> Engine ServiceRuntime
  -> WASM provider handle-service
  -> optional component-scoped secrets.read
  -> response back to consumer
```

The checked-in components must be regenerated whenever the plugin WIT ABI
changes. They intentionally use only generic registration/service contracts;
they must not gain Chat-, AI-provider-, Web-, filesystem-, or product-specific
host APIs.

## Multiple-provider fixture

`multiple-provider.wasm` and `multiple-consumer.wasm` exercise the targeted
`Multiple` service ABI added to `rintawa:engine/services`. Their auditable Rust
sources are `multiple_provider.rs` and `multiple_consumer.rs`.

The integration test starts two independent instances of the provider component.
The consumer declares `example.multiple-echo@1`, enumerates the two providers with
`list-providers`, then invokes each opaque handle through `call-provider`. The
consumer traps during `start` if enumeration or either targeted call is wrong.

These binaries were built against the current `engine.wit` with `wit-bindgen
0.57.1` and componentized through the repository `rintawa-dev` Rust-component
pipeline. As with the unary fixtures, regenerate them whenever the plugin WIT ABI
changes.
