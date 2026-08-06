#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd -P) # env-policy: allow
git_common=$( # env-policy: allow
    /usr/bin/git -C "$root" rev-parse --path-format=absolute --git-common-dir # env-policy: allow
)
git_dir=$(/usr/bin/git -C "$root" rev-parse --path-format=absolute --git-dir) # env-policy: allow

case "$git_dir" in # env-policy: allow
    "$git_common"|"$git_common"/*) ;; # env-policy: allow
    *)
        printf 'worktree Git directory %s is outside common directory %s\n' \
            "$git_dir" "$git_common" >&2 # env-policy: allow
        exit 65
        ;;
esac

image=$(tr -d '\n' < "$root/dist/proof-image/identity.txt") # env-policy: allow
lock="$git_common/.spice-stage-app.lock" # env-policy: allow
if ! mkdir "$lock"; then # env-policy: allow
    printf 'stage-app is already running; common repository lock exists: %s\n' \
        "$lock" >&2 # env-policy: allow
    exit 75
fi

release_lock() {
    if [ -d "$lock" ]; then # env-policy: allow
        rmdir "$lock" # env-policy: allow
    fi
}
trap release_lock EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

docker run --rm --platform linux/amd64 --network none \
    --mount "type=bind,source=$root,target=/workspace" \
    --mount "type=bind,source=$git_common,target=$git_common,readonly" \
    --mount "type=volume,source=spice-stage-target,target=/cargo-target" \
    --env CARGO_HOME=/usr/local/cargo \
    --env CARGO_TARGET_DIR=/cargo-target \
    "$image" cargo xtask stage-app # env-policy: allow

trap - EXIT HUP INT TERM
release_lock
