#!/usr/bin/env bash
set -euo pipefail
fixture_root="$(cd "$(dirname "$0")" && pwd)"
core_root="$(cd "$fixture_root/../../../../.." && pwd)"
workspace="$core_root/target/world-system-fixture"
mkdir -p "$workspace/src" "$workspace/wit" "$workspace/rtw"
cp "$fixture_root/guest.rs" "$workspace/src/lib.rs"
cp "$core_root/crates/extension-engine/wit/engine.wit" "$workspace/wit/engine.wit"
cp "$fixture_root/Cargo.toml" "$workspace/Cargo.toml"
cat > "$workspace/rintawa-dev.toml" <<'TOML'
schema = 1
artifact-root = "rtw"
[[rust-components]]
manifest-path = "Cargo.toml"
artifact = "rintawa_world_system_fixture"
output = "component.wasm"
TOML
cat > "$workspace/rtw/rtw.toml" <<'TOML'
format = 1
content = "rintawa.extension@1"
entry = "manifest.toml"
TOML
cat > "$workspace/rtw/manifest.toml" <<'TOML'
id = "fixture.world-system"
name = "WASM World System fixture"
version = "0.0.1"
sdk = "^0.0"
TOML
RUSTUP_TOOLCHAIN=stable "$core_root/target/debug/rintawa-dev" check "$workspace"
cp "$workspace/rtw/component.wasm" "$fixture_root/component.wasm"
