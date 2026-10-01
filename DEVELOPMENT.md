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
| Proof/browser artifacts and old `node_modules/` directories | Ignored experiment output or local tooling, not source or a managed dependency generation. | Follow the artifact owner's cleanup rules. Disposable browser experiments supply their own tooling; this repository has no npm install contract. |
| `<git-common>/.spice-rs/`; `<git-dir>/.spice-rs/`; `<worktree>/.spice-rs/` | The first is durable appliance authority; the second holds disposable per-worktree projections; a same-named worktree-content path is ordinary repository content and is never runtime authority. | Preserve common authority unless an authorized lifecycle operation owns its removal; rebuild disposable projections through the resident; treat worktree content normally. |
| `<git-common>/.spice-install/agents/skills/spice/SKILL.md` | Generated installed bootstrap skill. | Republish through the digest-bound `spice install` preview/apply cycle; do not edit the installed copy. |

## Test portfolio and overlap law

Tests overlap by purpose, not name. Use Cargo metadata for target inventory and
`spice dev cargo -- test --locked -p PACKAGE -- --list` for a package's
test binaries; add the intended target selector when narrowing to `--lib`,
`--test TARGET`, or `--doc`.

| Portfolio | Purpose and scope | Feedback invocation |
| --- | --- | --- |
| Source-local unit tests | Private invariants, parsers, reducers, and state machines. | `spice dev cargo -- test --locked -p PACKAGE --lib` or one exact test filter. |
| Package integration targets | Public seams and policy contracts. | `spice dev cargo -- test --locked -p PACKAGE --test TARGET`; package verification uses ordinary Cargo all-targets test selection. |
| Doctests | Public API examples and compile-time documentation contracts. | `spice dev cargo -- test --locked -p PACKAGE --doc`. |
| Parent-launched ignored fixtures | Process death, resource-limit, supervisor, and benchmark children. | Run the named parent; never blanket-run `--ignored`. |
| Canvas-only WASM typecheck | Rust changes whose dependency reach includes `app`; unrelated native workspace packages and the xtask bootstrap are outside this check. | `spice dev cargo -- canvas-wasm-typecheck`. |
| Canvas pure checks | Rust client reducers, message rendering, layout, and state transitions. | `spice dev cargo -- test --locked -p app --lib`, or the relevant app test target. |
| Canvas browser interaction | Actual app build, resident, and browser behavior. No permanent process-backed browser suite is registered. | Build with `spice dev xtask -- app`, then exercise the changed surface through a disposable `spice demo` or isolated resident, under the host color scheme (below). Record observations and remove the experiment. |
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

Canvas compilation remains part of staged gate selection when the source reach
requires it. Actual browser interaction evidence is disposable feedback, not a
reusable browser-shard receipt. No separate slow-test classification command or
JavaScript test package remains.

## Browser evidence follows the host system theme

Use the repository-owned `launchCanvasBrowser` helper for disposable Canvas
browser checks. It detects the host appearance and gives the headless context
the matching `prefers-color-scheme`; callers do not repeat theme setup:

```js
import { chromium } from "playwright";
import launchCanvasBrowser from "./app/browser/canvas.mjs";

const canvas = await launchCanvasBrowser(chromium);
try {
  await canvas.page.goto(canvasUrl);
  // Inspect or capture the actual Canvas surface.
} finally {
  await canvas.close();
}
```

An intentional theme-matrix check overrides `contextOptions.colorScheme`.
Ordinary screenshots and visual checks retain the system default. The returned
`colorScheme` and `colorSchemeSource` report the selected value and whether it
came from `"system"` detection or `"explicit"` configuration. This browser
preference never changes an explicit Canvas preference: a stored
`spice:canvas-theme` still wins.

```js
const lightCanvas = await launchCanvasBrowser(chromium, {
  contextOptions: { colorScheme: "light" },
});
```

## Profile Canvas latency in Firefox

Canvas exposes opt-in User Timing diagnostics without changing route parsing.
Open the ordinary Canvas URL, run
`sessionStorage.setItem("spice:canvas-trace", "1"); location.reload()` in its
developer console, and leave that tab open for the capture. A traced page
installs `window.__spiceCanvasTrace`; an ordinary page does not attach the
native input listeners or create timing entries.

