# Installing Spice

Installation is a digest-bound authority change. Preview privately resolves the
complete receipt-owned plan without writing and prints its ordered byte-free
public stage summary with a canonical digest. Applying that exact digest
publishes the appliance and selects its repository-wide Git hooks. Neither
operation stops, drains, imports, or deletes foreign processes or state.

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
cargo build --locked --target-dir target -p spice || exit
target/debug/spice install || exit
```

Build immediately before preview so the planner and source agree.
`cargo run --locked --target-dir target -p spice -- install` is the
rebuild-and-preview equivalent.

Preview prints the ordered byte-free public stage summary and canonical digest and
changes nothing. The private digest-bound plan publishes the executable,
bootstrap skill, supervised-shell entry/static/wrapper assets, and three Git
hooks under the Git common directory's `.spice-install`, clears admitted
worktree-local hook overrides, and switches the common `core.hooksPath` last.

## Operator-authorized authority switch

Stop unless the operator has explicitly authorized repository-wide activation.
The operation has two postconditions: every effective `core.hooksPath` names
the installed hooks, and the first `spice` on the supervised search path is the
admitted installed wrapper, whose selected route executes `$SPICE_BIN`. The
binary itself remains a separate exact path and never enters supervised
`PATH`.

Inspect every declaring scope even when preview succeeds. Repeated identical
recognized values from several scopes are admissible at preview, but apply
replaces only the repository-local value; a retained system, global, or command
value would then make the next preview observe a mixed selection. Require those
higher scopes to contribute no value. Preserve admitted absence and repeated
recognized values in common-local and worktree-local scope exactly for reversal;
replace either only when preview refuses its foreign selection.

Start with observation. Exit status 1 from either `--get-all` means the key is
absent. The unscoped command exposes every selected origin; the local command
isolates the repository's common-local declaration:

```sh
SPICE_REPO=/absolute/path/to/this/worktree
SPICE_COMMON=$(/usr/bin/git -C "$SPICE_REPO" rev-parse --path-format=absolute --git-common-dir) || exit
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
SPICE_HOOKS="$SPICE_COMMON/.spice-install/git-hooks"
cd "$SPICE_REPO" || exit
if /usr/bin/git config --show-origin --show-scope --get-all core.hooksPath; then
  :
else
  spice_config_status=$?
  test "$spice_config_status" -eq 1 || exit "$spice_config_status"
fi
if /usr/bin/git config --local --show-origin --get-all core.hooksPath; then
  :
else
  spice_config_status=$?
  test "$spice_config_status" -eq 1 || exit "$spice_config_status"
fi
```

Resolve every selected system, global, or command-scope declaration in its own
authority so it contributes no value; do not retain it after rewriting it to
`.githooks`, because apply selects `$SPICE_HOOKS` in repository-local scope.
Only when independent inspection locates the refused foreign value in the
common-local scope, replace that scope:

```sh
/usr/bin/git config --local --replace-all core.hooksPath .githooks || exit
```

Distinct worktree scopes exist only when common-local
`extensions.worktreeConfig`, after trailing ASCII whitespace is removed, is
`true` case-insensitively. No other Git boolean spelling enables this branch.
In that case, enumerate the paths printed by `/usr/bin/git worktree list
--porcelain` and repeat the inner inspection for each one.
The inspection treats an absent flag or override as absence and propagates every
other Git failure; it does not mutate any scope:

```sh
if spice_worktree_config=$(/usr/bin/git config --local --get extensions.worktreeConfig); then
  spice_ascii_tab=$(printf '\011')
  spice_ascii_cr=$(printf '\015')
  spice_ascii_vtab=$(printf '\013')
  spice_ascii_formfeed=$(printf '\014')
  while :; do
    case "$spice_worktree_config" in
      *' '|*"$spice_ascii_tab"|*"$spice_ascii_cr"|*"$spice_ascii_vtab"|*"$spice_ascii_formfeed")
        spice_worktree_config=${spice_worktree_config%?}
        ;;
      *) break ;;
    esac
  done
  case "$spice_worktree_config" in
    [Tt][Rr][Uu][Ee])
      SPICE_INSPECT_WORKTREE=/absolute/path/to/an/inspected/worktree
      if /usr/bin/git -C "$SPICE_INSPECT_WORKTREE" config --worktree --show-origin --get-all core.hooksPath; then
        :
      else
        spice_config_status=$?
        test "$spice_config_status" -eq 1 || exit "$spice_config_status"
      fi
      ;;
  esac
else
  spice_config_status=$?
  test "$spice_config_status" -eq 1 || exit "$spice_config_status"
fi
```

Replace a worktree scope with `.githooks` only when that inspection locates the
exact refused foreign value in that scope. Leave an absent scope, repeated
identical `.githooks` values, or repeated identical `$SPICE_HOOKS` values
untouched; never create an override merely to standardize linked worktrees:

```sh
SPICE_OVERRIDE_WORKTREE=/absolute/path/to/the-worktree-with-the-refused-value
/usr/bin/git -C "$SPICE_OVERRIDE_WORKTREE" config --worktree --replace-all core.hooksPath .githooks || exit
```

After any deliberate scope change, run preview again and apply only its newly
printed digest. When the initial preview succeeded and inspection changed no
scope, apply its digest directly.

Now create the final preview, apply its exact digest, and verify the
repository-wide authority unconditionally:

```sh
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
SPICE_HOOKS="$SPICE_COMMON/.spice-install/git-hooks"
test -x "$SPICE_BIN" || exit
/usr/bin/cmp target/debug/spice "$SPICE_BIN" || exit
test "$(/usr/bin/git config --local --get core.hooksPath)" = "$SPICE_HOOKS" || exit
```

After apply, verify every worktree that was normalized. Repeat the same pair
with each exact path; do not run it when no override was normalized:

```sh
SPICE_OVERRIDE_WORKTREE=/absolute/path/to/the-worktree-normalized-above
test "$(/usr/bin/git -C "$SPICE_OVERRIDE_WORKTREE" config --get core.hooksPath)" = "$SPICE_HOOKS" || exit
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

