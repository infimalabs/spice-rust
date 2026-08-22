# Agent guidance

This repository develops and self-hosts the Rust Spice appliance.

## Cargo isolation

Resident lanes receive a private admitted `CARGO_HOME`. When a development
command can create or refresh `Cargo.lock`, or needs dependencies outside the
admitted generation, invoke the Rust-owned `dev cargo` surface through the
exact Git-common appliance:

```sh
git_common=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
spice_rs="${git_common}/.spice-install/bin/spice"
if [ ! -f "$spice_rs" ] || [ -L "$spice_rs" ] || [ ! -x "$spice_rs" ]; then
  printf '%s\n' "Rust Spice appliance is unavailable: $spice_rs" >&2
  exit 1
fi
"$spice_rs" dev cargo -- <cargo-arguments>
```

This repository currently has a hybrid command boundary: unqualified
`spice session`, `spice task`, and `spice steering` commands remain owned by
the ambient Python controls plane. The exact Rust appliance above owns only
Rust-specific surfaces such as `dev cargo`; its existence does not imply that
the current agent has a Rust lane, session, or task claim. A missing, symbolic,
non-regular, or non-executable asset is a hard stop. Do not search `PATH` for a
replacement, invoke another Cargo route, or point `CARGO_HOME` at a shared
mutable cache to work around a stale generation.

## Rust source organization

Use ordinary Rust modules all the way down when organizing handwritten source:
declare modules with `mod name;` and place their contents in `name.rs` or
`name/mod.rs`. Do not use `include!` as a handwritten-source splitting
mechanism; reserve it for generated source or cases where textual inclusion is
the intended contract.
