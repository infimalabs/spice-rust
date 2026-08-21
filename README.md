# Spice

## Source development

Ordinary source-task completion consists of validation, commit, and landing.
No appliance transition is part of that completion path.

The manual commands below are for the initial authority transition and
operator-directed recovery, not ordinary source-task completion.

## Install and start

Spice is this repository's installed appliance. Preview its digest-bound plan,
then apply that exact filesystem and repository-wide hook authority:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
```

Follow [INSTALL.md](INSTALL.md#operator-authorized-authority-switch) for the
operator-authorized switch. Preview is read-only. Apply first publishes the
executable, skill, and three hooks under the Git common directory's
`.spice-install`; after validating every linked worktree, it changes the common
hook path last. Admission permits an absent selection, or one repeated effective
value naming tracked `.githooks`, the target installed path, or the intact
receipt's prior installed path. Conflicting or foreign values refuse before any
write. Admitted worktree overrides are cleared so every linked worktree uses the
same absolute two-line launchers.

The receipt records exact reversal values. Digest-bound unapply restores the
prior appliance and receipt from one compressed snapshot and removes the new
generation without leaving runnable sidecars. The switch gives Rust the bare
`spice` command and common hooks; predecessor process/state retirement remains a
separate operator decision.

Build this lane's Canvas app, then start Spice with:

```sh
cargo xtask app
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" up
```

That command starts this exact executable, detached in its own process group.
It returns an endpoint-and-epoch reply after owner-only discovery and an
authenticated `/bootstrap` agree on the resident generation. `--json` uses the
typed envelope; `--open` opens the browser.

```
{"endpoint":"http://127.0.0.1:8767","epoch":"..."}
```

`up --foreground` instead leaves the process, signals, streaming
`canvas endpoint:` line, and terminal outcome attached. Detached diagnostics
append to Git-common `.spice-rs/resident.log`. `spice down` drains and ends the
resident after withdrawing discovery and releasing its lock. Startup failures
return directly; a second `up` names the unchanged holder.

The resident serves the immutable HTML, JavaScript, CSS, and WASM generation
that `cargo xtask app` publishes under ignored `dist/stage-app`; it refuses an
incomplete generation. `127.0.0.1:8767` is the no-fallback default. `--host` and
`--port` override it, while `--port 0` takes a free port.

The accepted browser-substrate decision and its conditional migration sequence
are recorded in [app/SUBSTRATE.md](app/SUBSTRATE.md).

A non-loopback listener requires explicit consent and a URL-safe bearer token:

```sh
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" \
  up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

Readiness uses that token, but the reply contains only endpoint and epoch. A
one-shot private descriptor keeps it out of argv, environment, discovery, log
URLs, and replies. `--open` appends it as a URL fragment, outside the HTTP
origin. Wildcard binds publish loopback; for a VPN browser, replace
`127.0.0.1` with the host's VPN address, keep the port, and append `#TOKEN`.

Activation refuses unless all three installed hooks are executable. Pre-commit
runs the ordinary staged selector: recognized paths execute only their affected
Cargo targets and local policy arms. A selector/toolchain authority path, an
unclassified path, or a Rust/manifest path outside the workspace refuses with
that path before policy or tests run. There is no exhaustive fallback in the
ordinary hook or allocator-owned merge resolution. Bare `cargo xtask verify`
is retired and refuses before starting work; use staged selection or
`cargo xtask verify --package PACKAGE` for bounded feedback.

Legacy verifier bundles already stored by earlier releases remain exact to
their producing worktree and selected-source identity. Only an open two-parent
resolution may read both parent archives for forward compatibility: matching
per-arm authorities can be reused, while resolution-only or protected-authority
changes execute their selected checks normally. Current single-parent commits
and task completion never read or produce these archives.

The native `commit-msg` hook enforces message hygiene and exact v1
`Spice-Origin` grammar. The `reference-transaction` hook refuses a
current-branch update that would abandon commits already merged upstream.

## Scoped verification

