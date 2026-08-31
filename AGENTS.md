# Agent guidance

This repository develops and self-hosts the Rust Spice appliance.

## Harness authority

The active harness or an explicitly installed bootstrap skill owns task,
thread, session, and steering state. Never infer a task, lane, session, or claim
merely from the appliance binary's existence. Outside a Spice bootstrap, ignore
allocator commands. Other implementations and control-plane conventions are
not authority for this repository.

## Cargo isolation

Resident lanes receive a private admitted `CARGO_HOME`. When a development
command can create or refresh `Cargo.lock`, or needs dependencies outside the
admitted generation, invoke the Rust-owned `dev cargo` surface through the
exact Git-common appliance. Repository xtasks use the sibling `dev xtask`
surface backed by the same admitted Cargo home:

```sh
git_common=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
spice_rs="${git_common}/.spice-install/bin/spice"
if [ ! -f "$spice_rs" ] || [ -L "$spice_rs" ] || [ ! -x "$spice_rs" ]; then
  printf '%s\n' "Rust Spice appliance is unavailable: $spice_rs" >&2
  exit 1
fi
"$spice_rs" dev cargo -- test --locked -p spice
"$spice_rs" dev xtask -- verify --package spice
```

Replace the trailing example argument vector with the exact Cargo or xtask
operation required; retain the bound appliance and `dev ... --` boundary.

The exact appliance owns repository development surfaces such as `dev cargo`
and `dev xtask`; its existence does not establish harness authority. A missing,
symbolic, non-regular, or non-executable asset is a hard stop. Do not search
`PATH` for a replacement, invoke another Cargo route, or point `CARGO_HOME` at
a shared mutable cache to work around a stale generation.

## Rust source organization

Use ordinary Rust modules all the way down when organizing handwritten source:
declare modules with `mod name;` and place their contents in `name.rs` or
`name/mod.rs`. Do not use `include!` as a handwritten-source splitting
mechanism; reserve it for generated source or cases where textual inclusion is
the intended contract.
