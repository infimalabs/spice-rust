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

Within a supervised repository shell, tracked wrapper policy resolves ambient
`spice` to the exact Git-common appliance. A missing, symbolic, non-file, or
non-executable asset is a hard stop before the command runs:

```sh
spice dev xtask -- app
spice up
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
spice up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

The initiating CLI receives the token in its own argv, then passes it to the
detached resident through a one-shot private descriptor. It is absent from the
resident child's argv and environment and from discovery, log URLs, and replies.
`--open` places it in the URL fragment, outside the HTTP origin.

Supervised shells may use an optional configured
[RTK](https://github.com/rtk-ai/rtk) executable. When that executable can be
launched, the installed shell hooks route each top-level command across a
bounded `rtk rewrite -- ...` selector. Any nonempty, parseable, faithful output
selects the rewrite regardless of selector status; exit 1 with empty output
keeps the native command without a diagnostic. Other empty answers, launch
failures, timeouts, and unusable rewrites report
`RTK rewrite degraded to native` and execute the original argv unchanged. RTK
is not an appliance asset. Its lane-specific history is isolated at
`<git-common>/.spice-rs/rtk/<lane>/history.db` through `RTK_DB_PATH`, and
`spice doctor` observes the exact configured executable through one bounded
launch check without starting a resident. Version and rewrite statuses are
advisory; rewrite validation belongs to the command that will execute it.

Activation requires all three installed hooks. Pre-commit runs the staged
selector: recognized paths execute their affected Cargo targets and policy
arms; authority, unclassified, and out-of-workspace Rust paths refuse before
execution. There is no exhaustive fallback. Use staged selection or
`spice dev xtask -- verify --package PACKAGE` for bounded feedback.

## Develop

Source work completes through validation, commit, and the active harness's handoff
or landing path. Installation is a separate operator or supervisor action.

Use package-scoped feedback while work is moving:

```sh
spice dev cargo -- fmt --all -- --check
spice dev xtask -- verify --package gate
```

At the commit boundary, stage the exact change and inspect its proof receipts:

```sh
spice gate prove
```

Inspection never runs missing work. Follow its exact remediation with
`spice gate prove ACTION`, or generate all current misses with
`spice gate prove --missing`, then use the normal `git commit` path. Only the
proof runner publishes trusted receipts; ad hoc Cargo, Canvas, or shell output
is useful feedback but is not reusable commit evidence. A trust refusal remains
an operator boundary and is never bypassed by proof generation.

Canvas proof is `canvas:build` plus one `canvas:shard:<name>` receipt per current
registry shard. Release 0.31.0's `BROWSER-1kKSWhZk browser coverage complete`
marker and `--spice-coverage-receipt` writer are retired; see
[Gate evidence](docs/product/planes/gate-evidence.md) for the receipt and cache
contract.

Native `spice gate pre-commit` resolves tracked policy and fingerprint authority
once in an isolated worker, restores the canonical staged selection from the
manifest, verifies its exact receipts, then runs live and admitted configured
checks. `pre_commit_builtins = false` runs only configured commands and refuses
an empty battery. Selection, policy, tests, and reporting consume one captured
source. Configured commands receive its identity and selected paths as metadata,
run from the live repository root, and must bind their own source bytes when
required. Every child executes under bounded process-group containment; a clean
run is silent and a refusal names only selected findings plus its private report
path.

[DEVELOPMENT.md](DEVELOPMENT.md) owns crate seams, generated surfaces, the test
portfolio, feedback tiers, concurrency, and safe cache cleanup.
[SHELL.md](SHELL.md) inventories every tracked shell boundary.
[TASTES.md](TASTES.md) owns the checked taste seeds and curation law.
