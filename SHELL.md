# Shell boundary inventory

A tracked shell file exists only at a boundary Rust cannot cross without a
prior binary artifact or without losing source-in-the-current-shell behavior.
This is the complete inventory.

## Generated Git-hook launchers

`spice install` generates `pre-commit`, `commit-msg`, and
`reference-transaction` under the Git-common `.spice-install/git-hooks` from
[`spice/src/install/git_hooks.rs`](spice/src/install/git_hooks.rs). They are
installed artifacts, not tracked files. Each is a `#!/bin/sh -eu` script whose
final line `exec`s the absolute Git-common appliance's `gate HOOK` leaf,
forwarding Git's argv, standard input, output, and exit status unchanged.
`pre-commit` first strips Git- and Command-Line-Tools-injected environment that
would select a different Git or toolchain context. No launcher reads policy or
routes anywhere else.

The absolute path is in the body rather than the shebang because a shebang
depends on kernel length and argument-splitting rules, cannot portably carry
the `gate HOOK` subcommand, and would hide a repository move inside a kernel
launch failure. In the body, the non-portable identity stays inspectable and is
repaired by a fresh digest-bound install.

## `.githooks/{pre-commit,commit-msg,reference-transaction}`

Development launchers Git runs when the tracked hook path is selected. Each
resolves the Git common directory and `exec`s the installed appliance's
matching `gate` leaf. They never build: `reference-transaction` runs under Git's
shared reference lock.

## `dist/proof-image/cargo-wrapper.sh`

Installed as `/opt/spice/bin/cargo` in the sealed Linux proof image. It creates
`/cargo-target/tmp` on the empty target tmpfs and `exec`s the real Cargo. A Rust
wrapper would need a prebuilt `linux/amd64` binary before the image that
records its toolchain exists.

## `dist/proof-runner/build.sh`

The macOS release operator's bootstrap that runs the unsealed runner to derive
compile-time pins, then compiles and exercises the sealed runner. Moving that
derivation into the runner would let the target author its own trust inputs.

## `steer/hooks/entry.sh`

Supervised launches set this as the outer `SHELL`. It receives the shell tool's
complete argument vector before any native shell exists and replaces itself
with `spice run` and the inner stage. A Rust child cannot replace the shell
process that chose to start it.

## `steer/hooks/shell/.zshenv` and `steer/hooks/shell/bash_env`

Startup files selected through `ZDOTDIR` and `BASH_ENV` for agent tools that
start zsh or bash directly instead of honoring `SHELL`. While the shell still
holds the complete execution string, each re-enters `spice run` with the inner
stage once; an interactive shell clears both selectors and starts normally.
Only a file the shell sources itself sees that string.

## `steer/hooks/static/stage.sh`

The one immediate child of `spice run` for every supervised command. It removes
the routing variables, restores `SHELL` to the user's native shell, and `exec`s
that shell, so the wrapper's child identity survives while scripts and
descendant shells start natively. A Rust child cannot change its own
environment and then replace itself on the caller's behalf.

## `steer/hooks/wrappers/spice`

Prepended to supervised `PATH` when `[wrappers] spice = "appliance"` is
configured. The shell must resolve the ambient `spice` word before Rust runs;
this script `exec`s the exact appliance the resident validated, with the
caller's arguments, and performs no discovery or policy.
