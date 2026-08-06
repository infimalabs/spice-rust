#!/usr/bin/env sh

set -eu

common_dir=$(git rev-parse --path-format=absolute --git-common-dir)
hooks="$common_dir/.spice-install/git-hooks" # env-policy: allow
for hook in pre-commit commit-msg reference-transaction; do
    if [ ! -x "$hooks/$hook" ]; then # env-policy: allow
        printf 'hook installation failed: %s/%s is not executable\n' "$hooks" "$hook" >&2 # env-policy: allow
        exit 1
    fi
done

git worktree list --porcelain |
while IFS= read -r line; do
    case "$line" in # env-policy: allow
        "worktree "*)
            worktree=${line#worktree }
            if prior=$(git -C "$worktree" config --worktree --get-all core.hooksPath); then # env-policy: allow
                git -C "$worktree" config --worktree --unset-all core.hooksPath # env-policy: allow
                printf 'cleared worktree hook override at %s (was %s)\n' "$worktree" "$prior" # env-policy: allow
            fi
            ;;
    esac
done

git config --local core.hooksPath "$hooks" # env-policy: allow

configured=$(git config --local --get core.hooksPath)
if [ "$configured" != "$hooks" ]; then # env-policy: allow
    printf 'hook installation failed: expected %s, found %s\n' "$hooks" "$configured" >&2 # env-policy: allow
    exit 1
fi

printf 'activated Spice hooks from %s\n' "$hooks" # env-policy: allow
