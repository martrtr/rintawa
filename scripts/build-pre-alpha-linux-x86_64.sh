#!/usr/bin/env bash
# Build a local, offline-installable Linux x86_64 pre-alpha archive from Core and rtwKit.
set -euo pipefail

core_root="$(git rev-parse --show-toplevel)"
kit_root="${1:?usage: $0 /absolute/path/to/rtwKit /absolute/output-directory}"
output_root="${2:?usage: $0 /absolute/path/to/rtwKit /absolute/output-directory}"
kit_root="$(cd "$kit_root" && pwd -P)"
output_root="$(mkdir -p "$output_root" && cd "$output_root" && pwd -P)"

[[ -f "$kit_root/Cargo.toml" && -d "$kit_root/packages" ]] || {
    echo "not an rtwKit checkout: $kit_root" >&2
    exit 2
}
case "$(uname -s):$(uname -m)" in
    Linux:x86_64) ;;
    *) echo "this build script must run on Linux x86_64" >&2; exit 2 ;;
esac

core_revision="$(git -C "$core_root" rev-parse HEAD)"
kit_revision="$(git -C "$kit_root" rev-parse HEAD)"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
name="rintawa-pre-alpha-linux-x86_64-$stamp"
staging="$(mktemp -d "$output_root/.${name}.XXXXXX")"
artifact_dir="$staging/artifacts"
bundle="$staging/$name"
archive="$output_root/$name.tar.gz"

cleanup() { rm -rf "$staging"; }
trap cleanup EXIT
[[ ! -e "$archive" ]] || { echo "refusing to replace $archive" >&2; exit 1; }

mkdir -p "$artifact_dir" "$bundle/bin" "$bundle/packages"

echo "Building Core release binary at $core_revision"
(
    cd "$core_root"
    cargo build --locked --release -p rintawa
)
install -m 0755 "$core_root/target/release/rintawa" "$bundle/bin/rintawa"

echo "Building rtwKit package artifacts at $kit_revision"
for package in web-runtime web-ui settings-ui dice chat character narrator ai-provider; do
    (
        cd "$kit_root"
        cargo run --locked -p rtwkit-registry-tool -- build-package \
            "$kit_root" "$package" "$artifact_dir" local/pre-alpha "pkg-$package-v0.1.0" "$kit_revision" >/dev/null
    )
done
python3 "$kit_root/scripts/build-roleplay-template.py" \
    --artifacts "$artifact_dir" --output "$artifact_dir/TextRoleplay.rtw" >/dev/null

for package in web-runtime web-ui settings-ui dice chat character narrator ai-provider TextRoleplay; do
    install -m 0644 "$artifact_dir/$package.rtw" "$bundle/packages/$package.rtw"
done
install -m 0755 "$core_root/scripts/pre-alpha-linux-start.sh" "$bundle/start.sh"
install -m 0644 "$kit_root/LICENSE" "$bundle/LICENSE"

glibc_floor="$(readelf --version-info "$bundle/bin/rintawa" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -n 1 || true)"
cat > "$bundle/README.txt" <<EOF
Rintawa pre-alpha for Linux x86_64
==================================

Build UTC: $stamp
Core revision: $core_revision
rtwKit revision: $kit_revision
Binary ABI: Linux x86_64, dynamically linked (${glibc_floor:-glibc version unavailable})

Secrets
-------
Use Settings → Secrets to manage local credentials, or Select API key in AI
Provider settings. The selected shell is explicitly granted secret-manage by
start.sh; requesting settings receive secret-prompt. Values are write-only.
The default backend is system credentials. To explicitly select local files:
  ./bin/rintawa --home PATH secret-backend local-file
Restart the Host after changing backends. CLI stdin commands remain available.

Quick start
-----------
1. Extract this archive.
2. Run ./start.sh
3. Open the local URL printed by Rintawa.
4. Select New World → Text Roleplay, approve its shown permissions, then import
   a Tavern V2/V3 character card.

The first start initializes a separate local data directory:
  ${XDG_DATA_HOME:-$HOME/.local/share}/rintawa-pre-alpha
It never writes to the regular Rintawa home. Pass --home PATH to choose a new,
empty directory. Use ./start.sh --once for a startup-only verification.

Included RTWs
-------------
Baseline: web-runtime, web-ui, settings-ui, ai-provider, dice.
Template: TextRoleplay embeds web-runtime, settings-ui, ai-provider, character,
chat and narrator. All eight RTW files are also available in packages/.

AI is not configured by this archive. In Home open … → Settings → Ai Provider,
add Ollama or an OpenAI-compatible server, and approve the explicit local origin.
Select API key opens the shell-owned picker, where you can create a credential or
reuse one and approve its use. Manage shared values and consumers in Settings →
Secrets. CLI stdin commands remain available; credentials are never bundled.
The default backend remains system credentials. local-file is an explicit choice,
not encrypted at rest, protected by OS permissions; switching never migrates keys.

This is an experimental pre-alpha build, not a signed release.
EOF
(
    cd "$bundle"
    find . -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
)

echo "Verifying bundle initialization in a clean temporary home"
verify_home="$staging/verify-home"
"$bundle/start.sh" --home "$verify_home" --once > "$staging/bootstrap.log" 2>&1
template_list="$($bundle/bin/rintawa --home "$verify_home" template-list)"
grep -F 'Text Roleplay' <<<"$template_list" >/dev/null
template_digest="$(awk '/ Text Roleplay$/ { print $1; exit }' <<<"$template_list")"
[[ -n "$template_digest" ]] || { echo "cannot find installed Text Roleplay template" >&2; exit 1; }
"$bundle/bin/rintawa" --home "$verify_home" world-create --template "$template_digest" --approve-runtime-permissions >/dev/null

tar --sort=name --mtime='UTC 2026-01-01' --owner=0 --group=0 --numeric-owner \
    -C "$staging" -czf "$archive" "$name"
sha256sum "$archive" > "$archive.sha256"
printf '%s\n' "$archive"
printf '%s\n' "$archive.sha256"
