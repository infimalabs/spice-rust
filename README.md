# Spice

## Source development

Ordinary source-task completion consists of validation, commit, and landing.
No appliance transition is part of that completion path.

The manual commands below are for the initial authority transition and
operator-directed recovery, not ordinary source-task completion.

The implementation-independent contract lives in the [Spice product
model](docs/product/README.md). Its [Rust implementation
profile](docs/product/rust.md) records the exact exceptions and Rust-specific
choices that this repository must preserve.

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

After the initial installation, bind the exact Git-common appliance once for
all developer task commands. A missing, symbolic, non-file, or non-executable
asset is a hard stop:

```sh
git_common=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
spice_rs="${git_common}/.spice-install/bin/spice"
if [ ! -f "$spice_rs" ] || [ -L "$spice_rs" ] || [ ! -x "$spice_rs" ]; then
  printf '%s\n' "Rust Spice appliance is unavailable: $spice_rs" >&2
  exit 1
fi
"$spice_rs" dev xtask -- app
"$spice_rs" up
```

The `up` command starts this exact executable, detached in its own process group.
It returns an endpoint-and-epoch reply after owner-only discovery and an
authenticated `/bootstrap` agree on the resident generation. `--json` uses the
typed envelope; `--open` opens the browser.

### Optional RTK companion

Supervised agent shells use [RTK](https://github.com/rtk-ai/rtk) when it is
installed, and otherwise run commands natively. RTK is a separate optional
executable, not an appliance asset. Tracked `spice-rust.toml` or Git-common
`.spice-rs/config.toml` may replace the default `rtk` executable:

```toml
[rtk]
executable = "/opt/rtk/bin/rtk"
```

Each top-level shell command crosses a bounded `rtk rewrite -- ...` selector.
Exit 0 or 3 with nonempty output selects the rewrite; exit 1 with empty output
keeps the native command. Launch, timeout, protocol, and parsing failures print
`RTK rewrite degraded to native` and execute the original argv unchanged. RTK
history is isolated at
`<git-common>/.spice-rs/rtk/<lane>/history.db` through `RTK_DB_PATH`.
`spice doctor` reports whether the exact configured executable is available but
does not start it.

```
{"endpoint":"http://127.0.0.1:8767","epoch":"..."}
```

`up --foreground` instead leaves the process, signals, streaming
`canvas endpoint:` line, and terminal outcome attached. Detached diagnostics
append to Git-common `.spice-rs/resident.log`. `spice down` drains and ends the
resident after withdrawing discovery and releasing its lock. Startup failures
return directly; a second `up` names the unchanged holder.

The resident serves the immutable HTML, JavaScript, CSS, and WASM generation
that `"$spice_rs" dev xtask -- app` publishes under ignored `dist/stage-app`; it refuses an
incomplete generation. `127.0.0.1:8767` is the no-fallback default. `--host` and
`--port` override it, while `--port 0` takes a free port.

The Rust/WASM-only browser substrate and its protocol boundary are recorded in
the [Rust implementation profile](docs/product/rust.md#canvas-and-transport).

A non-loopback listener requires explicit consent. Add a URL-safe bearer token
unless the surrounding network is the intended access boundary:

```sh
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" \
  up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

On a trusted VPN, omit `--auth-token` to use the VPN as that boundary.

When supplied, readiness uses that token, but the reply contains only endpoint
and epoch. A one-shot private descriptor keeps it out of argv, environment,
discovery, log URLs, and replies. `--open` appends it as a URL fragment, outside
the HTTP origin. Wildcard binds publish loopback; for a VPN browser, replace
`127.0.0.1` with the host's VPN address, keep the port, and append `#TOKEN` only
when authentication is enabled.

Activation requires all three installed hooks. Pre-commit runs the staged
selector: recognized paths execute their affected Cargo targets and policy
arms; authority, unclassified, and out-of-workspace Rust paths refuse before
execution. There is no exhaustive fallback. Use staged selection or
`"$spice_rs" dev xtask -- verify --package PACKAGE` for bounded feedback.

Only open two-parent resolution may reuse matching legacy verifier evidence.
Current single-parent commits and task completion never read or produce it.

The native `commit-msg` hook enforces message hygiene and exact v1
`Spice-Origin` grammar. The `reference-transaction` hook refuses a
current-branch update that would abandon commits already merged upstream.

## Scoped verification

Run package-local Clippy, tests, and doctests while work is moving with:

```sh
"$spice_rs" dev xtask -- verify --package gate
```

The unscoped verifier deliberately refuses before launching any compiler or
test process. Repository-wide verification is not a task-completion contract.
When a changed public seam crosses packages, name each affected package or run
the exact downstream target that exercises it.

`"$spice_rs" dev cargo -- run --quiet --locked --package spice --bin spice --
gate selection` is the source-development delegate for the ordinary
staged-change gate. It captures the index once, maps changed paths to canonical
tests and policy arms, and retains that evidence in one typed report. Invalid
or unclassified paths refuse before execution.

This selected evidence is the commit boundary; it never escalates to an
exhaustive repository run. Native `spice gate pre-commit` resolves tracked
policy once in an isolated worker, runs the packaged selector, then admitted
configured commands in authored order. `pre_commit_builtins = false` runs only
configured commands and refuses an empty battery. All work shares one captured
source and bounded process group; clean is silent and failures name only
selected findings plus the private report path.

The scoped package command identifies every repository-wide action and compiled
gate arm it intentionally skips. Its three Cargo children run serially with a
single build job so the feedback loop does not amplify peer compilation pressure.
Formatting remains a workspace check; run `"$spice_rs" dev cargo -- fmt --all
-- --check` at the package checkpoint. File unexpected failures outside the
changed slice as separate work instead of widening the current run.

[DEVELOPMENT.md](DEVELOPMENT.md) maps the crate ownership seams, generated
surfaces, test portfolio, validation tiers, concurrency posture, and safe
disk-cleanup rules used by development agents.

The [product model](docs/product/README.md) governs portable behavior, and the
[Rust implementation profile](docs/product/rust.md) is the current deviation
and specialization register.

Shared Cargo-cache inventory and cleanup are documented in
[DEVELOPMENT.md](DEVELOPMENT.md#disk-budget). [SHELL.md](SHELL.md) inventories
the few shell-bound entrypoints that remain.

The `gate` integration tests enforce repository contracts from stable product
inputs. Lattice conformance compares every local path dependency in
`Cargo.lock` with the acyclic graph compiled into
`gate/tests/dependency_lattice.rs`. That test's `EXPECTED` table is the source
of truth: it fails when a workspace member is added without an entry or when a
manifest adds, removes, reverses, or cycles a local dependency edge.

The product model and Rust profile replace the former port doctrine and pinned
shape audit. [TASTES.md](TASTES.md) remains executable repository truth: its
seed block and curation law are checked against the configured taste registry.
