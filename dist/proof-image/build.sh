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

docker buildx build \
    --load \
    --platform linux/amd64 \
    --provenance=false \
    --sbom=false \
    --tag "$tag" \
    "$context" # env-policy: allow

identity=$(docker image inspect --format '{{.Id}}' "$tag") # env-policy: allow
expected=$(tr -d '\n' < "$root/dist/proof-image/identity.txt") # env-policy: allow
if [ "$identity" != "$expected" ]; then # env-policy: allow
    printf 'proof image identity drift: expected %s, built %s\n' "$expected" "$identity" >&2 # env-policy: allow
    exit 65
fi

docker run --rm --network=none --platform linux/amd64 "$identity" \
    sh -ceu 'test "$(uname -m)" = x86_64; test "$(rustc --version)" = "rustc 1.97.1 (8bab26f4f 2026-07-14)"; test "$(trunk --version)" = "trunk 0.21.14"; test "$(cargo-mutants mutants --version)" = "cargo-mutants 27.1.0"; test "$(wasm-opt --version)" = "wasm-opt version 123 (version_123)"' # env-policy: allow

docker save --output "$archive" "$identity" # env-policy: allow
checksum=$(shasum -a 256 "$archive" | awk '{print $1}') # env-policy: allow
printf '%s\n' "$checksum" > "$archive.sha256" # env-policy: allow
printf '%s\n' "$identity" > "$archive.identity" # env-policy: allow

printf 'proof image %s\narchive %s\n' "$identity" "$archive" # env-policy: allow
