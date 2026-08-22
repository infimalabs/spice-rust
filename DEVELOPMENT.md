# Rust development loop

This is the repository-native loop for an agent changing the Rust port. It is
an optimization of feedback order. Package checks and staged target selection
are the completion evidence; repository-wide verification is retired.

The inventory below is a dated map, not metadata authority. Re-run Cargo
metadata when manifests change.

## Find the owner before editing

The 23 crates form five intentional layers. `gate/tests/dependency_lattice.rs`
is the checked source of truth for every local dependency edge and acyclicity.

| Layer | Crates and ownership |
| --- | --- |
| Floors | `wire` owns crossing DTOs; `atomic` owns durable file replacement; `sill` owns process groups and advisory locks. |
| Durable domain | `journal` is the sole SQLite authority; `tape` preserves vendor bytes; `dialect` classifies task documents without I/O; `fold` owns disposable projections; `board` derives task/claim views; `bus` is the resident's sole writer and timer rim. |
| Agent control | `listen` decodes preserved records; `consent` decides pre-action policy; `steer` owns durable direction and acknowledgment; `taste` selects prose reminders; `vendor` declares launch contracts; `lane` owns durable seats and supervised occupants. |
| Product and artifacts | `app` is the WASM browser body and depends only on `wire`; `canvas` serves the loopback surface and app generations; `artifact` stores task evidence outside the journal; `dist` stages releases; `proof` verifies sealed appliances. |
| Enforcement and composition | `gate` is hermetic repository enforcement and depends only on `atomic`, `sill`, and `wire`; `spice` composes daemonless CLI/resident surfaces; `xtask` owns repository-local build, verification, cache, and release orchestration. |

Treat these as ownership seams. A wire-shape change crosses browser, journal,
and resident boundaries. A process-lifecycle change starts at `sill`, not in a
caller-specific retry. Authority writes belong to `journal`/`bus`; replayable
views belong to `board`/`fold`. Browser code must not reach server internals.
Manifest edge changes must update the dependency lattice in the same change.

## Tracked and generated surfaces

| Surface | Owner and rule | Recovery |
| --- | --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Tracked compiler and dependency contract. | Repair and commit; never regenerate blindly. |
| `spice-rust.toml`, `baseline.toml`, `gate/src/battery/manifest.toml` | Tracked configuration, ratchet, and gate inventory. | Change with the owning compiled registry/tests. |
| `fold/src/registry.rs`, `fold/build/{registry,admission}.rs` | Tracked fold declaration and validator; `build.rs` generates no source. | Edit both sides with the fold contract. |
| `dist/stage-app/`, `dist/.stage-app-build-*` | Ignored atomic Canvas generations owned by `cargo xtask app`. | Rebuild with `cargo xtask app`; do not edit a generation. |
| `dist/stage/{objects,targets}/` | Ignored content-addressed release staging owned by `cargo xtask stage-target TARGET`. | Re-stage from tracked source and pins. |
| `target/` and configured CargoBuild roots | Rebuildable compiler/test caches. | Use the bounded cleanup rules below. |
| Proof/browser artifacts and `node_modules/` | Ignored evidence or pinned dependencies, not source. | Follow owner READMEs; restore dependencies with `npm ci`. |
| `.spice/`, `.agents/` | Ignored control state and skills, not caches. | Never include them in an ignored-file sweep. |

## Test portfolio and overlap law

Cargo unit, integration, doctest, and Canvas surfaces overlap by purpose, not by
name. Use Cargo metadata and `--list` for the current inventory.

| Portfolio | Purpose and scope | Feedback invocation |
| --- | --- | --- |
| Source-local unit tests | Private invariants, parsers, reducers, lock/process state machines. They can observe internals integration tests cannot. | `cargo test --locked -p PACKAGE --lib` or one exact test filter. |
| Package integration targets | Public seams, CLI behavior, Git/SQLite/process fixtures, policy contracts. | `cargo test --locked -p PACKAGE --all-targets`; normally use the scoped verifier below. |
| Doctests | Public API examples and compile-time documentation contracts. | `cargo test --locked -p PACKAGE --doc`. |
| Parent-launched ignored fixtures | Process death, resource-limit, supervisor, and benchmark children. | Run the named parent; never blanket-run `--ignored`. |
| Canvas-only WASM typecheck | Rust changes whose dependency reach includes `app`; unrelated native workspace packages and the xtask bootstrap are outside this check. | `cargo canvas-wasm-typecheck`. |
| Canvas browser guard | App build/resident/socket/browser behavior, shard inventory, timing, and descendant cleanup. | Run `npm run test:canvas` when the changed slice reaches its harness. |
| Change-local gate, battery, and landing | Staged path-to-test and path-to-policy evidence plus final selected contract admission. | `cargo run --quiet --locked --package spice --bin spice -- gate selection`. |

