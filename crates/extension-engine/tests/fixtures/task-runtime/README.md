# WASM cooperative-task fixture

`component.wasm` is a compact Component Model fixture used to exercise the
owner-scoped cooperative task WIT boundary. `guest.rs` is its auditable source.

The fixture is generated against `crates/extension-engine/wit/engine.wit` with
`wit-bindgen 0.57.1`, compiled for `wasm32-unknown-unknown`, and wrapped with
`wit-component 0.247.0`. Its normal `start` callback schedules one 10 ms periodic
task. Its first callback cancels itself, proving that the host pump executes the
callback transactionally without retaining a background resource.

The checked-in component must be regenerated whenever the task/runtime WIT ABI
changes. It must not gain WASI threads, filesystem access, raw sockets, or any
Web-specific API.
