#!/usr/bin/env bash
set -euo pipefail

fixture_root="$(cd "$(dirname "$0")" && pwd)"
core_root="$(cd "$fixture_root/../../../../.." && pwd)"
workspace="$core_root/target/task-runtime-fixture"
mkdir -p "$workspace/src" "$workspace/wit" "$workspace/rtw"
cp "$fixture_root/guest.rs" "$workspace/src/lib.rs"
cp "$core_root/crates/extension-engine/wit/engine.wit" "$workspace/wit/engine.wit"

cat > "$workspace/Cargo.toml" <<'TOML'
[package]
name = "rintawa-task-runtime-fixture"
version = "0.0.1"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "=0.57.1"

[workspace]

[profile.dev]
debug = 0
strip = "symbols"
opt-level = "z"
panic = "abort"
lto = true
codegen-units = 1
TOML
cat > "$workspace/rintawa-dev.toml" <<'TOML'
schema = 1
artifact-root = "rtw"
[[rust-components]]
manifest-path = "Cargo.toml"
artifact = "rintawa_task_runtime_fixture"
output = "component.wasm"
TOML
cat > "$workspace/rtw/rtw.toml" <<'TOML'
format = 1
content = "rintawa.extension@1"
entry = "manifest.toml"
TOML
cat > "$workspace/rtw/manifest.toml" <<'TOML'
id = "fixture.task-runtime"
name = "WASM task runtime fixture"
version = "0.0.1"
sdk = "^0.0"
TOML
if [[ ! -f "$workspace/Cargo.lock" ]]; then
  (cd "$workspace" && RUSTUP_TOOLCHAIN=stable cargo generate-lockfile --offline)
fi

dev_cli="$core_root/target/debug/rintawa-dev"
if [[ ! -x "$dev_cli" ]]; then
  (cd "$core_root" && RUSTUP_TOOLCHAIN=stable cargo build --locked -p rintawa-dev)
fi
RUSTUP_TOOLCHAIN=stable "$dev_cli" check "$workspace"
cp "$workspace/rtw/component.wasm" "$fixture_root/component.wasm"
