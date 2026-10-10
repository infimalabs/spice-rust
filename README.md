# Spice

Spice is an operations console for supervising coding agents in real Git
worktrees. The [product model](docs/product/README.md) states what Spice is and
the invariants its code keeps.

## Install

Preview the digest-bound installation plan, review it, then apply that exact
plan:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
```

An installed appliance updates from a sealed release-package directory without
a Rust toolchain or source checkout:

```sh
spice install --package /absolute/path/to/spice-release-package
spice install --package /absolute/path/to/spice-release-package --apply=DIGEST_FROM_PREVIEW
```

[INSTALL.md](INSTALL.md) owns activation, upgrade, reversal, and recovery.

## Run

```sh
spice dev xtask -- app
spice up
```

`up` returns once the resident is authenticated and prints its endpoint.
`--open` opens the browser, `--foreground` keeps the process attached, and
`spice down` drains the resident and releases its lock. Detached diagnostics
append to `<git-common>/.spice-rs/resident.log`. The resident listens on a
loopback port the kernel picks unless `--host IP` or `--port PORT` names one; a
named port keeps the URL stable across restarts.

A non-loopback listener requires `--allow-insecure-bind`. Add a URL-safe bearer
token unless the surrounding network is the intended access boundary:

```sh
spice up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

The token reaches the detached resident through a one-shot private descriptor.
It never appears in the resident's argv or environment, discovery, log URLs, or
replies; `--open` places it in the URL fragment, outside the HTTP origin.

Supervised shells may route commands through an optional configured
[RTK](https://github.com/rtk-ai/rtk) executable. An unusable rewrite reports
`RTK rewrite degraded to native` and runs the original command unchanged; each
lane's RTK history is isolated under `<git-common>/.spice-rs/rtk/`, and
`spice doctor` checks the configured executable.

## Develop

[DEVELOPMENT.md](DEVELOPMENT.md) owns the development loop, crate seams, and
cleanup; [Gate evidence](docs/product/planes/gate-evidence.md) owns commit
proof; [SHELL.md](SHELL.md) inventories every tracked shell file;
[TASTES.md](TASTES.md) owns taste curation.
