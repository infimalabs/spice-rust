#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd) # env-policy: allow
state="$root/target/proof-image" # env-policy: allow
context="$state/context" # env-policy: allow
archive=${1:-"$state/spice-proof-image.tar"} # env-policy: allow
tag=spice-proof-image:build

rm -rf "$context" # env-policy: allow
mkdir -p "$context" "$(dirname "$archive")" # env-policy: allow
cargo vendor --locked "$context/vendor" # env-policy: allow
cp "$root/dist/proof-image/Dockerfile" "$context/Dockerfile" # env-policy: allow
cp "$root/dist/proof-image/cargo-config.toml" "$context/cargo-config.toml" # env-policy: allow
cp "$root/dist/proof-image/cargo-wrapper.sh" "$context/cargo-wrapper.sh" # env-policy: allow

log="$state/build.log" # env-policy: allow
status="$state/build.status" # env-policy: allow

# The status travels through a file because this is POSIX sh, where a pipeline
# reports only its last stage, and `|| code=$?` keeps `set -eu` from taking the
# stage before the status is written. Progress stays on stderr, where buildx
# puts it.
{ code=0; docker buildx build \
    --load \
    --platform linux/amd64 \
    --provenance=false \
    --sbom=false \
    --tag "$tag" \
    "$context" 2>&1 || code=$?; echo "$code" > "$status"; } | tee "$log" >&2 # env-policy: allow

code=$(cat "$status") # env-policy: allow
if [ "$code" -ne 0 ]; then # env-policy: allow
    # A build the runtime dropped and a build a compiler refused arrive with the
    # same exit status, and the runtime says only "unexpected EOF". On a host
    # where many lanes share one Docker daemon that phrase almost always means a
    # concurrent global prune rather than anything in this source, so name it
    # instead of leaving every lane to rediscover it.
    if grep -q 'unexpected EOF' "$log"; then # env-policy: allow
        printf 'the container runtime dropped this build; no compiler refused it\n' >&2
        printf 'on a shared daemon that is a concurrent global prune, which takes the\n' >&2
        printf 'build cache and the pinned proof image from every lane at once\n' >&2
        printf 'clean up only what this lane owns, its own %s, and rerun\n' "$state" >&2 # env-policy: allow
    fi
    exit "$code" # env-policy: allow
fi

identity=$(docker image inspect --format '{{.Id}}' "$tag") # env-policy: allow
expected=$(tr -d '\n' < "$root/dist/proof-image/identity.txt") # env-policy: allow
if [ "$identity" != "$expected" ]; then # env-policy: allow
    printf 'proof image identity drift: expected %s, built %s\n' "$expected" "$identity" >&2 # env-policy: allow
    exit 65
fi

docker run --rm --network=none --platform linux/amd64 "$identity" \
    sh -ceu 'test "$(uname -m)" = x86_64; test "$(rustc --version)" = "rustc 1.97.1 (8bab26f4f 2026-07-14)"; test "$(rustfmt --version)" = "rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)"; test "$(trunk --version)" = "trunk 0.21.14"; test "$(cargo-mutants mutants --version)" = "cargo-mutants 27.1.0"; test "$(wasm-opt --version)" = "wasm-opt version 123 (version_123)"' # env-policy: allow

docker save --output "$archive" "$identity" # env-policy: allow
checksum=$(shasum -a 256 "$archive" | awk '{print $1}') # env-policy: allow
printf '%s\n' "$checksum" > "$archive.sha256" # env-policy: allow
printf '%s\n' "$identity" > "$archive.identity" # env-policy: allow

printf 'proof image %s\narchive %s\n' "$identity" "$archive" # env-policy: allow
