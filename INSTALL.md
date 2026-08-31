# Installing Spice

Installation is a digest-bound authority change. Preview observes the complete
plan without writing. Applying that exact preview publishes the appliance and
selects its repository-wide Git hooks. Neither operation stops, drains,
imports, or deletes foreign processes or state.

## Development task boundary

Ordinary source work ends with a validated, landed commit. Staged selection and
package-scoped checks do not compare the worktree build with the installed
appliance, and source-task completion contains no installation step. The
supervising loop may publish landed source through `"$SPICE_BIN" dev xtask --
install`; manual installation, repair, and relocation require explicit operator
authorization.

Every apply remains bound to the exact digest printed by its preview. Run the
commands below from the root of the worktree being installed, and keep paths
explicit until activation establishes the bare `spice` identity.

## Build and preview

```sh
cargo build --locked -p spice || exit
target/debug/spice install || exit
```

Build immediately before preview so the planner and source agree.
`cargo run --locked -p spice -- install` is the rebuild-and-preview equivalent.

Preview prints the complete digest-bound plan and changes nothing. The plan
publishes the executable, bootstrap skill, supervised-shell entry/static/wrapper
assets, and three Git hooks under the Git common directory's `.spice-install`,
clears admitted worktree-local hook overrides, and switches the common
`core.hooksPath` last.

## Operator-authorized authority switch

Stop unless the operator has explicitly authorized repository-wide activation.
The operation has two postconditions: every effective `core.hooksPath` names
the installed hooks, and the first `spice` on the supervised search path is
`$SPICE_BIN`.

Inspect every declaring scope first. A conflicting or foreign hook selection
refuses without writing. After authorization, replace each selected foreign
value with the recognized tracked `.githooks` value, preview again, and apply
only the newly printed digest. Change a worktree-local scope only when that
worktree already declares an override named by the refusal; never create
worktree overrides merely to standardize linked worktrees:

```sh
SPICE_REPO=/absolute/path/to/this/worktree
SPICE_COMMON=$(git -C "$SPICE_REPO" rev-parse --path-format=absolute --git-common-dir) || exit
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
cd "$SPICE_REPO" || exit
git config --local core.hooksPath .githooks || exit
```

When preview names an existing worktree-local override, normalize that exact
worktree before producing the final preview. Repeat this pair for every named
override; do not run it when preview names none:

```sh
SPICE_OVERRIDE_WORKTREE=/absolute/path/to/the/worktree-named-by-preview
git -C "$SPICE_OVERRIDE_WORKTREE" config --worktree core.hooksPath .githooks || exit
```

Now create the final preview, apply its exact digest, and verify the
repository-wide authority unconditionally:

```sh
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
SPICE_HOOKS="$SPICE_COMMON/.spice-install/git-hooks"
test -x "$SPICE_BIN" || exit
/usr/bin/cmp target/debug/spice "$SPICE_BIN" || exit
test "$(git config --local --get core.hooksPath)" = "$SPICE_HOOKS" || exit
```

After apply, verify every worktree that was normalized. Repeat the same pair
with each exact path; do not run it when no override was normalized:

```sh
SPICE_OVERRIDE_WORKTREE=/absolute/path/to/the-worktree-normalized-above
test "$(git -C "$SPICE_OVERRIDE_WORKTREE" config --get core.hooksPath)" = "$SPICE_HOOKS" || exit
```

Give the operator shell the same executable identity and verify it:

```sh
SPICE_BIN_DIR=${SPICE_BIN%/spice}
export PATH="$SPICE_BIN_DIR:$PATH"
hash -r
test "$(command -v spice)" = "$SPICE_BIN" || exit
spice --json doctor || exit
```

## Hook authority

Preview checks every linked worktree before writing. An absent selection, or
only repeated identical values naming tracked `.githooks`, the target installed
path, or the intact receipt's prior installed path, is admitted. Conflicting or
foreign values refuse. Apply clears admitted worktree overrides and switches
the common path last. One repository has one effective hook authority.

The installed `/bin/sh` launchers end by `exec`ing the absolute Git-common Spice
while preserving Git's argv and I/O. `pre-commit` first normalizes inherited Git
and toolchain environment; the other two launchers are exactly two lines.
[SHELL.md](SHELL.md#generated-git-hook-launchers) records the exact ABI. Recovery
never searches `PATH` or substitutes Cargo, `xtask`, or a sidecar;
[INSTALL_RECOVERY.md](INSTALL_RECOVERY.md) owns the exceptional workflows.

## Start and operate

```sh
"$SPICE_BIN" dev xtask -- app
"$SPICE_BIN" up
"$SPICE_BIN" --json journal verify
"$SPICE_BIN" --json board
"$SPICE_BIN" down
```

`up` returns after authenticated readiness and never falls back from its chosen
endpoint. Detached diagnostics are in
`<git-common>/.spice-rs/resident.log`; README covers foreground, port, browser,
and non-loopback options. `down` stops only the resident named by this
appliance's discovery and lock authority.

## Upgrade

Use one freshly built executable for preview and apply:

```sh
"$SPICE_BIN" dev cargo -- build --locked -p spice || exit
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

Do not unapply first. An intact completed receipt seeds an in-place refresh;
receipt edits, asset drift, and interrupted operations refuse. Refresh publishes
assets, activates hooks last, and retains each operation's original pre-install
image while binding the installed image it supersedes. Ordinary reversal
therefore restores the filesystem observed by the first install and publishes
the current terminal receipt. Only migration from the admitted version-7
receipt carries a compressed rollback snapshot; reversing that migration
restores the exact pre-migration assets and version-7 receipt. Snapshot blobs
remain inert unless the receipt authenticates them.

The supervising shortcut also restores the resident; `--no-resident` omits that
lifecycle:

```sh
"$SPICE_BIN" dev xtask -- install --port PORT
"$SPICE_BIN" dev xtask -- install -- --host 0.0.0.0 --allow-insecure-bind
```

Arguments after `--` are forwarded verbatim to `spice up`. Manual apply accepts
only its preview digest. Every linked worktree observes one Git-common upgrade,
and the installed pre-commit worker verifies the receipt, assets, and exact
binary before selected-source verification.

## Ownership and reversal

Preview changes nothing. Apply changes only receipt-owned assets and hook
authority; it does not delete foreign processes or state. Unapply follows the
receipt's exact reversal plan:

```sh
"$SPICE_BIN" install --unapply
```

Deleting `<git-common>/.spice-rs`, `<git-dir>/.spice-rs`,
`<git-common>/.spice-install`, or foreign state is a separate destructive act
requiring explicit authorization and preserved evidence. Never use pattern-wide
process kills that can match a live peer worktree.
