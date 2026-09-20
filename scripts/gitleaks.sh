#!/usr/bin/env bash

set -euo pipefail

readonly gitleaks_version="8.30.1"

case "$(uname -s):$(uname -m)" in
    Linux:x86_64|Linux:amd64)
        readonly release_platform="linux_x64"
        readonly release_sha256="551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"
        ;;
    Linux:aarch64|Linux:arm64)
        readonly release_platform="linux_arm64"
        readonly release_sha256="e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080"
        ;;
    Darwin:x86_64|Darwin:amd64)
        readonly release_platform="darwin_x64"
        readonly release_sha256="dfe101a4db2255fc85120ac7f3d25e4342c3c20cf749f2c20a18081af1952709"
        ;;
    Darwin:arm64|Darwin:aarch64)
        readonly release_platform="darwin_arm64"
        readonly release_sha256="b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5"
        ;;
    *)
        echo "unsupported platform for pinned Gitleaks: $(uname -s) $(uname -m)" >&2
        exit 1
        ;;
esac

if [[ -n "${GITLEAKS_CACHE_DIR:-}" ]]; then
    cache_root="${GITLEAKS_CACHE_DIR}"
elif [[ -n "${XDG_CACHE_HOME:-}" ]]; then
    cache_root="${XDG_CACHE_HOME}/rintawa-security"
elif [[ -n "${HOME:-}" ]]; then
    cache_root="${HOME}/.cache/rintawa-security"
else
    cache_root="${TMPDIR:-/tmp}/rintawa-security"
fi

readonly archive_name="gitleaks_${gitleaks_version}_${release_platform}.tar.gz"
readonly archive_dir="${cache_root}/gitleaks/${gitleaks_version}"
readonly archive_path="${archive_dir}/${archive_name}"
readonly release_url="https://github.com/gitleaks/gitleaks/releases/download/v${gitleaks_version}/${archive_name}"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
        return
    fi
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
        return
    fi

    echo "secret scan bootstrap requires sha256sum or shasum." >&2
    exit 1
}

archive_is_valid() {
    [[ -f "${archive_path}" ]] && [[ "$(sha256_file "${archive_path}")" == "${release_sha256}" ]]
}

mkdir -p "${archive_dir}"

if ! archive_is_valid; then
    rm -f "${archive_path}"
    temporary_archive="${archive_path}.tmp.$$"
    trap 'rm -f "${temporary_archive:-}"' EXIT

    curl --proto '=https' --tlsv1.2 -fsSL "${release_url}" -o "${temporary_archive}"
    actual_sha256="$(sha256_file "${temporary_archive}")"
    if [[ "${actual_sha256}" != "${release_sha256}" ]]; then
        echo "Gitleaks archive checksum mismatch for ${archive_name}." >&2
        echo "expected: ${release_sha256}" >&2
        echo "actual:   ${actual_sha256}" >&2
        exit 1
    fi
    mv "${temporary_archive}" "${archive_path}"
fi

temporary_directory="$(mktemp -d)"
trap 'rm -rf "${temporary_directory}"; rm -f "${temporary_archive:-}"' EXIT
tar -xzf "${archive_path}" -C "${temporary_directory}" gitleaks

"${temporary_directory}/gitleaks" "$@"
