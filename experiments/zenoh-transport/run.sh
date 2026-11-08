#!/usr/bin/env bash
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-"$here/target"}"

cargo build --locked   --manifest-path "$here/guest/Cargo.toml"   --target wasm32-unknown-unknown

export RINTAWA_ZENOH_PROBE_GUEST="$CARGO_TARGET_DIR/wasm32-unknown-unknown/debug/rintawa_zenoh_guest.wasm"

command -v bwrap >/dev/null || {
    printf 'Bubblewrap (bwrap) is required by the managed native test.\n' >&2
    exit 1
}
cargo build --locked --manifest-path "$here/Cargo.toml" --bins
export RINTAWA_ZENOH_PROBE_NATIVE_PEER="$CARGO_TARGET_DIR/debug/native_peer"
export RINTAWA_ZENOH_PROBE_DIRECT_PEER="$CARGO_TARGET_DIR/debug/direct_peer"
cargo test --locked --manifest-path "$here/Cargo.toml" -- --nocapture
