# Agent guidance

This repository develops and self-hosts the Rust Spice appliance.

## Cargo isolation

Resident lanes receive a private admitted `CARGO_HOME`. When a development
command can create or refresh `Cargo.lock`, or needs dependencies outside the
admitted generation, run it as `spice dev cargo -- <cargo-arguments>`. Do not
point `CARGO_HOME` at a shared mutable Cargo cache to work around a stale
generation.

## Rust source organization

Use ordinary Rust modules all the way down when organizing handwritten source:
declare modules with `mod name;` and place their contents in `name.rs` or
`name/mod.rs`. Do not use `include!` as a handwritten-source splitting
mechanism; reserve it for generated source or cases where textual inclusion is
the intended contract.
