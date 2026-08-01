# Spice Rust

## Workspace verification

Run the lattice verification before handing work to another agent:

```sh
cargo test --workspace --all-targets
```

The `gate` integration test compares every local path dependency with the
acyclic crate lattice documented in
`.spice-rust-proposal-living-spec/TREE.md`. The command fails when a workspace
member is added without a lattice entry or when a manifest adds, removes,
reverses, or cycles a local dependency edge.
