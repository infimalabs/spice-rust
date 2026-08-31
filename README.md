# Spice

Spice is an operations console for supervising coding agents in real Git
worktrees. The [product model](docs/product/README.md) defines portable behavior;
the [Rust implementation profile](docs/product/rust.md) fixes this appliance's
concrete authority, storage, transport, verification, and release choices.

## Install and run

Preview the digest-bound installation plan, review it, then apply that exact
plan:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
```

Installation publishes the executable, bootstrap skill, supervised-shell
entry/static/wrapper assets, and three Git hooks under the Git common directory,
then switches hook authority last. It refuses conflicting linked-worktree
selections before writing. [INSTALL.md](INSTALL.md) owns the complete
activation, upgrade, and reversal procedure;
[INSTALL_RECOVERY.md](INSTALL_RECOVERY.md) owns exceptional recovery.

Bind the exact installed appliance before repository development or operation.
A missing, symbolic, non-file, or non-executable asset is a hard stop:

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

`up` returns after owner-only discovery and authenticated bootstrap agree on the
resident generation:

```json
{"endpoint":"http://127.0.0.1:8767","epoch":"..."}
```

`--open` opens the browser. `up --foreground` keeps process, signals, endpoint
output, and terminal outcome attached. Detached diagnostics append to Git-common
`.spice-rs/resident.log`; `spice down` drains the resident, withdraws discovery,
and releases its lock. `--host IP` and `--port PORT` override the loopback
address and default port; `--port 0` selects a free port.

A non-loopback listener requires `--allow-insecure-bind`. Add a bearer token of
16–256 ASCII letters, digits, hyphens, periods, underscores, or tildes unless
the surrounding network is the intended access boundary:

```sh
"$spice_rs" up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

The initiating CLI receives the token in its own argv, then passes it to the
detached resident through a one-shot private descriptor. It is absent from the
resident child's argv and environment and from discovery, log URLs, and replies.
`--open` places it in the URL fragment, outside the HTTP origin.

Supervised shells may use an optional configured
[RTK](https://github.com/rtk-ai/rtk) executable. When that executable passes
health admission, each top-level shell command crosses a bounded `rtk rewrite
-- ...` selector. Exit 0 or 3 with nonempty output selects the rewrite; exit 1
with empty output keeps the native command.
Launch, timeout, protocol, and parsing failures report
`RTK rewrite degraded to native` and execute the original argv unchanged. RTK
is not an appliance asset. Its lane-specific history is isolated at
`<git-common>/.spice-rs/rtk/<lane>/history.db` through `RTK_DB_PATH`, and
`spice doctor` probes the exact configured executable through the same bounded
health contract without starting a resident.

Activation requires all three installed hooks. Pre-commit runs the staged
selector: recognized paths execute their affected Cargo targets and policy
arms; authority, unclassified, and out-of-workspace Rust paths refuse before
execution. There is no exhaustive fallback. Use staged selection or
`"$spice_rs" dev xtask -- verify --package PACKAGE` for bounded feedback.

## Develop

Source work completes through validation, commit, and the active harness's handoff
or landing path. Installation is a separate operator or supervisor action.

Use package-scoped feedback while work is moving:

```sh
"$spice_rs" dev cargo -- fmt --all -- --check
"$spice_rs" dev xtask -- verify --package gate
```

At the commit boundary, stage the exact change and run the selected gate:

```sh
"$spice_rs" dev cargo -- run --quiet --locked --package spice --bin spice -- \
  gate selection
```

The gate captures the index once and runs the affected Cargo targets and
canonical policy arms. Known selection, policy, build, and toolchain authority
selects explicit exhaustive reach. Unknown paths and out-of-workspace
Rust/manifest paths refuse instead of guessing coverage. The unscoped verifier
likewise refuses before launching work.

Native `spice gate pre-commit` resolves tracked policy once in an isolated
worker, runs the packaged selector, then admitted configured commands in
authored order. `pre_commit_builtins = false` runs only configured commands and
refuses an empty battery. Packaged selection, policy, tests, and reporting
consume one captured source. Configured commands receive its identity and
selected paths as metadata, run from the live repository root, and must bind
their own source bytes when required. Every child executes under bounded
process-group containment; a clean run is silent and a refusal names only
selected findings plus its private report path.

[DEVELOPMENT.md](DEVELOPMENT.md) owns crate seams, generated surfaces, the test
portfolio, feedback tiers, concurrency, and safe cache cleanup.
[SHELL.md](SHELL.md) inventories every tracked shell boundary.
[TASTES.md](TASTES.md) owns the checked taste seeds and curation law.
