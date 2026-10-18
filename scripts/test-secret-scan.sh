#!/usr/bin/env bash

set -euo pipefail

repository_root="$(git rev-parse --show-toplevel)"
cd "${repository_root}"

temporary_directory="$(mktemp -d)"
trap 'rm -rf "${temporary_directory}"' EXIT

printf '%s\n' 'ordinary configuration text' > "${temporary_directory}/clean.txt"
bash scripts/gitleaks.sh dir \
    --no-banner \
    --no-color \
    --redact=100 \
    "${temporary_directory}/clean.txt"

if command -v sha256sum >/dev/null 2>&1; then
    synthetic_secret="$(printf '%s' 'rintawa-gitleaks-self-test' | sha256sum | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    synthetic_secret="$(printf '%s' 'rintawa-gitleaks-self-test' | shasum -a 256 | awk '{print $1}')"
else
    echo "Gitleaks self-test requires sha256sum or shasum." >&2
    exit 1
fi

test_repository="${temporary_directory}/repository"
mkdir -p "${test_repository}"

# Git hooks inherit repository-local environment variables such as GIT_DIR and
# GIT_INDEX_FILE. Clear every local variable before operating on the nested
# self-test repository so the fixture cannot mutate the caller's index or HEAD.
mapfile -t git_local_env_vars < <(git rev-parse --local-env-vars)
with_clean_git_environment() (
    local variable
    for variable in "${git_local_env_vars[@]}"; do
        unset "${variable}"
    done
    "$@"
)
git_in_test_repository() {
    with_clean_git_environment git -C "${test_repository}" "$@"
}

git_in_test_repository init -q
printf '%s\n' 'ordinary committed configuration' > "${test_repository}/config.txt"
git_in_test_repository add config.txt
git_in_test_repository \
    -c user.name='Rintawa Security Test' \
    -c user.email='security-test@example.invalid' \
    commit -qm 'baseline'

printf 'api_key = "%s"\n' "${synthetic_secret}" \
    > "${test_repository}/provider.env"
git_in_test_repository add provider.env

set +e
with_clean_git_environment bash scripts/gitleaks.sh git \
    --no-banner \
    --no-color \
    --redact=100 \
    --report-format json \
    --report-path "${temporary_directory}/findings.json" \
    --staged \
    "${test_repository}" \
    >/dev/null 2>&1
scan_status=$?
set -e

if [[ "${scan_status}" -ne 1 ]] || [[ ! -s "${temporary_directory}/findings.json" ]]; then
    echo "Gitleaks self-test failed: synthetic secret was not detected." >&2
    exit 1
fi
