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

The `gate` integration tests hold repository-owned contracts to their current
implementation.

Lattice conformance compares every local path dependency in `Cargo.lock` with
the gate's acyclic workspace inventory. It fails when a workspace member is
added without an inventory entry or when a manifest adds, removes, reverses,
or cycles a local dependency edge.

Doctrine conformance holds root `DOCTRINE.md` to its settled kernel and five
decision records: every invariant is stated rather than merely named, every
decision carries rationale and explicit deletions, unsettled language is
rejected, and relative links resolve on disk.