Run package-local Clippy, tests, and doctests while work is moving with:

```sh
cargo xtask verify --package gate
```

Bare `cargo xtask verify` deliberately refuses before launching any compiler or
test process. Repository-wide verification is not a task-completion contract.
When a changed public seam crosses packages, name each affected package or run
the exact downstream target that exercises it.

`cargo run --quiet --locked --package spice --bin spice -- gate selection` is
the source-development delegate for the ordinary staged-change gate. It builds
and invokes one public Spice command, captures the index once, maps every changed
path to canonical Cargo test targets and focused policy arms, and retains the
path-to-check evidence in one typed report. Known backend, frontend,
documentation, and configuration paths select only their local evidence. A
selector/toolchain authority path, an unclassified path, or an out-of-workspace
Rust/manifest path returns a deterministic path-specific refusal before any
selected policy or test execution.

This selected evidence is the commit boundary; it never escalates to an
exhaustive repository run. The native `spice gate pre-commit` command resolves
tracked pre-commit policy once in one isolated worker. It runs
the packaged selected-change gate exactly once by default, followed by admitted
configured commands in authored order; `pre_commit_builtins = false` runs only
configured commands and refuses an empty effective battery. Packaged and
configured work share the captured source and the fixed 180-second envelope,
including a 164-second worker allowance and cleanup reserves. A clean worker is
silent; a refusal names only selected failures plus a private canonical report
path. The supervisor owns the worker's complete process group, including Cargo,
rustc, Trunk, parsers, and descendants. Every tracked hook dispatches directly
to installed Spice; the authored source-development command above reaches
`gate selection` and never redispatches the pre-commit dispatcher.

The scoped package command identifies every repository-wide action and compiled
gate arm it intentionally skips. Its three Cargo children run serially with a
single build job so the feedback loop does not amplify peer compilation pressure.
Formatting remains a workspace check; run
`cargo fmt --all -- --check` at the package checkpoint. File unexpected failures
outside the changed slice as separate work instead of widening the current run.

[DEVELOPMENT.md](DEVELOPMENT.md) maps the crate ownership seams, generated
surfaces, complete test portfolio, validation tiers, measured concurrency
limits, and safe disk-cleanup rules used by development agents.

[SHAPE_CONFORMANCE.md](SHAPE_CONFORMANCE.md) governs the Rust port's matches,
intentional deviations, resolved risk dispositions, and current self-hosting GO
decision against the authoritative product shape.

## Cargo build cache

Cargo's configured `build_directory` may point every worktree at a shared
machine cache shaped as `CargoBuild/<bucket>/<workspace-hash>`. Inventory it
without writes with:

```sh
cargo xtask build-cache --dry-run --max-bytes 40GiB --retain-live-worktrees 2
```

Apply the same deterministic oldest-first plan with `--apply`, or drive by
free space instead of total cache size with `--free-space-floor 100GiB`. The
collector retains the current worktree and the configured number of hottest
linked worktree roots, skips any root with a held `.cargo-build-lock`, and never
manages `CARGO_HOME`, `CARGO_TARGET_DIR`, or sccache.

[SHELL.md](SHELL.md) inventories the few shell-bound entrypoints that remain
and every orchestration script retired into these Rust commands.

The `gate` integration tests enforce repository contracts from stable product
inputs. Lattice conformance compares every local path dependency in
`Cargo.lock` with the acyclic graph compiled into
`gate/tests/dependency_lattice.rs`. That test's `EXPECTED` table is the source
of truth: it fails when a workspace member is added without an entry or when a
manifest adds, removes, reverses, or cycles a local dependency edge.

Doctrine conformance holds [DOCTRINE.md](DOCTRINE.md) to its compiled section,
kernel, and decision-record contract. It requires every invariant to be stated
rather than merely named, every decision to carry rationale and explicit
deletions, settled language throughout, and every relative record link to
resolve on disk. [TASTES.md](TASTES.md) is the companion record that governs
curated preferences.
