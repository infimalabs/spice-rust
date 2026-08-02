# Spice Rust

## Development hooks

During coexistence, Git remains configured to the Python-owned `.spice/hooks`
directory or the legacy tracked `.githooks` wrappers. A digest-bound Rust
install publishes inactive `pre-commit`, `commit-msg`, and
`reference-transaction` artifacts under `.spice/rust/git-hooks` without
changing either Python hook set or `core.hooksPath`.

At an explicitly authorized hook cutover, preview and apply the current Rust
install plan, then activate its exact hook directory once per checkout or
linked worktree:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
./scripts/install-hooks.sh
```

The activation script refuses unless all three native hooks are executable. The
native pre-commit hook runs the canonical Rust verifier, which performs
formatting, Clippy, native and browser-target builds, deterministic app
verification, workspace tests, doctests, the machine-readable
succession-readiness check, and the still-installed Python policy gate during
coexistence. It prints each command before execution and names the first
command and exit status when verification fails. The native commit-msg hook
preserves the current fixed message-hygiene law and additionally requires every
committed `Spice-Origin` line to use the exact v1 grammar. The native
reference-transaction hook refuses a current-branch update that would abandon
commits already merged into its upstream.

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
