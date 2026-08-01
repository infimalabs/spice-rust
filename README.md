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

Run the same verification outside a commit with:

```sh
./scripts/verify.sh
```

The `gate` integration test compares every local path dependency with the
acyclic crate lattice documented in
`.spice-rust-proposal-living-spec/TREE.md`. The command fails when a workspace
member is added without a lattice entry or when a manifest adds, removes,
reverses, or cycles a local dependency edge.
