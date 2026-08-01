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

The `gate` integration tests are the two closure gates FRAME J74 names as
checks rather than prose, and both hold the workspace to the documents of
record.

Lattice conformance compares every local path dependency with the acyclic
crate lattice documented in `.spice-rust-proposal-living-spec/TREE.md`. It
fails when a workspace member is added without a lattice entry or when a
manifest adds, removes, reverses, or cycles a local dependency edge.

Corpus conformance holds `.spice-rust-proposal-living-spec/DOCTRINE.md` to the
population map in `SCAFFOLD.md`: every slot filled in order, every kernel
invariant stated rather than merely named, each of the five wave-zero decision
records carrying its rationale and the deletions that make it a decision, no
unsettled language, and every corpus link resolving on disk. Doctrine that
lives only in prose drifts unobserved; these assertions are what keep it from
doing so quietly.
