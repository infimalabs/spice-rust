# Sealed Darwin proof runner

`build.sh` seals the `aarch64-apple-darwin` half of the release appliance: the
`proof` crate's `spice-proof-darwin-runner` binary, built with one compiled pin
document naming the exact source commit, the vendored closure of `Cargo.lock`,
the SDK, developer, and Cargo cache directories, and the path and bytes of
every tool it will run. Seal the runner from the repository root:

```sh
dist/proof-runner/build.sh
```

The command requires native arm64 macOS, a tracked tree at `HEAD`, and no
non-ignored untracked files outside release output. It writes the runner, its
vendored closure, and a readable copy of the pin document under
`target/proof-native-darwin/`, then exercises the runner protocol end to end
before reporting the runner's identity.

Bind that path as `proof.runner` in machine-local configuration. Nothing else
selects a runner: the exact Git-common Spice appliance's
`dev xtask -- stage-target aarch64-apple-darwin` refuses to
stage until the configured runner attests, and proof replay admits the runner by
digesting the exact bytes it is about to execute.

Pins live inside the executable rather than beside it, so a runner built without
them refuses instead of running unpinned, and editing them produces a different
executable with a different identity. The runner re-reads its own bytes before
every execution, revalidates every pin, then emits its attestation on stderr and
runs the battery member under `sandbox-exec` with every route off this host
denied, the repository read-only, the host's Cargo caches unreadable, the
package-manager prefixes and framework directory a `-sys` build script probes
for a library unreadable, every checkout beside this one neither readable nor
writable, and fresh empty Cargo home, target, and mutation-output directories
that it removes
afterward. Loopback stays reachable because the battery's own corpus binds it,
which is the boundary the container half gets from a network namespace holding
nothing but a loopback interface.

That profile opens with `(allow default)`, so it denies named things rather than
admitting named things, and it therefore still denies less than the container
half, which has no host filesystem to deny at all. What it establishes about
host dependencies is that the prefixes such a dependency is found under are
unreachable, not that the host is absent.

The neighbouring checkouts are the exception that proves the shape: the
directory the workspace sits in is denied for reading and writing alike, and
the parent node's metadata and three paths are then read back — the workspace,
the Git common directory the runner resolved, and the run's own scratch root.
The literal parent allowance lets Cargo and the compiler resolve those admitted
descendants without revealing the parent's entries or any peer's metadata or
bytes. Only reading is given back there. The one path the run may write is the
scratch root, named by the last rule in the profile, so a scratch root that
happens to sit in the neighbourhood survives the denial while the workspace and
its repository stay read-only, which they already were. Denying reads alone
would have left the sharper half open: a
run that cannot see a peer's source can still truncate it, and the receipt would
name neither. The common directory is read back because a linked worktree
keeps it beside the workspace rather than inside it, so denying the
neighbourhood would otherwise take the repository being proved. A seal whose
workspace parent holds a pinned path instead refuses, naming that pin: the
denial would take it with nothing to allow it back from. The rustup toolchain
root is not pinned by name and needs no pin of its own, because `build.sh` seals
the compiler `rustup which rustc` resolves and that binary lives inside the
root.
