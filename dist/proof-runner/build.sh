#!/bin/sh
set -eu

root=$(git rev-parse --show-toplevel) # env-policy: allow
cd "$root" # env-policy: allow

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    printf 'the native proof runner is sealed on native arm64 macOS only\n' >&2
    exit 65
fi
if ! git diff-index --quiet HEAD --; then
    printf 'the native proof runner is sealed from tracked source at HEAD\n' >&2
    exit 65
fi
untracked=$(git ls-files --others --exclude-standard | sed \
    -e '/^dist\/stage\/objects\//d' \
    -e '/^dist\/stage\/targets\//d')
if [ -n "$untracked" ]; then # env-policy: allow
    printf 'untracked source is outside release output:\n%s\n' "$untracked" >&2 # env-policy: allow
    exit 65
fi

bundle="$root/target/proof-native-darwin" # env-policy: allow
vendor="$bundle/vendor" # env-policy: allow
rm -rf "$bundle" # env-policy: allow
mkdir -p "$bundle/bin" # env-policy: allow
cargo vendor --locked "$vendor" # env-policy: allow

# The unsealed build of the runner is the one tool that computes the identities
# the sealed build is pinned with, so the pins never come from a second
# implementation that could disagree with the one enforcing them.
unsealed() {
    cargo run --locked -q -p proof --bin spice-proof-darwin-runner -- "$@" # env-policy: allow
}

cargo_path=$(rustup which cargo) # env-policy: allow
rustc_path=$(rustup which rustc) # env-policy: allow
mutants_path=$(command -v cargo-mutants) # env-policy: allow
git_path=$(command -v git) # env-policy: allow
sandbox_path=/usr/bin/sandbox-exec
pins=$(printf '%s\n' \
    "source_commit = \"$(git rev-parse --verify 'HEAD^{commit}')\"" \
    "vendor_digest = \"$(unsealed closure-digest "$vendor")\"" \
    "sdk_path = \"$(xcrun --show-sdk-path)\"" \
    "developer_dir = \"$(xcode-select -p)\"" \
    "cargo_home = \"$(unsealed cargo-home)\"" \
    '' \
    '[cargo]' \
    "path = \"$cargo_path\"" \
    "digest = \"$(unsealed tool-digest "$cargo_path")\"" \
    '' \
    '[rustc]' \
    "path = \"$rustc_path\"" \
    "digest = \"$(unsealed tool-digest "$rustc_path")\"" \
    '' \
    '[cargo_mutants]' \
    "path = \"$mutants_path\"" \
    "digest = \"$(unsealed tool-digest "$mutants_path")\"" \
    '' \
    '[git]' \
    "path = \"$git_path\"" \
    "digest = \"$(unsealed tool-digest "$git_path")\"" \
    '' \
    '[sandbox_exec]' \
    "path = \"$sandbox_path\"" \
    "digest = \"$(unsealed tool-digest "$sandbox_path")\"") # env-policy: allow

env SPICE_NATIVE_PINS="$pins" \
    cargo build --locked --release -p proof --bin spice-proof-darwin-runner # env-policy: allow

runner="$bundle/bin/spice-proof-darwin-runner" # env-policy: allow
install -m 555 "$root/target/release/spice-proof-darwin-runner" "$runner" # env-policy: allow
identity=$("$runner" runner-identity "$runner") # env-policy: allow
# The runner names its scratch root after the challenge, so this exercise mints
# a fresh one the way the host does. A fixed spelling collides with any seal
# running beside it, and one interrupted seal would leave a root behind that
# refuses every later seal on this machine for a reason that is not about it.
challenge=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n') # env-policy: allow
"$runner" spice-proof-native-runner-v1 "$identity" "$challenge" \
    cargo --version # env-policy: allow

printf '%s\n' "$pins" > "$bundle/pins.toml" # env-policy: allow
printf 'runner %s\nidentity %s\n' "$runner" "$identity" # env-policy: allow
