# Installing Spice

Installation is a digest-bound authority change. Preview writes nothing and
prints the ordered plan with its canonical digest; apply publishes exactly that
plan and selects the repository-wide Git hooks. Neither stops, drains, imports,
or deletes foreign processes or state. The receipt and reversal contract lives
in [Distribution](docs/product/planes/distribution.md#installing-into-a-repository-the-product-does-not-own).

Source work ends with a validated, landed commit and never installs.
Activation, repair, and relocation need explicit operator authorization.

## Build, preview, apply

Run from the root of the worktree being installed, and build immediately before
preview so planner and source agree:

```sh
cargo build --locked --target-dir target -p spice || exit
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

The plan publishes the executable, bootstrap skill, supervised-shell assets, and
three Git hooks under the Git common directory's `.spice-install`, clears
admitted worktree-local hook overrides, and switches the common
`core.hooksPath` last. Verify the result:

```sh
SPICE_COMMON=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
/usr/bin/cmp target/debug/spice "$SPICE_BIN" || exit
test "$(/usr/bin/git config --local --get core.hooksPath)" = "$SPICE_COMMON/.spice-install/git-hooks" || exit
```

Give the operator shell the same executable identity:

```sh
export PATH="${SPICE_BIN%/spice}:$PATH"
hash -r
test "$(command -v spice)" = "$SPICE_BIN" || exit
spice --json doctor || exit
```

### Windows PowerShell

Use the same digest-bound flow from PowerShell. Git for Windows must be in its
system Program Files location because the installer deliberately ignores
`PATH` when it activates hooks:

```powershell
$ErrorActionPreference = 'Stop'
$Git = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'

cargo build --locked --target-dir target -p spice
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
.\target\debug\spice.exe install
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
.\target\debug\spice.exe install --apply=DIGEST_PRINTED_BY_PREVIEW
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$SpiceCommon = & $Git rev-parse --path-format=absolute --git-common-dir
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$SpiceCommon = $SpiceCommon.Trim()
$SpiceBin = Join-Path $SpiceCommon '.spice-install\bin\spice.exe'
if ((Get-FileHash .\target\debug\spice.exe).Hash -ne
    (Get-FileHash $SpiceBin).Hash) {
    throw 'installed Spice does not match the built executable'
}
$ExpectedHooks = $SpiceCommon.TrimEnd('/', '\') + '/.spice-install/git-hooks'
$ObservedHooks = & $Git config --local --get core.hooksPath
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$ObservedHooks = $ObservedHooks.Trim()
if ($ObservedHooks -ne $ExpectedHooks) {
    throw "unexpected core.hooksPath: $ObservedHooks"
}

$env:PATH = "$(Split-Path -Parent $SpiceBin);$env:PATH"
if ((Get-Command spice.exe).Source -ne $SpiceBin) {
    throw 'operator shell did not select the installed appliance'
}
spice.exe --json doctor
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

## Hook authority

One repository has one effective hook authority. Preview inspects every linked
worktree and admits only absence or repeated identical values naming tracked
`.githooks`, the installed hooks, or the receipt's prior installed hooks. Any
other value refuses, naming the worktree and the observed values. Locate its
scope:

```sh
/usr/bin/git -C /absolute/path/to/worktree config --show-origin --show-scope --get-all core.hooksPath
```

System, global, and command scopes must contribute no value. Replace a refused
local or worktree value with `.githooks`, preview again, and apply only the new
digest. Leave admitted values as they are: reversal restores exactly what the
receipt recorded, so standardizing them would change what uninstall puts back.

The installed launchers `exec` the absolute Git-common appliance
([SHELL.md](SHELL.md#generated-git-hook-launchers)). Neither they nor recovery
search `PATH` or substitute Cargo, `xtask`, or a nearby executable.

On Windows, run the same inspection through the admitted Git for Windows
executable:

```powershell
$Git = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'
& $Git -C C:\absolute\path\to\worktree config --show-origin --show-scope --get-all core.hooksPath
```

## Upgrade

An installed appliance verifies a sealed release package (index, manifest,
payloads, proof receipts, artifact identities, modes, and platform) before
applying it, without Cargo, `xtask`, or a source checkout:

```sh
"$SPICE_BIN" install --package /absolute/path/to/spice-release-package || exit
"$SPICE_BIN" install --package /absolute/path/to/spice-release-package --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

From source, publish the app generation, then preview and apply with one freshly
built executable. Refresh in place; do not unapply first:

```sh
"$SPICE_BIN" dev xtask -- app || exit
"$SPICE_BIN" dev cargo -- build --locked --target-dir target -p spice || exit
target/debug/spice install || exit
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

Apply never restarts a running resident. Restart through the installed
appliance with `"$SPICE_BIN" down` and `"$SPICE_BIN" up`, or use the operator
shortcut, which stops the published resident, refreshes, and starts the new
executable; `--no-resident` omits that lifecycle and arguments after `--` go to
`spice up`:

```sh
"$SPICE_BIN" dev xtask -- install --port PORT
```

The shortcut refuses with `resident::operator-install-only` when run from a
lane the repository seats, because that lane runs the appliance it would
replace. A lane may refresh a scratch repository that has never seated it.

### Windows PowerShell

The installed Windows appliance uses the same package and digest contract:

```powershell
$Git = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'
$SpiceCommon = & $Git rev-parse --path-format=absolute --git-common-dir
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$SpiceCommon = $SpiceCommon.Trim()
$SpiceBin = Join-Path $SpiceCommon '.spice-install\bin\spice.exe'

& $SpiceBin install --package C:\absolute\path\to\spice-release-package
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $SpiceBin install --package C:\absolute\path\to\spice-release-package --apply=DIGEST_PRINTED_BY_PREVIEW
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

For a source refresh, publish the app and build through the installed
appliance, then preview and apply with the resulting worktree executable:

```powershell
& $SpiceBin dev xtask -- app
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $SpiceBin dev cargo -- build --locked --target-dir target -p spice
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
.\target\debug\spice.exe install
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
.\target\debug\spice.exe install --apply=DIGEST_PRINTED_BY_PREVIEW
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Restart explicitly after apply, or use the operator shortcut:

```powershell
& $SpiceBin down
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $SpiceBin up
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Alternatively, perform the source refresh and resident lifecycle together.
& $SpiceBin dev xtask -- install --port PORT
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

## macOS symbols

A macOS install keeps the executable's dSYM, and the receipt records the
executable digest, the shared Mach-O UUID, and the source commit. Verify the
installed pair, or symbolize a `sample(1)` report taken from the installed
image; both forms are read-only:

```sh
"$SPICE_BIN" --json install symbols
sample "$(pgrep -n -f "$SPICE_BIN")" 1 -file "$TMPDIR/spice-sample.txt"
"$SPICE_BIN" --json install symbols --sample "$TMPDIR/spice-sample.txt"
```

## Reversal

POSIX shell:

```sh
"$SPICE_BIN" install --unapply
```

Windows PowerShell:

```powershell
& $SpiceBin install --unapply
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Deleting `<git-common>/.spice-rs`, `<git-dir>/.spice-rs`,
`<git-common>/.spice-install`, or foreign state is a separate destructive act
that needs explicit authorization and preserved evidence. Never use
pattern-wide process kills that can match a live peer worktree.

## Recovery

**Executable-policy refusal.** The resident binds executable configuration to
the digest of its exact bytes. Inspect the pending surface with
`spice --json trust`; if the digest is unknown, restart the resident so it pends
the current bytes. Review the reported locations and command words, then grant
only that digest with `spice trust grant DIGEST`. Any byte change is a new
identity; revoke with `spice trust revoke DIGEST`. Never guess a digest.

**Damaged appliance.** Build, preview, and apply a fresh digest as above.
Receipt, binary, hook, and fixed-asset drift refuse distinctly rather than
overwrite evidence; restore the receipt-owned bytes or reverse.

**Relocation.** Launchers and the hook path are absolute, so a moved
repository fails closed. At the new location, build, preview the rebased plan,
and apply.

**Break glass.** `git commit --no-verify` skips `pre-commit` and `commit-msg`,
not `reference-transaction`. Use it only for an explicitly authorized emergency
after running and recording equivalent checks; it is never completion evidence.
