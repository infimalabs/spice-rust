#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd) # env-policy: allow
archive=${1:?"usage: dist/proof-image/load.sh ARCHIVE"} # env-policy: allow
expected=$(tr -d '\n' < "$root/dist/proof-image/identity.txt") # env-policy: allow

expected_checksum=$(tr -d '\n' < "$archive.sha256") # env-policy: allow
observed_checksum=$(shasum -a 256 "$archive" | awk '{print $1}') # env-policy: allow
if [ "$observed_checksum" != "$expected_checksum" ]; then # env-policy: allow
    printf 'proof archive checksum drift: expected %s, observed %s\n' \
        "$expected_checksum" "$observed_checksum" >&2 # env-policy: allow
    exit 65
fi
archived=$(tr -d '\n' < "$archive.identity") # env-policy: allow
if [ "$archived" != "$expected" ]; then # env-policy: allow
    printf 'proof archive identity drift: expected %s, archive names %s\n' "$expected" "$archived" >&2 # env-policy: allow
    exit 65
fi

docker load --input "$archive" # env-policy: allow
loaded=$(docker image inspect --format '{{.Id}}' "$expected") # env-policy: allow
if [ "$loaded" != "$expected" ]; then # env-policy: allow
    printf 'loaded proof image drift: expected %s, loaded %s\n' "$expected" "$loaded" >&2 # env-policy: allow
    exit 65
fi

printf 'loaded proof image %s\n' "$loaded" # env-policy: allow
