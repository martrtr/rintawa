# rintawa-dev

Development CLI for Rintawa extensions.

```bash
cargo run -p rintawa-dev -- check path/to/extension
```

Projects that require a build step can add `rintawa-dev.toml`:

```toml
schema = 1
artifact-root = "build/rtw"
build = ["npm", "run", "build"]
```

`check` builds the project, creates a temporary RTW artifact, and validates it through the artifact store.

Run the same snapshot through the Extension Engine:

```bash
cargo run -p rintawa-dev -- run path/to/extension
```

Use `--once` to start and stop immediately for lifecycle checks. Web bundle components expose loopback URLs while running.

Watch and reload after stable source changes:

```bash
cargo run -p rintawa-dev -- watch path/to/extension
```

`watch-ignore` uses gitignore-style patterns:

```toml
watch-ignore = ["*.tmp", "**/build/", "sources/**/node_modules/"]
```

`.git`, `target`, and `node_modules` are ignored by default. A separate build
`artifact-root` is ignored automatically while a build command is configured.

## Rust WebAssembly Components

`rintawa-dev` can build Rust guest crates directly into Component Model binaries without
requiring a separate `cargo-component` or `wasm-tools` CLI. The guest crate embeds the
Rintawa WIT world with `wit-bindgen`; `rintawa-dev` compiles it to
`wasm32-unknown-unknown` and wraps that core module through `wit-component`.

```toml
schema = 1
artifact-root = "rtw"

[[rust-components]]
manifest-path = "runtime/Cargo.toml"
artifact = "example_runtime"
output = "runtime.wasm"
```

`output` is relative to `artifact-root` and is treated as generated content by watch
fingerprinting. Rust component builds are locked and use `.rintawa-dev/target`, which is
ignored by watch mode. The development environment therefore needs the
`wasm32-unknown-unknown` Rust target and an LLD linker; the repository toolchain/flake
provide both.
