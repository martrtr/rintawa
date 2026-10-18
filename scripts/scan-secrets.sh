#!/usr/bin/env bash

set -euo pipefail

repository_root="$(git rev-parse --show-toplevel)"
cd "${repository_root}"

if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
    echo "secret scan requires complete Git history; use checkout fetch-depth: 0." >&2
    exit 1
fi

gitleaks=(bash scripts/gitleaks.sh)

"${gitleaks[@]}" git \
    --no-banner \
    --no-color \
    --redact=100 \
    --log-opts="--all" \
    .

if ! git diff --quiet --; then
    "${gitleaks[@]}" git \
        --no-banner \
        --no-color \
        --redact=100 \
        --pre-commit \
        .
fi

if ! git diff --cached --quiet --; then
    "${gitleaks[@]}" git \
        --no-banner \
        --no-color \
        --redact=100 \
        --staged \
        .
fi