Metadata selection is conservative target-root reachability, not line coverage.
Equal names, overlap, or an advisory subsumption finding never authorizes test
deletion; removal needs a reviewed purpose and clean evidence from the affected
targets.

## Feedback and completion tiers

Run the narrowest command that can falsify the current edit, then widen at a
stable checkpoint. The four surfaces have distinct jobs:

| Surface | Run it when | Command and boundary |
| --- | --- | --- |
| Targeted staged gate | The index is ready for a commit, or you need exact change-to-check evidence. | `cargo run --quiet --locked --package spice --bin spice -- gate selection`. Known paths select affected Cargo targets and focused canonical policy arms. Selector/toolchain authority, unclassified, and out-of-workspace Rust/manifest paths refuse before execution. |
| Package validation | Work is moving within one crate and needs broader compile/test feedback than an exact test. Add downstream packages when a public seam changes. | `cargo xtask verify --package PACKAGE`. Runs package Clippy, all targets, and doctests, while printing every repository-wide action it intentionally omits. |
| Repository-wide verifier | Never during routine development, handoff, review, or completion. | Bare `cargo xtask verify` refuses before starting work. Use the staged selector, named package checks, and exact downstream targets instead. |
| Study | You are explicitly investigating suite architecture, mutation reach, or redundant coverage. These can be expensive and are not ordinary edit feedback. | `cargo xtask gate suite-seam`, `cargo xtask gate mutation-coverage`, or `cargo xtask gate test-subsumption`. Keep their exhaustive inventories out of the commit common path. |

Use that boundary in this order:

1. Inspect status, identify the owner, and distinguish tracked source from an
   owned generated surface.
2. During edits, run `cargo check --locked -p PACKAGE` and an exact test. Add
   direct downstream packages for public-seam changes.
3. At a checkpoint, run formatting and `cargo xtask verify --package PACKAGE`.
   Add the owning app, Canvas, lattice, proof, or staging command for its seam.
4. Stage the intended change and run `cargo run --quiet --locked --package
   spice --bin spice -- gate selection` (the installed dispatcher invokes that
   same public leaf through tracked policy). Inspect its typed local selection,
   or repair the exact path named by a deterministic authority/classification
   refusal.
5. Confirm no owned process remains, inspect the diff, and use the installed
   hooks. File unexpected failures outside the selected slice as separate work;
   do not widen the current run. Commit and land the task.

## Measured cost and concurrency

On the measured 12-core, 24 GiB host, a warm `cargo check -p xtask` took 1.21
seconds and the scoped `xtask` verifier 22.89 seconds. A representative two-file
`gate` change selected its affected targets and 10 local arms in 79.28 seconds
with fresh output and 61.80 seconds on immediate repeat; it did not run the full
verifier or a study. These are observations, not latency promises.

The cold isolated fleet matrix in
[`gate/THROUGHPUT_AUDIT.md`](gate/THROUGHPUT_AUDIT.md) found saturation at four
agent-equivalent runs; 4, 8, and 10 runs peaked at 51, 63, and 99 concurrent
`rustc` processes and none received a GO. Keep checks scoped while peers compile.
The package verifier serializes its three Cargo children and grants each one
build job; do not widen the slice to compete for the same machine.

## Disk budget and safe cleanup

The measured snapshot held 70.2 GiB across CargoBuild and about 129 GiB across
11 linked worktrees; compiler caches and retired worktrees dominate recoverable
space. Preview the bounded collector before applying it:

```sh
cargo xtask build-cache --dry-run --max-bytes 40GiB --retain-live-worktrees 2
```

Use `--free-space-floor 100GiB` instead when free space is the constraint, then
repeat the reviewed request with `--apply`. The collector protects current/live
roots and held `.cargo-build-lock` roots. Clean `target` only after confirming
no Cargo/rustc owner, and remove retired worktrees without `--force` only after
preserving wanted changes. Never use `git clean -fdX`: `.agents/`, `.spice/`,
artifacts, generations, dependencies, and compiler output do not share one
disposal policy.
