#!/usr/bin/env bash
# Bootstrap and run the self-contained Rintawa Linux pre-alpha bundle.
set -euo pipefail

bundle_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
binary="$bundle_root/bin/rintawa"
packages="$bundle_root/packages"
marker_name=".rintawa-pre-alpha-initialized"
once=false
home="${RINTAWA_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/rintawa-pre-alpha}"

usage() {
    cat <<'EOF'
Usage: ./start.sh [--home PATH] [--once]

Starts the pre-alpha bundle.  Its default data directory is
${XDG_DATA_HOME:-$HOME/.local/share}/rintawa-pre-alpha.

--home PATH  use a new, empty Rintawa data directory instead.
--once       validate startup and exit without serving the interface.
EOF
}

while (($#)); do
    case "$1" in
        --home)
            (($# >= 2)) || { echo "--home requires a path" >&2; exit 2; }
            home="$2"
            shift 2
            ;;
        --once)
            once=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

[[ -x "$binary" ]] || { echo "missing executable: $binary" >&2; exit 1; }
[[ -d "$packages" ]] || { echo "missing package directory: $packages" >&2; exit 1; }
[[ ! -L "$home" ]] || { echo "refusing symlinked home: $home" >&2; exit 1; }

if [[ ! -e "$home/$marker_name" ]]; then
    if [[ -e "$home" ]] && find "$home" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
        echo "Refusing to initialize nonempty home: $home" >&2
        echo "Choose an empty --home, or use the same pre-alpha home after a completed startup." >&2
        exit 1
    fi
    mkdir -p "$home"

    install_baseline() {
        local package="$1"
        shift
        local output instance component permission
        output="$($binary --home "$home" install "$packages/$package.rtw")"
        instance="$(awk -F ' = ' '/^instance = / { print $2; exit }' <<<"$output")"
        [[ -n "$instance" ]] || { echo "could not determine installed instance for $package" >&2; exit 1; }
        while (($#)); do
            component="$1"
            permission="$2"
            shift 2
            "$binary" --home "$home" grant-runtime "$instance" "$component" "$permission" >/dev/null
        done
    }

    echo "Initializing Rintawa pre-alpha home: $home"
    install_baseline web-runtime runtime background-task runtime loopback-listen
    install_baseline web-ui shell background-task shell world-session-read shell world-session-write shell runtime-policy-write shell secret-manage home background-task home world-session-read
    install_baseline settings-ui settings background-task appearance background-task
    install_baseline ai-provider backend background-task settings background-task settings secret-prompt
    install_baseline dice dice background-task
    "$binary" --home "$home" package-install "$packages/TextRoleplay.rtw" >/dev/null
    : > "$home/$marker_name"
    echo "Installed the Text Roleplay template. Create a world from it in the browser."
fi

arguments=(--home "$home" run)
if "$once"; then
    arguments+=(--once)
fi

exec "$binary" "${arguments[@]}"
