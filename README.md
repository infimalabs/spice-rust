# Spice

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
ordinary hook. `cargo xtask verify` alone owns the complete 13-action graph and
all 27 policy arms.

Every verifier child runs in a bounded process group. The scheduler re-observes
peer Cargo pressure after its serialized test prebuild, caps local workers and
Cargo jobs, and fails closed to one of each when pressure cannot be read. The
test phase is bounded at 180 seconds and the supervising command at 300 seconds;
failure or interruption cancels and reaps siblings before returning. See
[DEVELOPMENT.md](DEVELOPMENT.md) and
[gate/THROUGHPUT_AUDIT.md](gate/THROUGHPUT_AUDIT.md) for execution and measured
fleet details.

The native `commit-msg` hook enforces message hygiene and exact v1
`Spice-Origin` grammar. The `reference-transaction` hook refuses a
current-branch update that would abandon commits already merged upstream.

## Workspace verification

Run the same verification outside a commit, and before handing work to
another agent, with:

```sh
cargo xtask verify
```

The command discovers the repository from the current directory and runs the
one canonical verifier; there is no wrapper or second completion suite. It
prebuilds workspace tests once, packs targets deterministically into bounded
shards, and launches no more lanes than the scheduler grants. Each shard owns a
separate publication path and bounded process group.

`cargo run --quiet --locked --package spice --bin spice -- gate selection` is
the source-development delegate for the ordinary staged-change gate. It builds
and invokes one public Spice command, captures the index once, maps every changed
path to canonical Cargo test targets and focused policy arms, and retains the
path-to-check evidence in one typed report. Known backend, frontend,
documentation, and configuration paths select only their local evidence. A
selector/toolchain authority path, an unclassified path, or an out-of-workspace
Rust/manifest path returns a deterministic path-specific refusal before any
selected policy or test execution.

This selection is not a completion verifier; `cargo xtask verify` remains that
surface. The native `spice gate pre-commit` command resolves tracked pre-commit
policy once in one isolated worker. It runs
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

For fast feedback while changing one Cargo package, scope Clippy, all-target
tests, and doctests to that package:

```sh
cargo xtask verify --package gate
```

The scoped command identifies every full-verifier action and compiled gate arm
it skips. It is feedback only; run the zero-argument command before completing
or handing off work. Formatting remains a workspace check; run
`cargo fmt --all -- --check` at the package checkpoint.

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
