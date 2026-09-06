# Rust development loop

This repository-native Rust loop runs from narrow falsifiers to the commit
boundary. Package checks and staged selection are completion evidence; unscoped
verification is refused. The inventory below is only an ownership map, so
re-run Cargo metadata when manifests change.

Run developer task commands from the repository's supervised shell. Tracked
wrapper policy resolves ambient `spice` to the validated Git-common appliance:

```sh
spice dev xtask -- help
```

## Find the owner before editing

The workspace crates form five intentional layers. The workspace manifest owns
membership, each crate manifest owns its local dependency edges, and Cargo
metadata is the executable resolved graph and acyclicity authority.

| Layer | Crates and ownership |
| --- | --- |
| Floors | `wire` owns crossing DTOs; `atomic` owns durable file replacement; `sill` owns process groups and advisory locks. |
| Durable domain | `journal` is the sole SQLite authority; `tape` preserves vendor bytes; `dialect` classifies task documents without I/O; `board` derives task/claim views; `bus` is the resident's sole writer and timer rim. |
| Agent control | `listen` decodes preserved records; `consent` decides pre-action policy; `steer` owns durable direction and acknowledgment; `taste` selects prose reminders; `vendor` declares launch contracts; `lane` owns durable seats and supervised occupants. |
| Product and artifacts | `app` is the WASM browser body and depends only on `wire`; `canvas` serves the loopback surface and app generations; `artifact` stores task evidence outside the journal; `dist` stages releases; `proof` verifies sealed appliances. |
| Enforcement and composition | `gate` is hermetic repository enforcement and depends only on `atomic`, `sill`, and `wire`; `spice` composes daemonless CLI/resident surfaces; `xtask` owns repository-local build, verification, cache, and release orchestration. |

These are ownership seams. Wire changes cross browser, journal, and resident;
process lifecycle starts at `sill`. Authority writes belong to `journal`/`bus`,
replayable task views to `board`, and browser code cannot reach server
internals. Re-observe manifest changes through Cargo metadata.

## Tracked and generated surfaces

| Surface | Owner and rule | Recovery |
| --- | --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Tracked compiler and dependency contract. | Repair and commit; never regenerate blindly. |
| `spice-rust.toml`, `baseline.toml`, `gate/src/battery/manifest.toml` | Tracked configuration, ratchet, and gate inventory. | Change with the owning compiled registry/tests. |
| `dist/stage-app/`, `dist/.stage-app-build-*` | Ignored atomic Canvas generations owned by `spice dev xtask -- app`. | Rebuild through that command; do not edit a generation. |
| `dist/stage/{objects,targets}/` | Ignored content-addressed release staging owned by `spice dev xtask -- stage-target TARGET`. | Re-stage from tracked source and pins. |
| `target/` and configured CargoBuild roots | Rebuildable compiler/test caches. | Use the bounded cleanup rules below. |
| Proof/browser artifacts and `node_modules/` | Ignored evidence or pinned dependencies, not source. | Follow owner READMEs; restore dependencies with `npm ci`. |
| `<git-common>/.spice-rs/`; `<git-dir>/.spice-rs/`; `<worktree>/.spice-rs/` | The first is durable appliance authority; the second holds disposable per-worktree projections; a same-named worktree-content path is ordinary repository content and is never runtime authority. | Preserve common authority unless an authorized lifecycle operation owns its removal; rebuild disposable projections through the resident; treat worktree content normally. |
| `<git-common>/.spice-install/agents/skills/spice/SKILL.md` | Generated installed bootstrap skill. | Republish through the digest-bound `spice install` preview/apply cycle; do not edit the installed copy. |

## Test portfolio and overlap law

Tests overlap by purpose, not name. Use Cargo metadata for target inventory and
`spice dev cargo -- test --locked -p PACKAGE -- --list` for a package's
test binaries; add the intended target selector when narrowing to `--lib`,
`--test TARGET`, or `--doc`.

