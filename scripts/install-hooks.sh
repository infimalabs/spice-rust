#!/usr/bin/env sh

set -eu

root=$(git rev-parse --show-toplevel)
cd "$root" # env-policy: allow

hooks=.spice/git-hooks
for hook in pre-commit commit-msg reference-transaction; do
    if [ ! -x "$hooks/$hook" ]; then # env-policy: allow
        printf 'hook installation failed: %s/%s is not executable\n' "$hooks" "$hook" >&2 # env-policy: allow
        exit 1
    fi
done

git_dir=$(git rev-parse --git-dir)
common_dir=$(git rev-parse --git-common-dir)
if [ "$git_dir" != "$common_dir" ]; then # env-policy: allow
    git config --local extensions.worktreeConfig true
fi

git config --worktree core.hooksPath "$hooks" # env-policy: allow

configured=$(git config --worktree --get core.hooksPath)
if [ "$configured" != "$hooks" ]; then # env-policy: allow
    printf 'hook installation failed: expected %s, found %s\n' "$hooks" "$configured" >&2 # env-policy: allow
    exit 1
fi

printf 'activated Spice hooks from %s/%s\n' "$root" "$hooks" # env-policy: allow
