# Shell boundary inventory

Every tracked shell file remains only at a boundary Rust cannot cross without
adding a prior binary artifact or losing source-in-the-current-shell behavior.
This is the complete inventory; `xtask/tests/shell_inventory.rs` compares it
with the Git index.

## Generated Git-hook launchers

`spice install` generates `pre-commit`, `commit-msg`, and
`reference-transaction` under the Git-common `.spice-install/git-hooks`. These
are installed artifacts rather than tracked shell files. Their exact forms are
shown with a representative absolute Git-common prefix. `pre-commit` first
normalizes Git and Command Line Tools environment inherited from the hook:

```sh
#!/bin/sh -eu
SPICE_SHELL_HOOK_INSTALLED_APPLIANCE=1
export SPICE_SHELL_HOOK_INSTALLED_APPLIANCE
git_exec_path=${GIT_EXEC_PATH-}
case ${PATH-} in
  "$git_exec_path":*) PATH=${PATH#"$git_exec_path":}; export PATH ;;
esac
unset GIT_DIR GIT_EXEC_PATH GIT_PREFIX
case $git_exec_path in
  /Library/Developer/CommandLineTools/*)
    [ "${CPATH-}" != /usr/local/include ] || unset CPATH
    [ "${LIBRARY_PATH-}" != /usr/local/lib ] || unset LIBRARY_PATH
    case ${SDKROOT-} in /Library/Developer/CommandLineTools/*) unset SDKROOT ;; esac
    case ${MANPATH-} in *Library/Developer/CommandLineTools*) unset MANPATH ;; esac
    ;;
esac
exec '/absolute/git-common/.spice-install/bin/spice' gate pre-commit "$@"
```

The other launchers are exactly two lines:

```sh
#!/bin/sh -eu
exec '/absolute/git-common/.spice-install/bin/spice' gate commit-msg "$@"
```

```sh
#!/bin/sh -eu
exec '/absolute/git-common/.spice-install/bin/spice' gate reference-transaction "$@"
```

Each final `exec` preserves Spice's exit status, stdout, and stderr and forwards
Git's exact argv. `pre-commit` receives no operands; its Rust dispatcher captures
the staged index. `commit-msg` receives exactly the message-file path. Both
inherit standard input without reading it. `reference-transaction` receives
exactly one state operand and preserves Git's update records on standard input;
the Rust gate consumes them only for `prepared`, while `committed` and `aborted`
are inert. No launcher reads policy or performs fallback routing.

The short `#!/bin/sh -eu` line names a stable system interpreter and makes a
failed expansion or command fatal. The shell performs no policy: `pre-commit`
only removes inherited environment that would select a different Git/toolchain
context, and every launcher quotes and `exec`s the explicit absolute appliance
path. Putting that long path in the shebang would depend on kernel shebang length
and argument-parsing rules, could not portably encode the `gate HOOK` subcommand,
and would leave repository relocation hidden inside kernel launch failure. The
generated body makes that absolute, non-portable identity inspectable and
repairable through a fresh digest-bound install plan.

The tracked `.githooks` files are development launchers, not installed assets.
They resolve the Git common directory and dispatch directly to its installed
Spice. They never invoke Cargo or `xtask`; current-source policy is the separate
configured `spice dev cargo -- run --quiet --locked --package spice --bin spice
-- gate selection` leaf.

## `dist/proof-image/cargo-wrapper.sh`

Caller: `dist/proof-image/Dockerfile`, installed as `/opt/spice/bin/cargo`
inside the sealed Linux image.

Disposition: Irreducible glue.

Reason: It creates `/cargo-target/tmp` after the proof boundary mounts an empty
target tmpfs, then `exec`s the image's real Cargo. A Rust wrapper would require
a prebuilt `linux/amd64` binary before the image that establishes and records
that binary's toolchain exists.

## `dist/proof-runner/build.sh`

Caller: Native arm64 macOS release operator via
`dist/proof-runner/README.md`.

Disposition: Irreducible glue.

Reason: It is the transparent bootstrap recipe that runs the unsealed runner
to derive compile-time pins, then compiles and exercises the sealed runner.
Moving that derivation into the runner being sealed would make the target
author its own trust inputs; introducing a second prebuilt Rust sealer would
only move the unsealed bootstrap and add another binary identity.

## `steer/hooks/entry.sh`

Caller: The installed shell startup hook embedded by `steer::hook` and
published by `spice::install`; supervised agent launches set this executable as
their outer `SHELL`.

Disposition: Irreducible glue.

Reason: It receives the shell tool's complete argument vector before the native
command shell exists, then replaces itself with `spice run` and the separately
packaged inner stage. `spice run` first gives the complete top-level command to
the optional configured RTK selector; every unusable answer reports a
degradation and runs the original argv unchanged. A Rust child cannot replace
the shell process that chose to start it.

## `steer/hooks/static/stage.sh`

Caller: Executed by `spice run` as its one immediate child for every supervised
top-level shell command.

Disposition: Irreducible glue.

Reason: It removes the packaged routing variables, restores `SHELL` to the
user's native shell, and replaces itself with that shell. This preserves the
wrapper's child process identity across `exec` while ensuring executed scripts
and descendant shells start natively; a Rust child cannot mutate its own
process environment and then replace itself on the caller's behalf.

## `steer/hooks/wrappers/spice`

Caller: Agent command lookup through the installed wrapper directory prepended
to supervised launch `PATH` when `[wrappers] spice = "appliance"` is configured.

Disposition: Irreducible glue.

Reason: The shell must resolve the ambient `spice` word before the Rust
appliance can run. The resident validates and exports the exact Git-common
appliance path; this extensionless wrapper performs no discovery or policy and
replaces itself with that typed executable while preserving the caller's exact
arguments and process status.
