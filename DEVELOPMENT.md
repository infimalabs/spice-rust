# Rust development loop

This is the repository-native loop for changing the Rust appliance. It orders
feedback from the narrowest falsifier to the commit boundary. Package checks
and staged target selection are completion evidence; unscoped repository-wide
verification is not admitted.

The inventory is a readable ownership map, not metadata authority. Re-run Cargo
metadata when manifests change.

Bind and validate the exact Git-common Rust appliance before using any
developer task command below:

```sh
git_common=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
spice_rs="${git_common}/.spice-install/bin/spice"
if [ ! -f "$spice_rs" ] || [ -L "$spice_rs" ] || [ ! -x "$spice_rs" ]; then
  printf '%s\n' "Rust Spice appliance is unavailable: $spice_rs" >&2
  exit 1
fi
```

## Find the owner before editing

The workspace crates form five intentional layers. The workspace manifest owns
membership; `gate/tests/dependency_lattice.rs` is the checked source of truth
for every local dependency edge and acyclicity.

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
| `dist/stage-app/`, `dist/.stage-app-build-*` | Ignored atomic Canvas generations owned by `"$spice_rs" dev xtask -- app`. | Rebuild through that command; do not edit a generation. |
| `dist/stage/{objects,targets}/` | Ignored content-addressed release staging owned by `"$spice_rs" dev xtask -- stage-target TARGET`. | Re-stage from tracked source and pins. |
| `target/` and configured CargoBuild roots | Rebuildable compiler/test caches. | Use the bounded cleanup rules below. |
| Proof/browser artifacts and `node_modules/` | Ignored evidence or pinned dependencies, not source. | Follow owner READMEs; restore dependencies with `npm ci`. |
| `<git-common>/.spice-rs/`; `<git-dir>/.spice-rs/`; `<worktree>/.spice-rs/` | The first is durable appliance authority; the second holds disposable per-worktree projections; a same-named worktree-content path is ordinary repository content and is never runtime authority. | Preserve common authority unless an authorized lifecycle operation owns its removal; rebuild disposable projections through the resident; treat worktree content normally. |
| `<git-common>/.spice-install/agents/skills/spice/SKILL.md` | Generated installed bootstrap skill. | Republish through the digest-bound `spice install` preview/apply cycle; do not edit the installed copy. |

## Test portfolio and overlap law

Cargo unit, integration, doctest, and Canvas surfaces overlap by purpose, not by
name. Use Cargo metadata for target inventory and `"$spice_rs" dev cargo --
test --locked -p PACKAGE -- --list` for the selected package's test-binary
inventory; add the same target selector used by the intended run when narrowing
to `--lib`, `--test TARGET`, or `--doc`.

| Portfolio | Purpose and scope | Feedback invocation |
| --- | --- | --- |
| Source-local unit tests | Private invariants, parsers, reducers, lock/process state machines. They can observe internals integration tests cannot. | `"$spice_rs" dev cargo -- test --locked -p PACKAGE --lib` or one exact test filter. |
| Package integration targets | Public seams, CLI behavior, Git/SQLite/process fixtures, policy contracts. | `"$spice_rs" dev cargo -- test --locked -p PACKAGE --all-targets`; normally use the scoped verifier below. |
| Doctests | Public API examples and compile-time documentation contracts. | `"$spice_rs" dev cargo -- test --locked -p PACKAGE --doc`. |
| Parent-launched ignored fixtures | Process death, resource-limit, supervisor, and benchmark children. | Run the named parent; never blanket-run `--ignored`. |
| Canvas-only WASM typecheck | Rust changes whose dependency reach includes `app`; unrelated native workspace packages and the xtask bootstrap are outside this check. | `"$spice_rs" dev cargo -- canvas-wasm-typecheck`. |
| Canvas browser guard | App build/resident/socket/browser behavior, shard inventory, timing, and descendant cleanup. | Run `npm run test:canvas` when the changed slice reaches its harness. |
| Change-local gate, battery, and landing | Staged path-to-test and path-to-policy evidence plus final selected contract admission. | `"$spice_rs" dev cargo -- run --quiet --locked --package spice --bin spice -- gate selection`. |

Metadata selection is conservative target-root reachability, not line coverage.
Equal names, overlap, or an advisory subsumption finding never authorizes test
deletion; removal needs a reviewed purpose and clean evidence from the affected
targets.

## Feedback and completion tiers

Run the narrowest command that can falsify the current edit, then widen at a
stable checkpoint. The four surfaces have distinct jobs:

| Surface | Run it when | Command and boundary |
| --- | --- | --- |
| Targeted staged gate | The index is ready for a commit, or you need exact change-to-check evidence. | `"$spice_rs" dev cargo -- run --quiet --locked --package spice --bin spice -- gate selection`. Known paths select affected Cargo targets and focused canonical policy arms; known selection, policy, build, and toolchain authority selects explicit exhaustive reach. Unclassified and out-of-workspace Rust/manifest paths refuse. |
| Package validation | Work is moving within one crate and needs broader compile/test feedback than an exact test. Add downstream packages when a public seam changes. | `"$spice_rs" dev xtask -- verify --package PACKAGE`. Runs package Clippy, all targets, and doctests, while printing every repository-wide action it intentionally omits. |
| Repository-wide verifier | Never during routine development, handoff, review, or completion. | The unscoped developer verifier refuses before starting work. Use the staged selector, named package checks, and exact downstream targets instead. |
| Study | You are explicitly investigating suite architecture, mutation reach, or redundant coverage. These can be expensive and are not ordinary edit feedback. | Use `"$spice_rs" dev xtask -- gate ARM` with `suite-seam`, `mutation-coverage`, or `test-subsumption`. Keep their exhaustive inventories out of the commit common path. |

Use that boundary in this order:

1. Inspect status, identify the owner, and distinguish tracked source from an
   owned generated surface.
2. During edits, run `"$spice_rs" dev cargo -- check --locked -p PACKAGE` and
   an exact test. Add direct downstream packages for public-seam changes.
3. At a checkpoint, run formatting and `"$spice_rs" dev xtask -- verify --package PACKAGE`.
   Add the owning app, Canvas, lattice, proof, or staging command for its seam.
4. Stage the intended change and run `"$spice_rs" dev cargo -- run --quiet
   --locked --package spice --bin spice -- gate selection` (the installed
   dispatcher invokes that same public leaf through tracked policy). Inspect its
   typed selection, or repair the exact path named by a deterministic
   classification refusal.
5. Confirm no owned process remains, inspect the diff, and use the installed
   hooks. File unexpected failures outside the selected slice as separate work;
   do not widen the current run. Commit, hand off, or land according to the
   active harness.

## Cost and concurrency

Keep checks scoped while peers compile. The package verifier serializes its
three Cargo children and grants each one build job; do not widen the slice to
compete for the same machine.

## Disk budget and safe cleanup

Preview the bounded collector before applying it:

```sh
"$spice_rs" dev xtask -- build-cache --dry-run --max-bytes BYTES --retain-live-worktrees COUNT
```

Use `--free-space-floor BYTES` instead when free space is the constraint, then
repeat the reviewed request with `--apply`. The collector protects current/live
roots and held `.cargo-build-lock` roots. Clean `target` only after confirming
no Cargo/rustc owner, and remove retired worktrees without `--force` only after
preserving wanted changes. Never use `git clean -fdX`: appliance state, the
generated bootstrap skill, artifacts, generations, dependencies, and compiler
output do not share one disposal policy.
