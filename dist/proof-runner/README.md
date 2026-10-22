# Sealed Darwin proof runner

`build.sh` seals the `aarch64-apple-darwin` half of release proof: the `proof`
crate's `spice-proof-darwin-runner`, compiled with one pin document naming the
source commit, the vendored `Cargo.lock` closure, the SDK, developer, and Cargo
cache directories, and the path and bytes of each boundary tool (Cargo, rustc,
cargo-mutants, Git, and `sandbox-exec`). Run it from the repository root:

```sh
dist/proof-runner/build.sh
```

It requires native arm64 macOS, a tracked tree at `HEAD`, and no non-ignored
untracked files outside release output. It writes the runner, its vendored
closure, and a readable copy of the pins under `target/proof-native-darwin/`,
then exercises the runner protocol end to end and reports the runner's
identity.

Bind that path as `proof.runner` in the worktree that sealed it:
`spice config proof.runner PATH` plans the write and
`spice config proof.runner PATH --apply=DIGEST` applies it to that worktree's
Git-directory `.spice/worktree.toml`, which wins over a Git-common default
in `.spice/config.toml`, so worktrees sealing runners from different commits
never repoint each other. Nothing else selects a runner:
`spice dev xtask -- stage-target aarch64-apple-darwin` refuses until the
configured runner attests, and proof replay admits it by digesting the exact
bytes it is about to execute.

Pins are compiled into the executable, so a runner built without them refuses
and editing them changes its identity. Before every execution the runner
re-reads its own bytes, revalidates every pin, emits its attestation on stderr,
and runs the battery under `sandbox-exec`: routes off the host denied (loopback
stays, because the battery's own corpus binds it), the repository read-only,
host Cargo caches and the library prefixes `-sys` build scripts probe
unreadable, neighbouring checkouts neither readable nor writable, and fresh
Cargo home, target, and mutation-output directories removed afterward. Cleanup
restores owner access only on real directories inside that private root, so
member-created read-only trees cannot leak the root and symlink targets are
never traversed.

The profile opens with `(allow default)`, so it denies named things and still
denies less than the container half, which has no host filesystem at all: it
establishes that the prefixes host dependencies are found under are
unreachable, not that the host is absent. The rationale for each rule lives
with the profile in
[`proof/src/bin/spice-proof-darwin-runner/main.rs`](../../proof/src/bin/spice-proof-darwin-runner/main.rs).
