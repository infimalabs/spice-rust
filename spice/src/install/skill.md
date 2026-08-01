---
name: spice-rust
description: Exercise the repository-local Rust Spice appliance explicitly during coexistence.
---

# Rust Spice coexistence

The ordinary `spice` command, repository hooks, task allocator, and live
authority remain owned by the Python harness until the operator authorizes
cutover.

Use `.spice/rust/bin/spice-rust` only when a task explicitly calls for a Rust
release-one command. Daemonless inspection is safe in the governed worktree.
Run `.spice/rust/bin/spice-rust up` and every mutating Rust workflow only in an
isolated trial repository until cutover. Never import Python authority bytes
into Rust state.
