# Rust development loop

This is the repository-native loop for an agent changing the Rust port. It is
an optimization of feedback order, not a smaller completion contract. Package
checks are evidence while work is moving; `cargo xtask verify` is the only
completion verifier.

The inventory below was read from Cargo metadata and live files at `dd52ab23`
on 2026-08-11. Re-run metadata rather than copying this table when manifests
change.

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
| `fold/src/registry.rs` plus `fold/build/{registry,admission}.rs` | Tracked fold declaration and build-time validator; `build.rs` generates no source file. | Edit both sides only when the fold contract changes. |
| `dist/stage-app/`, `dist/.stage-app-build-*` | Ignored atomic Canvas generations owned by `cargo xtask app`. | Rebuild with `cargo xtask app`; do not edit a generation. |
| `dist/stage/{objects,targets}/` | Ignored content-addressed release staging owned by `cargo xtask stage-target TARGET`. | Re-stage from tracked source and pins. |
| `target/proof-image/`, `target/proof-native-darwin/` | Proof archives, logs, status, contexts, and native runner outputs. | Follow the proof READMEs; wholesale deletion gives up cheap recovery evidence. |
| `target/` and configured CargoBuild roots | Rebuildable compiler/test caches. | Use the bounded cleanup rules below. |
| `node_modules/` | Pinned Playwright dependency. | Restore with `npm ci`. |
| `artifacts/`, `test-results/`, `.playwright-mcp/` | Ignored browser/test evidence, not source. | Delete only after evidence is handed off or declared disposable. |
| `.spice/`, `.agents/` | Ignored control state and skills. | Not a cache. Never clean these with an ignored-file sweep. |

## Test portfolio and overlap law

Cargo currently declares 147 testable targets: 121 integration targets plus
library/binary unit targets. A complete `--list` found 2,661 Rust tests; doctest
listing found 14 more. The separate Canvas guard is a two-worker Playwright
coordinator with 120-second shard and 180-second coordinator bounds.

| Portfolio | Purpose and scope | Feedback invocation |
| --- | --- | --- |
| Source-local unit tests | Private invariants, parsers, reducers, lock/process state machines. They can observe internals integration tests cannot. | `cargo test --locked -p PACKAGE --lib` or one exact test filter. |
| Package integration targets | Public seams, CLI behavior, Git/SQLite/process fixtures, policy contracts. `spice` has 35, `gate` 23, `wire` 20, `app` 13, and the other crates 30 combined. | `cargo test --locked -p PACKAGE --all-targets`; normally use the scoped verifier below. |
| Doctests | Public API examples and compile-time documentation contracts. | `cargo test --locked -p PACKAGE --doc`. |
| Parent-launched ignored fixtures | Process death, resource-limit mutation, hard-supervisor, and measured benchmark children. Their named parent tests own invocation and cleanup. | Run the parent test; do not blanket-run `--ignored`. |
| Canvas browser guard | App build/resident/socket/browser behavior, shard inventory, timing, and descendant cleanup. | Full verifier; use `npm run test:canvas` only when working directly on its harness. |
| Change-local gate, battery, and landing | Staged path-to-test and path-to-policy evidence, the complete policy battery, and final contract admission. | `cargo xtask verify-action selection` for the staged common path; full verifier for completion. `verify-action battery` is explicit complete-policy evidence only. |

Metadata selection is conservative target-root reachability, not line coverage.
The long-running test-evidence authority currently declares its per-test
coverage denominator unknown. Its subsumption findings are advisory even when
coverage exists. Therefore equal names, overlapping targets, or a containment
candidate never authorize deleting a test. Removal requires a separately
reviewed purpose/defect argument, authoritative complete coverage, mutation
evidence where applicable, and a clean canonical verifier.

## Feedback and completion tiers

Run the narrowest command that can falsify the current edit, then widen at a
stable checkpoint. The four surfaces have distinct jobs:

| Surface | Run it when | Command and boundary |
| --- | --- | --- |
| Targeted staged gate | The index is ready for a commit, or you need exact change-to-check evidence. This is the installed hook's ordinary path. | `cargo xtask verify-action selection`. Known paths select affected Cargo targets and focused canonical policy arms. Unknown paths and selector/toolchain authorities escalate to every target and all 27 arms. It is not completion evidence. |
| Package validation | Work is moving within one crate and needs broader compile/test feedback than an exact test. Add downstream packages when a public seam changes. | `cargo xtask verify --package PACKAGE`. Runs entry-point audit, package Clippy, all targets, and doctests, while printing every full-only omission. |
| Full verifier | Immediately before handoff or task completion, after package/seam checks are clean. | `cargo xtask verify`. Runs the unchanged 13-action repository completion graph; no targeted or package result substitutes for it. |
| Study | You are explicitly investigating suite architecture, mutation reach, or redundant coverage. These can be expensive and are not ordinary edit feedback. | `cargo xtask gate suite-seam`, `cargo xtask gate mutation-coverage`, or `cargo xtask gate test-subsumption`. Keep their exhaustive inventories out of the commit common path. |

Use that boundary in this order:

