# Installing Spice

Installation is a digest-bound authority change. Preview writes nothing and
prints its canonical digest, change counts, affected paths, and exact apply
command; `--json` exposes the complete ordered plan. Apply publishes exactly
that plan, selects the repository-wide Git hooks, and reports durable receipt
status and progress. Neither stops, drains, imports, or deletes foreign
processes or state. The receipt and reversal contract lives
in [Distribution](docs/product/planes/distribution.md#installing-into-a-repository-the-product-does-not-own).

Source work ends with a validated, landed commit and never installs.
Activation, repair, and relocation need explicit operator authorization.

## Install from source

An installable executable links its app archive, and only the xtask install
builds one: it builds the app, links it into a fresh executable, then derives
the plan once and applies exactly that plan, printing its digest. A plain
`cargo build -p spice` links no app, and its preview refuses with
`install::canvas-refused`. Run from the root of the worktree being installed:

```sh
cargo run --locked -p xtask -- install --no-resident || exit
```

The plan publishes the executable, supervised-shell assets, and
three Git hooks under the Git common directory's `.spice-install`, clears
admitted worktree-local hook overrides, and switches the common
`core.hooksPath` last. Verify the result:

```sh
SPICE_COMMON=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
test -x "$SPICE_BIN" || exit
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

Use the same xtask install from PowerShell. Git for Windows must be in its
system Program Files location because the installer deliberately ignores
`PATH` when it activates hooks:

```powershell
$ErrorActionPreference = 'Stop'
$Git = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'

cargo run --locked -p xtask -- install --no-resident
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$SpiceCommon = & $Git rev-parse --path-format=absolute --git-common-dir
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$SpiceCommon = $SpiceCommon.Trim()
$SpiceBin = Join-Path $SpiceCommon '.spice-install\bin\spice.exe'
if (-not (Test-Path -LiteralPath $SpiceBin -PathType Leaf)) {
    throw "installed Spice executable is missing: $SpiceBin"
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

## Install into another repository

First build an installable executable with the source flow above or obtain one
from a release. The executable may live outside the repository that will use
Spice. From the root of that target repository, invoke the same absolute
executable for both the read-only preview and the digest-bound apply:

```sh
cd /absolute/path/to/target-repository
SOURCE_SPICE=/absolute/path/to/spice
"$SOURCE_SPICE" install
"$SOURCE_SPICE" install --apply=DIGEST_FROM_PREVIEW
```

Replace `DIGEST_FROM_PREVIEW` with the digest printed by the first command.
Using the absolute source path for the second command avoids depending on an
older `spice` selected by the shell before the target installation exists. The
apply copies that appliance and its app into the target repository's Git common
directory, activates its hooks and wrappers, and leaves the
source installation unchanged.

Select and verify the target repository's installed appliance:

```sh
SPICE_COMMON=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
test -x "$SPICE_BIN" || exit
export PATH="${SPICE_BIN%/spice}:$PATH"
hash -r
test "$(command -v spice)" = "$SPICE_BIN" || exit
spice --json doctor || exit
```

On Windows, set `$SourceSpice = 'C:\absolute\path\to\spice.exe'`, then run
`& $SourceSpice install` followed by
`& $SourceSpice install --apply=DIGEST_FROM_PREVIEW`. Use the PowerShell
verification block above to select the target repository's
`.spice-install\bin\spice.exe`.

## Share a macOS executable

Directly shared macOS executables must be signed with a Developer ID and
notarized. The operator needs Apple Developer Program membership, a
`Developer ID Application` certificate and its private key in the login
keychain, and a `notarytool` keychain profile created once:

```sh
xcrun notarytool store-credentials PROFILE
security find-identity -v -p codesigning
```

Build or copy the Darwin executable outside the sealed release, then sign,
submit, wait for Apple's verdict, and verify it with one repository-owned
command:

```sh
spice dev xtask -- share-darwin /absolute/path/to/spice \
  --identity "Developer ID Application: NAME (TEAMID)" \
  --notary-profile PROFILE
```

The command changes the executable in place. It refuses before signing when the
identity or profile is unavailable, retrieves Apple's log when notarization is
rejected, and finishes only after strict `codesign` verification and
Gatekeeper's primary-signature assessment (`spctl --assess --type open
--context context:primary-signature`) accept the file as notarized. A bare
Mach-O cannot carry a stapled ticket, so its first Gatekeeper assessment needs
network access. This operator sharing path does not mutate or replace any
sealed release artifact.

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

## Reference storage

Spice gates commits by reading Git references through gix, which has only the
files backend. A repository whose Git-common `config` sets
`extensions.refStorage` to `reftable`, as `git init` does under
`feature.experimental` or Git 3.0, would install cleanly and then refuse every
gated commit, so preview refuses first with `install::ref-storage-unsupported`,
naming the backend. Migrate before previewing again, or create the repository
with `git init --ref-format=files`:

```sh
/usr/bin/git -C /absolute/path/to/worktree refs migrate --ref-format=files
```

A repository migrated to reftable after install is refused by the gate itself
with the same remedy.

## Upgrade

An installed appliance verifies a sealed release package (index, manifest,
payloads, proof receipts, artifact identities, modes, and platform) before
applying it, without Cargo, `xtask`, or a source checkout:

```sh
"$SPICE_BIN" install --package /absolute/path/to/spice-release-package || exit
"$SPICE_BIN" install --package /absolute/path/to/spice-release-package --apply=DIGEST_PRINTED_BY_PREVIEW || exit
```

From source, refresh in place through the installed appliance; do not unapply
first. The shortcut stops the published resident, builds and applies as above,
and starts the new executable; `--no-resident` omits that lifecycle and
arguments after `--` go to `spice up`:

```sh
"$SPICE_BIN" dev xtask -- install --port PORT
```

Package apply never restarts a running resident. Restart through the installed
appliance with `"$SPICE_BIN" down` and `"$SPICE_BIN" up`.

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

For a source refresh, run the xtask install through the installed appliance.
It restarts the resident with arguments after `--` going to `spice up`;
`--no-resident` omits that lifecycle:

```powershell
& $SpiceBin dev xtask -- install --port PORT
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

After a package apply or a `--no-resident` refresh, restart explicitly:

```powershell
& $SpiceBin down
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $SpiceBin up
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

Deleting `<git-common>/.spice`, `<git-dir>/.spice`,
`<git-common>/.spice-install`, or foreign state is a separate destructive act
that needs explicit authorization and preserved evidence. Never use
pattern-wide process kills that can match a live peer worktree.

## State root

The state root is `.spice` under the Git-common directory and under each
linked worktree's Git directory. No other root name is recognized: a
directory by any other name is foreign state that install never reads, moves,
or deletes. No symlink stands in for the root, because every admission refuses
a linked component.

## Recovery

**Executable-policy refusal.** The resident binds executable configuration to
the digest of its exact bytes. Inspect the pending surface with
`spice --json trust`; if the digest is unknown, restart the resident so it pends
the current bytes. Review the reported locations and command words, then grant
only that digest with `spice trust grant DIGEST`. Any byte change is a new
identity; revoke with `spice trust revoke DIGEST`. Never guess a digest.

**Damaged appliance.** Install from source or apply a sealed package as above.
Receipt, binary, hook, and fixed-asset drift refuse distinctly rather than
overwrite evidence; restore the receipt-owned bytes or reverse.

**Relocation.** Launchers and the hook path are absolute, so a moved
repository fails closed. At the new location, install from source or apply a
sealed package again so the plan rebases.

**Break glass.** `git commit --no-verify` skips `pre-commit` and `commit-msg`,
not `reference-transaction`. Use it only for an explicitly authorized emergency
after running and recording equivalent checks; it is never completion evidence.
