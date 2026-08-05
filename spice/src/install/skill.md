---
name: spice-rust
description: Exercise the repository-local Rust Spice appliance explicitly.
---

# Rust Spice appliance

The digest-bound Rust hook surface owns repository verification after
`scripts/install-hooks.sh` activates it for the current worktree. The task
allocator and live resident authority retain their current operator-selected
implementations until their separate cutovers.

Use `.spice/rust/bin/spice-rust` only when a task explicitly calls for a Rust
release-one command. Daemonless inspection is safe in the governed worktree.
Run `.spice/rust/bin/spice-rust up` and every mutating Rust workflow only in an
isolated trial repository until live-authority cutover. Never import existing
authority bytes into Rust state.
