# Spice Rust

## Development hooks

Install the tracked hooks once per checkout or linked worktree:

```sh
./scripts/install-hooks.sh
```

The pre-commit hook runs formatting, Clippy, and the complete workspace test
suite through `scripts/verify.sh`. It prints each command before execution and
names the command and exit status when verification fails.

## Workspace verification

Run the same verification outside a commit, and before handing work to
another agent, with:

```sh
./scripts/verify.sh
```

The `gate` integration tests are executable closure checks over the workspace
and its documents of record.

Lattice conformance compares every local path dependency with the acyclic
crate graph compiled into `gate/tests/dependency_lattice.rs`. That test's
`EXPECTED` table is the source of truth: it fails when a workspace member is
added without an entry or when a manifest adds, removes, reverses, or cycles a
local dependency edge.

Doctrine conformance holds [DOCTRINE.md](DOCTRINE.md) to its compiled section,
kernel, and decision-record contract. It requires every invariant to be stated
rather than merely named, every decision to carry rationale and explicit
deletions, settled language throughout, and every relative record link to
resolve on disk. [TASTES.md](TASTES.md) and [LATER.md](LATER.md) are the
companion records that govern curated preferences and deferred surfaces.
