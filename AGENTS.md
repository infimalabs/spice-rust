# Agent guidance

This repository develops and self-hosts the Rust Spice appliance.

## Rust source organization

Use ordinary Rust modules all the way down when organizing handwritten source:
declare modules with `mod name;` and place their contents in `name.rs` or
`name/mod.rs`. Do not use `include!` as a handwritten-source splitting
mechanism; reserve it for generated source or cases where textual inclusion is
the intended contract.