1. Inspect `git status --short`, identify the owning crate and boundary, and
   check whether the path is tracked source or an owned generated surface.
2. During edits, use `cargo check --locked -p PACKAGE` and an exact unit or
   integration test. Add direct downstream packages when a public type or
   manifest edge changes.
3. At a package checkpoint, run `cargo fmt --all -- --check`, then
   `cargo xtask verify --package PACKAGE`. The scoped verifier runs entry-point
   audit, package Clippy, all-target tests, and doctests; it deliberately skips
   every full-only action and gate arm and prints that omission inventory.
4. Add the seam-specific command: `cargo xtask app` for app generation,
   the Canvas guard for browser/runtime behavior,
   `cargo test --locked -p gate --test dependency_lattice` for manifest edges,
   or the owning proof/staging command for release surfaces.
5. Stage the intended change and run `cargo xtask verify-action selection` (the
   installed hook repeats it silently). Inspect its typed selection if it
   escalates; do not weaken the exhaustive fallback.
6. Once, immediately before commit or handoff, run `cargo xtask verify`. Do not
   substitute targeted/package checks, the battery action, a study, or a prior
   worktree's result.
7. Confirm no owned Cargo/rustc/process group remains, inspect the final diff,
   and commit through the installed hooks.

The strict verifier remains 13 actions, a 180-second test-phase cutoff, and a
300-second hard envelope. It prebuilds workspace tests, then re-observes Cargo
pressure. Two or more remaining peers use one outer worker and one workspace
lane with a CPU-divided Cargo grant; an unreadable probe fails workers, jobs,
and lanes closed to one.

## Measured cost and concurrency

On the 12-core, 24 GiB host, a warm `cargo check -p xtask` took 1.21 seconds.
The scoped `xtask` verifier took 22.89 seconds: 0.26 seconds Clippy, 21.15
seconds all-target tests, and 0.28 seconds doctests. Merely listing all Rust
targets took 16.20 seconds after a 15.13-second compile and enumerated 2,661
tests; doctest listing took 3.31 seconds.

The staged selector was measured on the same host with a representative
two-file `gate` backend change. In a fresh shared clone with a new writable
Cargo output it admitted in 79.28 seconds; an immediate repeat admitted in
61.80 seconds. Both runs selected the affected `gate` targets and 10 focused
policy arms, stayed silent on success, and did not run the full verifier or any
study arm. These are cold-output and warm-repeat measurements of that backend
case, not promises for a documentation-only change or an unknown-path
exhaustive escalation.

The isolated fleet matrix in `gate/THROUGHPUT_AUDIT.md` is the heavy-workflow
limit: 4, 8, and 10 agent-equivalent runs peaked at 51, 63, and 99 concurrent
`rustc` processes and none received a GO. Four is the first measured saturation
and the measured efficient ceiling is below four for that cold workload. The
Canvas action took 89.9–116.7 seconds in those runs. Independent Cargo
jobservers, duplicate isolated compilation, and linear cache growth are the
cause; memory and disk capacity were not exhausted.

Consequently, prefer scoped checks while peers are compiling and avoid starting
multiple full verifiers together. Do not raise `CARGO_BUILD_JOBS`; the verifier
owns its local grant. When four or more agents are active, serialize heavy full
verification on the host and let other lanes perform source review, exact tests,
or non-Cargo work until pressure clears.

## Disk budget and safe cleanup

The same snapshot measured 18.49 GiB in this worktree's `target`, 9.58 GiB in
its CargoBuild root, 70.2 GiB across the machine CargoBuild hierarchy, and
103–104 GiB free. Eleven linked worktree directories ranged from 0.18 to 22.49
GiB and totaled about 129 GiB; the shared bare Git directory was 12.89 GiB.
`dist/stage-app` was 142 MiB, `node_modules` 18 MiB, and browser artifacts 3
MiB. Compiler caches and retired worktrees dominate cleanup value.

Use these rules:

- Above 40 GiB of CargoBuild cache, first run
  `cargo xtask build-cache --dry-run --max-bytes 40GiB --retain-live-worktrees 2`.
  Below 100 GiB free, use `--free-space-floor 100GiB` in place of `--max-bytes`.
  The measured size-cap dry run selected 31.9 GiB while retaining current/live
  roots and skipping 12.9 GiB. Review the named roots, then repeat the same
  request with `--apply`. Removed bytes are recoverable by recompilation; held
  `.cargo-build-lock` roots are never candidates.
- Clean this worktree's ordinary target only when its cache is corrupt or disk
  pressure justifies losing warm feedback: confirm no Cargo/rustc process owns
  it, then run `cargo clean --target-dir target`. Recovery is a normal rebuild.
- For a retired worktree, inspect its own status and active claim/processes,
  commit or preserve every wanted change, then use `git worktree remove PATH`
  without `--force`. Preview stale administrative cleanup with
  `git worktree prune --dry-run --verbose` before `git worktree prune`.
- Never use `git clean -fdX` here. Its dry run includes `.agents/`, `.spice/`,
  artifacts, app generations, dependencies, and compiler output in one sweep;
  several are control state or retained evidence rather than disposable cache.
