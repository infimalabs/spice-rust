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
admitted generation, invoke the Rust-owned `dev cargo` surface through ambient
`spice`. Repository shell-wrapper policy resolves that word to the validated
Git-common appliance. Repository xtasks use the sibling `dev xtask` surface
backed by the same admitted Cargo home:

```sh
spice dev cargo -- test --locked -p spice
spice dev xtask -- verify --package spice
```

Replace the trailing example argument vector with the exact Cargo or xtask
operation required; retain the ambient `spice` command and `dev ... --`
boundary.

The exact appliance behind the wrapper owns repository development surfaces
such as `dev cargo` and `dev xtask`; its existence does not establish harness
authority. A missing, symbolic, non-regular, or non-executable asset is a hard
stop. Do not substitute another Cargo route or point `CARGO_HOME` at a shared
mutable cache to work around a stale generation.

## Permanent test value

Permanent tests are maintained product contracts, not proof that one change
worked. Add one only when a plausible bug could violate current observable
behavior: a contract, boundary, invariant, state transition, precedence rule,
or real error. Prefer deterministic, fast, in-process, in-memory tests.

Do not add a test solely because a bug once occurred. Do not retain
process-orchestration tests when the same contract can be exercised in process,
or assertions of source text, plumbing and wiring, mock echoes, incidental
defaults, exact wording, implementation shape, or retired behavior. Bare
does-not-throw, non-empty-output, and length-grew checks do not earn permanent
maintenance. Delete an existing test that enforces vestigial behavior instead
of updating its expected output.

Prove a change by reproducing the failure and exercising the actual changed
path. Use a throwaway script or smoke check when that evidence does not meet the
bar for a permanent test. Use a broader or slower process test only when it is
the indispensable boundary of a current external contract and no focused
substitute can protect that contract.

## Rust source organization

Use ordinary Rust modules all the way down when organizing handwritten source:
declare modules with `mod name;` and place their contents in `name.rs` or
`name/mod.rs`. Do not use `include!` as a handwritten-source splitting
mechanism; reserve it for generated source or cases where textual inclusion is
the intended contract.