An installed appliance can preview and apply an explicitly materialized sealed
release package without Cargo, rustc, `xtask`, or a Spice source checkout:

```sh
SPICE_PACKAGE=/absolute/path/to/spice-release-package
"$SPICE_BIN" install --package "$SPICE_PACKAGE" || exit
"$SPICE_BIN" install --package "$SPICE_PACKAGE" --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

The package directory is the exact regular-file closure published by the
release pipeline. Before either command changes repository authority, the
installed appliance verifies the package index, two-target manifest, payloads,
proof receipts, artifact identities, modes, and current platform. It stages
only the admitted package document, selected payload and executable, and shared
Canvas files in one private production-shaped tree. The selected executable
revalidates those staged bindings before deriving and applying the existing
receipt-owned transaction with its embedded hooks and skill. Package acquisition
never changes the `--unapply` path.

Apply leaves a running resident untouched. When a restart is intended, stop and
start through this repository's installed appliance so discovery and lock
authority select the exact resident:

```sh
"$SPICE_BIN" down || exit
"$SPICE_BIN" up
```

The source-workspace refresh remains available to appliance developers.
Publish the source-bound Canvas generation, then use one freshly built
executable for preview and apply:

```sh
"$SPICE_BIN" dev xtask -- app || exit
"$SPICE_BIN" dev cargo -- build --locked --target-dir target -p spice || exit
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

Do not unapply first. An intact completed receipt seeds an in-place refresh;
receipt edits, fixed-asset drift, and interrupted operations refuse. Binary-only
byte or mode drift beneath the recorded parent is the sole digest-bound
reconciliation input. Refresh publishes assets, activates hooks last, and
retains each operation's original pre-install image while binding the installed
image it supersedes. Ordinary reversal therefore restores the filesystem
observed by the first install and publishes the current terminal receipt. Only
migration from the admitted version-7 receipt carries a compressed rollback
snapshot; reversing that migration restores the exact pre-migration assets and
version-7 receipt. Snapshot blobs remain inert unless the receipt authenticates
them.

The manual refresh above is complete only when no resident is published. It does
not replace a running resident's in-memory generation. When one is running, use
the supervising shortcut: it stops the exact published resident before refresh
and starts the newly installed executable afterward. `--no-resident` explicitly
omits that lifecycle:

```sh
"$SPICE_BIN" dev xtask -- install --port PORT
"$SPICE_BIN" dev xtask -- install -- --host 0.0.0.0 --allow-insecure-bind
```

Arguments after `--` are forwarded verbatim to `spice up`. The shortcut is the
operator's: run from a lane the repository seats, it refuses with
`resident::operator-install-only` before building anything, because every
seated lane runs the appliance it would replace. A lane may still refresh a
scratch repository that has never seated it. Manual apply accepts
only its preview digest. Every linked worktree observes one Git-common upgrade,
and the installed pre-commit worker verifies the receipt, assets, and exact
binary before selected-source verification.

## macOS symbols

A macOS release package installs the executable together with its dSYM, and
the receipt records the executable SHA-256, the shared Mach-O UUID, and the Git
commit it was linked from. Verification reads the installed pair and the
receipt and succeeds only when every recorded value agrees with the files on
disk; a mismatch names the exact observed value:

```sh
"$SPICE_BIN" --json install symbols
```

To attribute a short sample of a running installed executable to named Rust
functions and source lines, capture it with the system sampler and hand the
report to the same command:

```sh
sample "$(pgrep -n -f "$SPICE_BIN")" 1 -file "$TMPDIR/spice-sample.txt"
"$SPICE_BIN" --json install symbols --sample "$TMPDIR/spice-sample.txt"
```

The report must come from the installed image: its UUID is compared with the
verified receipt before any lookup. The symbolizer reads at most 4 MiB, resolves
at most 256 distinct addresses of the installed image, and runs one bounded
`dwarfdump --lookup` of at most five seconds per address against the retained
DWARF. Both forms are read-only and record nothing.

## Ownership and reversal

Preview changes nothing. After admitting the exact preview digest, apply may
first run one bounded maintenance pass that reclaims only recognized stale
install-record publications and unreferenced rollback blobs. Explicit
`--unapply` admits that same maintenance pass before reversal. Outside that
Spice-owned pre-plan-effect maintenance, apply changes only receipt-owned assets
and hook authority, and unapply follows the receipt's exact reversal plan.
Neither direction deletes foreign processes or state:

```sh
"$SPICE_BIN" install --unapply
```

Deleting `<git-common>/.spice-rs`, `<git-dir>/.spice-rs`,
`<git-common>/.spice-install`, or foreign state is a separate destructive act
requiring explicit authorization and preserved evidence. Never use pattern-wide
process kills that can match a live peer worktree.
