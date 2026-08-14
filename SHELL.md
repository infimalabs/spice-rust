# Shell boundary inventory

Every shell file is either retired into a named Rust entry point or remains at
a boundary that Rust cannot cross without adding a prior binary artifact or
losing source-in-the-current-shell behavior. This is the complete inventory;
`xtask/tests/shell_inventory.rs` compares its live entries with the Git index.

## `dist/proof-image/build.sh`

Caller: Release operator, formerly documented by
`dist/proof-image/README.md`, and proof recovery diagnostics.

Disposition: Ported to Rust and deleted.

Reason: `cargo xtask proof-image build` now assembles the context, invokes
Docker, admits `identity.txt`, exercises the image, and writes the archive
sidecars.

## `dist/proof-image/load.sh`

Caller: Release operator and proof recovery diagnostics.

Disposition: Ported to Rust and deleted.

Reason: `cargo xtask proof-image load ARCHIVE` now verifies the archive
checksum and identity before loading and reinspecting it.

## `scripts/install-hooks.sh`

Caller: Repository installer operator, `README.md`, and `INSTALL.md`.

Disposition: Ported to Rust and deleted.

Reason: installed `spice install` owns one digest-bound asset and activation
plan. It verifies every linked-worktree selection, publishes the executable and
hooks, retires obsolete hook-runtime files, clears admitted worktree overrides,
switches the common path last, and records exact reversal state. The verifier
audits that receipt against the freshly built Spice binary.

## `scripts/verify.sh`

Caller: Developer documentation.

Disposition: Ported to Rust and deleted.

Reason: The script was only an alternate spelling of the already canonical
`cargo xtask verify`; callers now use that command directly.

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
packaged inner stage. A Rust child cannot replace the shell process that chose
to start it.

## `steer/hooks/static/stage.sh`

Caller: Executed by `spice run` as its one immediate child for every supervised
top-level shell command.

Disposition: Irreducible glue.

Reason: It removes the packaged routing variables, restores `SHELL` to the
user's native shell, and replaces itself with that shell. This preserves the
wrapper's child process identity across `exec` while ensuring executed scripts
and descendant shells start natively; a Rust child cannot mutate its own
process environment and then replace itself on the caller's behalf.
