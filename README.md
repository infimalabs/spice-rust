# Spice

## Install and start

Spice is this repository's installed appliance. Preview and apply its
digest-bound install plan, then activate the installed hooks once for the
repository:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
./scripts/install-hooks.sh
```

The plan publishes the executable, operator skill, and the `pre-commit`,
`commit-msg`, and `reference-transaction` hooks once under the Git common
directory's `.spice-install`. Every linked worktree uses those same assets.
Build this lane's Canvas app, then start Spice with:

```sh
cargo xtask build-app
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" up
```

That resident serves the HTML, JavaScript, stylesheet, and WASM built in this
worktree's ignored `dist/stage-lane`. It refuses to start when that four-file
stage is absent or incomplete. There is no separate command to open the app:
the start above announces the exact URL on its standard error, and adding
`--open` has the resident open the default browser too.

```
canvas endpoint: http://127.0.0.1:8765
```

That URL is announced only after the resident reads back the discovery file it
just published and admits it, so it has already passed the same ownership,
loopback-origin, and generation checks a client applies. `127.0.0.1:8765` is
the default; `--host` and `--port` bind a different address, and `--port 0`
takes any free port. The listener is IPv4 loopback unless `--host` names
another address of this machine, so by default the app is reachable only from
the host running it.

The one-time predecessor shutdown and empty-authority start are specified in
[CUTOVER.md](CUTOVER.md).

The activation script refuses unless all three hooks are executable. The
pre-commit hook runs the canonical verifier, which performs
formatting, Clippy, native and browser-target builds, deterministic app
verification, workspace tests, doctests, the machine-readable
contract-landing check. The checked gate manifest maps each of the 25 retired
standard policy checks to one compiled gate arm, and workspace tests exercise
the exact mapping and production executor. Verification prints each command
before execution and names the first command and exit status when it fails. The
native commit-msg hook preserves the current fixed message-hygiene law and
additionally requires every committed `Spice-Origin` line to use the exact v1
grammar. The native reference-transaction hook refuses a current-branch update
that would abandon commits already merged into its upstream.

## Workspace verification

Run the same verification outside a commit, and before handing work to
another agent, with:

```sh
./scripts/verify.sh
```

The script changes to the repository root and then executes
`cargo xtask verify`; it is a location-independent wrapper over that one
verifier, not a second suite.

The `gate` integration tests enforce repository contracts from stable product
inputs. Lattice conformance compares every local path dependency in
`Cargo.lock` with the acyclic graph compiled into
`gate/tests/dependency_lattice.rs`. That test's `EXPECTED` table is the source
of truth: it fails when a workspace member is added without an entry or when a
manifest adds, removes, reverses, or cycles a local dependency edge.

Doctrine conformance holds [DOCTRINE.md](DOCTRINE.md) to its compiled section,
kernel, and decision-record contract. It requires every invariant to be stated
rather than merely named, every decision to carry rationale and explicit
deletions, settled language throughout, and every relative record link to
resolve on disk. [TASTES.md](TASTES.md) and [LATER.md](LATER.md) are the
companion records that govern curated preferences and deferred surfaces.