| Portfolio | Purpose and scope | Feedback invocation |
| --- | --- | --- |
| Source-local unit tests | Private invariants, parsers, reducers, lock/process state machines. They can observe internals integration tests cannot. | `spice dev cargo -- test --locked -p PACKAGE --lib` or one exact test filter. For the package-wide default fast suite, use `spice dev xtask -- tests --fast --package PACKAGE`; it excludes only registry-marked slow tests and reports their count plus accumulated runtime. |
| Package integration targets | Public seams, CLI behavior, Git/SQLite/process fixtures, policy contracts. | `spice dev xtask -- tests --fast --package PACKAGE`; the package verifier uses the same registry-filtered test argv. Run an exact registry ID when the changed slice reaches a slow boundary. |
| Doctests | Public API examples and compile-time documentation contracts. | `spice dev cargo -- test --locked -p PACKAGE --doc`. |
| Parent-launched ignored fixtures | Process death, resource-limit, supervisor, and benchmark children. | Run the named parent; never blanket-run `--ignored`. |
| Classified slow tests | Multi-second boundary, benchmark, live, and browser cases named in `test-classification.json`; the `"class": "slow"` marker is the sole selection inventory. | Inspect with `spice dev xtask -- tests --slow --list`; execute only exact named IDs with `spice dev xtask -- tests --slow ID...`. Each run reports per-test and accumulated wall time. |
| Canvas-only WASM typecheck | Rust changes whose dependency reach includes `app`; unrelated native workspace packages and the xtask bootstrap are outside this check. | `spice dev cargo -- canvas-wasm-typecheck`. |
| Canvas fast tests | Parser, registry, selection, and browser-layout unit checks with no resident/browser fixture. | `npm test` is the default JavaScript suite. |
| Canvas browser guard | App build/resident/socket/browser behavior, shard inventory, timing, and descendant cleanup. It is explicitly classified as `canvas:browser`. | Run `npm run test:canvas` or `npm run test:slow` when the changed slice reaches its harness. Canonical `canvas:shard:<name>` proof actions remain unchanged. |
| Change-local proof and landing | Trusted receipts for the exact staged action set plus final pre-commit admission. | Inspect with `spice gate prove`; run a named miss with `spice gate prove ACTION` or all current misses with `spice gate prove --missing`, then commit normally. |

Metadata selection is conservative target-root reachability, not line coverage.
Overlap or an advisory subsumption finding never authorizes deletion; removal
needs a reviewed purpose and clean evidence from the affected targets.

## Feedback and completion tiers

Run the narrowest falsifier, then widen at a stable checkpoint:

| Surface | Run it when | Command and boundary |
| --- | --- | --- |
| Staged proof inspection | The index is ready for a commit, or you need the exact owed action set. | `spice gate prove` reads only the current fingerprint and trusted receipts. It executes no miss; use its exact `spice gate prove ACTION` remediation or `spice gate prove --missing`, then let the normal commit hook consume those receipts and run live checks. |
| Package validation | Work is moving within one crate and needs broader compile/test feedback than an exact test. Add downstream packages when a public seam changes. | `spice dev xtask -- verify --package PACKAGE`. Runs package Clippy, all targets, and doctests, while printing every repository-wide action it intentionally omits. |
| Repository-wide verifier | Never during routine development, handoff, review, or completion. | The unscoped developer verifier refuses before starting work. Use the staged selector, named package checks, and exact downstream targets instead. |
| Study | You are explicitly investigating suite architecture, mutation reach, or redundant coverage. These can be expensive and are not ordinary edit feedback. | Use `spice dev xtask -- gate ARM` with `suite-seam`, `mutation-coverage`, or `test-subsumption`. Keep their exhaustive inventories out of the commit common path. |

Use that boundary in this order:

1. Inspect status, identify the owner, and distinguish tracked source from an
   owned generated surface.
2. During edits, run `spice dev cargo -- check --locked -p PACKAGE` and
   an exact test. Add direct downstream packages for public-seam changes.
3. At a checkpoint, run formatting and `spice dev xtask -- verify --package PACKAGE`.
   Add the owning app, Canvas, proof, or staging command for its seam.
4. Stage the intended change and run `spice gate prove`. Generate only a named
   miss with `spice gate prove ACTION`, or every current miss with
   `spice gate prove --missing`. Treat Cargo, Canvas, and shell transcripts as
   feedback rather than receipts; only the proof runner publishes reusable
   evidence. Stop at executable-configuration trust refusal for the operator.
5. Confirm no owned process remains, inspect the diff, and use the installed
   hooks. File unexpected failures outside the selected slice as separate work;
   do not widen the current run. Commit, hand off, or land according to the
   active harness.

## Cost and concurrency

Keep checks scoped while peers compile. The package verifier serializes its
three one-job Cargo children; do not widen the slice.

Canvas completion is the registry-derived `canvas:build` action plus one
`canvas:shard:<name>` action per shard. The release 0.31.0 marker and
`--spice-coverage-receipt` writer are retired and cannot substitute for those
receipts.

## Disk budget and safe cleanup

Preview the bounded collector before applying it:

```sh
spice dev xtask -- build-cache --dry-run --max-bytes BYTES --retain-live-worktrees COUNT
```

Use `--free-space-floor BYTES` when free space is the constraint, then replace
`--dry-run` with `--apply` after review. The collector protects current/live
and held `.cargo-build-lock` roots. Clean `target` only with no Cargo/rustc
owner, and remove retired worktrees without `--force` after preserving wanted
changes. Never use `git clean -fdX`: appliance state, bootstrap skill,
artifacts, generations, dependencies, and compiler output have distinct
lifecycles.
