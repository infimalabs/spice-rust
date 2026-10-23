# Rust development loop

Run developer commands from the repository's supervised shell, where ambient
`spice` resolves to the Git-common appliance (`spice dev xtask -- help`). Cargo
isolation is described in [spice/CARGO_ISOLATION.md](spice/CARGO_ISOLATION.md);
commit proof in [Gate evidence](docs/product/planes/gate-evidence.md).

## Find the owner before editing

The workspace manifest owns crate membership, each crate manifest its
dependency edges, and Cargo metadata the resolved graph.

| Layer | Crates and ownership |
| --- | --- |
| Floors | `wire` owns crossing DTOs; `atomic` owns durable file replacement; `sill` owns process groups and advisory locks; `door` owns bounded one-message local doors between processes of one user and the Windows launch channel from a parent to its exact child. |
| Durable domain | `journal` is the sole SQLite authority; `tape` preserves vendor bytes; `dialect` classifies task documents without I/O; `board` derives task/claim views; `bus` is the resident's sole writer and timer rim. |
| Agent control | `listen` decodes preserved records; `consent` decides pre-action policy; `steer` owns durable direction and acknowledgment; `taste` selects prose reminders; `vendor` declares launch contracts; `lane` owns durable seats and supervised occupants. |
| Product and artifacts | `app` is the WASM browser body and depends only on `wire`; `serve` serves the loopback surface and app generations; `artifact` stores task evidence outside the journal; `dist` stages releases; `proof` verifies sealed appliances. |
| Enforcement and composition | `gate` is hermetic repository enforcement and depends only on `atomic`, `sill`, and `wire`; `spice` composes daemonless CLI/resident surfaces; `xtask` owns repository-local build, verification, cache, and release orchestration. |

Authority writes belong to `journal`/`bus`, replayable task views to `board`,
and browser code cannot reach server internals.

## Tracked and generated surfaces

| Surface | Owner and rule |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Tracked compiler and dependency contract; repair and commit, never regenerate blindly. |
| `spice.toml`, `baseline.toml`, `gate/src/battery/manifest.toml` | Tracked configuration, ratchet, and gate inventory; change with the owning compiled registry. |
| `dist/stage-app/` | Ignored atomic app generations owned by `spice dev xtask -- app`; rebuild, never edit a generation. |
| `dist/stage/` | Ignored content-addressed release staging owned by `spice dev xtask -- stage-target TARGET`. |
| `target/` | Rebuildable compiler and test caches; see cleanup below. |
| `<git-common>/.spice/` | Durable appliance authority; removed only by an authorized lifecycle operation. |
| `<git-dir>/.spice/` | Disposable per-worktree projections the resident rebuilds. A `.spice` inside worktree content is ordinary content, never runtime authority. |
| `<git-common>/.spice-install/` | Installed appliance, hooks, and bootstrap skill; republish through `spice install`, never edit. |

## Feedback

Permanent tests follow [AGENTS.md](AGENTS.md#permanent-test-value). Retained
data belongs behind a one-time forward migration or an owner-naming refusal;
never preserve an earlier release's DTO, command path, or process fixture as another
executable test arm.

| Surface | Command and boundary |
| --- | --- |
| Exact test | `spice dev cargo -- test --locked -p PACKAGE` with a target selector or test filter. |
| Package | `spice dev xtask -- verify --package PACKAGE` runs package Clippy, all targets, and doctests as serial one-job Cargo children, and prints every repository-wide action it omits. Add downstream packages when a public seam changes. |
| App WASM typecheck | `spice dev cargo -- app-wasm-typecheck` checks only `app` for `wasm32`. |
| Browser | `spice dev xtask -- app-demo --open` first stages the current worktree's selected app sources through `spice dev xtask -- app`'s builder, compiling only when no current generation carries their identity, then serves the current native `spice` and that generation in an isolated demo root; it reports the source identity beside the served generation and refuses before launching when staging fails or the generation moves. `app-demo` flushes its canonical endpoint as soon as that server accepts connections, remains serving until stopped, removes the root on every ending, and reports the cleanup ([`app/BROWSER.md`](app/BROWSER.md)). Plain `spice demo` exercises the installed appliance, not current source, and retains its recovery commands for preserved roots. Browser evidence is disposable. |
| Commit | Stage the change, `spice gate prove`, then `spice gate prove ACTION` or `--missing`, then `git commit`. |
| Gate change | `spice gate prove` and the commit hooks run the installed appliance's gate code, not current source. When a change edits gate logic, run each changed arm from current source against the staged tree with `spice dev xtask -- gate ARM` before relying on the installed gate. |
| Study | `spice dev xtask -- gate ARM` with `suite-seam`, `mutation-coverage`, or `test-subsumption` studies suite architecture; it is expensive and never ordinary feedback. |

The unscoped `spice dev xtask -- verify` refuses: repository-wide verification
is never routine feedback. Overlap or an advisory subsumption finding never
authorizes deleting a test; removal needs a reviewed purpose.

## Subsystem guides

- Browser checks: [`app/BROWSER.md`](app/BROWSER.md); the always-chat team
  console: [`app/CONSOLE.md`](app/CONSOLE.md); rooms, working groups, and the
  operator inbox: [`app/ROOMS.md`](app/ROOMS.md); Design rooms:
  [`app/DESIGNS.md`](app/DESIGNS.md); the task overview:
  [`app/OVERVIEW.md`](app/OVERVIEW.md); menus: [`app/MENU.md`](app/MENU.md).
- Firefox latency profiling: [`app/PROFILE.md`](app/PROFILE.md).
- The frozen report study: [`xtask/REPORT.md`](xtask/REPORT.md).

## Safe cleanup

Preview the bounded build-cache collector, then replace `--dry-run` with
`--apply` after review:

```sh
spice dev xtask -- build-cache --dry-run --max-bytes BYTES --retain-live-worktrees COUNT
```

It collects only real Cargo-tagged target directories below their owning
worktrees, and never the current roots, the hottest linked worktrees, or roots
whose Cargo locks are held; `--free-space-floor BYTES` targets free space
instead. Remove unneeded worktrees without `--force` after preserving wanted
changes. Never use `git clean -fdX`: appliance state, bootstrap skill,
artifacts, generations, dependencies, and compiler output have distinct
lifecycles.
