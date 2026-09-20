# WASM service-routing fixtures

`provider.wasm` and `consumer.wasm` are Component Model fixtures for the
cross-extension unary service path. Their auditable Rust sources are
`provider.rs` and `consumer.rs`.

Both fixtures are generated against
`crates/extension-engine/wit/engine.wit` with `wit-bindgen 0.57.1`, compiled
for `wasm32-unknown-unknown`, and wrapped with `wit-component 0.247.0`.

The provider declares and provides the versioned service contract
`example.echo@1` and returns `pong` for the `ping` request. The consumer
declares the same contract as a required consumer and calls it from its normal
`start` callback. The integration test therefore exercises the complete
guest-import path:

```text
WASM consumer
  -> rintawa:engine/services.call
  -> Engine ServiceRuntime
  -> WASM provider handle-service
  -> response back to consumer
```

The checked-in components must be regenerated whenever the plugin WIT ABI
changes. They intentionally use only generic registration/service contracts;
they must not gain Chat-, AI-provider-, Web-, filesystem-, or product-specific
host APIs.
