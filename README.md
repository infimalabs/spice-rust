# Spice Rust

## Development hooks

Install the tracked hooks once per checkout or linked worktree:

```sh
./scripts/install-hooks.sh
```

During coexistence the active pre-commit hook remains Python-owned and invokes
`spice dev pre-commit`. The canonical Rust verifier runs that installed policy
gate after formatting, Clippy, native and browser-target builds, deterministic
app verification, workspace tests, doctests, and the machine-readable
succession-readiness check. It prints each command before execution and names
the first command and exit status when verification fails.

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