Build the current browser body with `spice dev xtask -- app` and serve that
generation through the development Canvas. Then capture two short runs of the
same interaction:

1. Open `about:profiling` in Firefox, select the **Web Developer** preset, and
   start the profiler.
2. In the traced Canvas tab, run `window.__spiceCanvasTrace.clear()` in the
   developer console immediately before the scenario.
3. Perform one fixed scenario: use the same loaded board, focus the same
   composer, enter or dictate the same text, and trigger the same resident
   update. Keep the capture short enough that the entire interaction is easy to
   inspect.
4. For the loaded comparison, start
   `spice dev cargo -- check --locked -p spice` in another terminal and perform
   the scenario while that real compile is active. Stop and save the profile.
5. Wait for the command and all compiler processes to exit. Clear the Canvas
   trace, repeat the identical scenario without compilation, then stop and save
   the settled profile.
6. In each Firefox profile, use the Marker Chart and filter for
   `spice.canvas.`. Compare those markers on the Canvas content main thread
   against DOM events, task scheduling gaps, style recalculation, layout, and
   paint. Retain the profile URLs or exported profiles with the investigation
   notes.

Marker names carry only bounded structural metadata:

| Marker | Interpretation |
| --- | --- |
| `trace.start` | App source/build identity for the capture. |
| `frame.received`, `frame.decode.*` | Incoming frame size and browser-side decode/admission work. |
| `state.apply.*`, `state.publish` | Runtime mutation and the Yew publication that schedules rendering. |
| `render.canvas` | Rust/Yew virtual-DOM construction with the retained card count. |
| `layout.measure` | Mosaic DOM measurement, including card count and measured width. |
| `composer.textarea.mount`, `composer.textarea.unmount` | Native textarea instance lifetime, focus, and selection. A changed instance during one interaction is a remount. |
| `composer.input`, `composer.keydown` | Native event kind, composition state, focus, and selection without draft text. |

The browser retains at most 256 Canvas diagnostic records; completed spans use
one measure and two marks, and the oldest record is removed as the bound rolls.
`window.__spiceCanvasTrace.clear()` removes only entries owned by the current
Canvas trace. Run
`sessionStorage.removeItem("spice:canvas-trace"); location.reload()` to disable
collection.

Interpret ordering conservatively. `render.canvas` ends when Rust finishes
building Yew nodes, not when the DOM is committed or painted.
`layout.measure` covers synchronous Mosaic DOM reads and writes, but a later
paint must be established from Firefox's own markers. A DOM `beforeinput` or
`input` marker is not the start of speech recognition. The trace intentionally
has no animation-frame marker: an animation-frame callback would show callback
eligibility, not presentation.

## Bounded historical report read

`spice dev report-page REQUEST` reads one page of retained journal facts for
private report evidence through the same authenticated resident door as every
other command. It is development-scoped: it is not a board export, and its
answers are private originals, never publication payloads. `REQUEST` is a JSON
file of at most 16 KiB:

```json
{"version": 1, "upper": 797204, "kinds": ["task::added", "task::phased"]}
```

- `upper` is the inclusive study barrier. A bound beyond the resident's cursor
  refuses with `report::boundary-unavailable`.
- `kinds` must come from the report vocabulary (`wire::intent::REPORT_FACT_KINDS`);
  anything else refuses with `report::kind-outside-scope`.
- Rows are limited to tasks in public projects. Claim and lease facts are
  admitted by the task they concern, so a hidden task's leases stay hidden.
- The first answer carries a `pin`: journal schema, the resident executable's
  digest, and the residency epoch. While an answer carries `continuation`,
  request the next page with `after` set to it and the same `pin`. A
  continuation without a pin refuses with `report::pin-required`. A changed
  decoder, schema, or residency refuses with `report::generation-changed`, so
  the acquisition restarts instead of splicing two generations together.
- `command_at` optionally attaches the retained review command record at one
  position, for a case study of a single correction flow; it refuses unless
  the record lies wholly within the barrier and a public task.

Client and resident must both carry this command. A client that has it,
talking to a resident that predates it, faults with
`command::answer-undecodable`. Bringing the resident up to date is an
operator install (`spice dev xtask -- install` from a worktree containing the
change); a lane cannot run it.

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
