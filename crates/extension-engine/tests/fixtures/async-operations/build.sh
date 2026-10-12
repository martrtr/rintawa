#!/usr/bin/env bash
set -euo pipefail
fixture_root="$(cd "$(dirname "$0")" && pwd)"
core_root="$(cd "$fixture_root/../../../../.." && pwd)"
workspace="$core_root/target/async-operations-fixtures"
mkdir -p "$workspace/producer/src" "$workspace/consumer/src" "$workspace/producer/wit" "$workspace/consumer/wit" "$workspace/rtw"
cp "$fixture_root/provider.rs" "$workspace/producer/src/lib.rs"
cp "$fixture_root/consumer.rs" "$workspace/consumer/src/lib.rs"
for guest in producer consumer; do
  cp "$core_root/crates/extension-engine/wit/engine.wit" "$workspace/$guest/wit/engine.wit"
done

cat > "$workspace/Cargo.toml" <<'TOML'
[workspace]
members = ["producer", "consumer"]
resolver = "2"
TOML
for guest in producer consumer; do
  cat > "$workspace/$guest/Cargo.toml" <<TOML
[package]
name = "rintawa-async-fixture-$guest"
version = "0.0.1"
edition = "2024"
publish = false
[lib]
crate-type = ["cdylib"]
[dependencies]
wit-bindgen = "=0.57.1"
TOML
done

cat > "$workspace/rintawa-dev.toml" <<'TOML'
schema = 1
artifact-root = "rtw"
[[rust-components]]
manifest-path = "producer/Cargo.toml"
artifact = "rintawa_async_fixture_producer"
output = "producer.wasm"
[[rust-components]]
manifest-path = "consumer/Cargo.toml"
artifact = "rintawa_async_fixture_consumer"
output = "consumer.wasm"
TOML
cat > "$workspace/rtw/rtw.toml" <<'TOML'
format = 1
content = "rintawa.extension@1"
entry = "manifest.toml"
TOML
cat > "$workspace/rtw/manifest.toml" <<'TOML'
id = "fixture.async-operations"
name = "Async operation integration fixture"
version = "0.0.1"
sdk = "^0.0"
TOML
if test -f "$fixture_root/Cargo.lock"; then
  cp "$fixture_root/Cargo.lock" "$workspace/Cargo.lock"
else
  (cd "$workspace" && cargo generate-lockfile --offline)
  cp "$workspace/Cargo.lock" "$fixture_root/Cargo.lock"
fi
# Nix may install wasm-ld without putting it on PATH.
if [[ -z "$(printenv CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER || true)" ]]; then
  linker="$(command -v wasm-ld 2>/dev/null || true)"
  if [[ -z "$linker" && -d /nix/store ]]; then
    linker="$(find /nix/store -maxdepth 3 -type f -path '*/bin/wasm-ld' -print -quit 2>/dev/null || true)"
  fi
  if [[ -n "$linker" ]]; then
    export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER="$linker"
  fi
fi
export CARGO_INCREMENTAL=0
dev_cli="$core_root/target/debug/rintawa-dev"
if [[ ! -x "$dev_cli" ]]; then
  (cd "$core_root" && cargo build --locked -p rintawa-dev)
fi
"$dev_cli" check "$workspace"
cp "$workspace/rtw/producer.wasm" "$fixture_root/producer.wasm"
cp "$workspace/rtw/consumer.wasm" "$fixture_root/consumer.wasm"
echo 'Built real WASM provider and consumer against current WIT.'
