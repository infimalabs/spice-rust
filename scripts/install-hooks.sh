#!/usr/bin/env sh

set -eu

root=$(git rev-parse --show-toplevel)
cd "$root" # env-policy: allow

for hook in pre-commit commit-msg reference-transaction; do
    if [ ! -x ".githooks/$hook" ]; then # env-policy: allow
        printf 'hook installation failed: .githooks/%s is not executable\n' "$hook" >&2 # env-policy: allow
        exit 1
    fi
done

git_dir=$(git rev-parse --git-dir)
common_dir=$(git rev-parse --git-common-dir)
if [ "$git_dir" != "$common_dir" ]; then # env-policy: allow
    git config --local extensions.worktreeConfig true
fi

git config --worktree core.hooksPath .githooks

configured=$(git config --worktree --get core.hooksPath)
if [ "$configured" != ".githooks" ]; then # env-policy: allow
    printf 'hook installation failed: expected .githooks, found %s\n' "$configured" >&2 # env-policy: allow
    exit 1
fi

printf 'installed tracked hooks from %s/.githooks\n' "$root" # env-policy: allow
