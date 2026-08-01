#!/usr/bin/env sh

set -eu

root=$(git rev-parse --show-toplevel)
cd "$root" # env-policy: allow

run() {
    label=$1
    shift
    printf '[verify] %s\n' "$label" # env-policy: allow
    if "$@"; then
        return 0
    else
        status=$?
        printf '[verify] failed: %s (exit %s)\n' "$label" "$status" >&2 # env-policy: allow
        return "$status" # env-policy: allow
    fi
}

run 'cargo fmt --all -- --check' cargo fmt --all -- --check
run 'cargo clippy --locked --workspace --all-targets -- -D warnings' \
    cargo clippy --locked --workspace --all-targets -- -D warnings
run 'cargo test --locked --workspace --all-targets' \
    cargo test --locked --workspace --all-targets
