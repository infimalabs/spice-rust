# Preparing and transitioning Spice

Spice can be built and its installation plan previewed beside the Python
predecessor without changing live command, hook, process, or state authority.
Preview is preparation. Applying the exact preview digest publishes the Rust
appliance and transfers this repository's Git-hook authority; it is therefore
the explicit operator decision, not a passive preparation step. Neither preview
nor apply stops, drains, imports, or deletes the other system, and neither
system is a precondition for the other. Retirement remains a separate
destructive decision.

## Development task boundary

Ordinary work in the Spice source repository ends when the validated commit is
landed. Staged selection and package-scoped checks do not compare that worktree
build with the shared installed appliance or print a repair preview. Normal task
completion contains no install step or per-digest authorization request. The
supervising loop calls `cargo xtask install` after landing and is the sole
automatic publisher.

This completion boundary changes only repository-local development guidance.
It does not add a digestless apply path, authorize authored input, or weaken
`spice install --apply=DIGEST`. Every installed client and every manual initial,
repair, migration, or relocation workflow below retains exact-plan authorization.
When the operator explicitly requests one of those manual workflows, that
request—not ordinary task completion—places it in scope.

Run the commands below from the worktree being installed. Paths stay explicit
during preparation. Do not use the bare `spice` name for this appliance until
the operator-authorized authority switch below has established its exact
meaning.

## Before installing

Python and Rust own `spice.toml` and `spice-rust.toml` respectively. A lane that
pinned either with `git update-index --skip-worktree` carries stale
configuration invisibly. Prove no worktree holds a pin:

```sh
SPICE_REPO=/absolute/path/to/this/worktree
SPICE_COMMON=$(git -C "$SPICE_REPO" rev-parse --path-format=absolute --git-common-dir)
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
cd "$SPICE_REPO"
git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r SPICE_WORKTREE; do
  for SPICE_CONFIG in spice.toml spice-rust.toml; do
    SPICE_ENTRY=$(git -C "$SPICE_WORKTREE" ls-files -v "$SPICE_CONFIG")
    test -n "$SPICE_ENTRY" || continue
    printf '%s %s %s\n' "$SPICE_ENTRY" "$SPICE_CONFIG" "$SPICE_WORKTREE"
  done
done | grep -v '^H ' || true
```

That must print nothing. A `skip-worktree` file looks clean to status and diff
and resists checkout and merge; `git ls-files -v` still exposes the pin.

## Build and preview

```sh
cargo build --locked -p spice
target/debug/spice install
```

Build immediately before preview so an older planner cannot decide current
state. `cargo run --locked -p spice -- install` is the rebuild-and-preview
equivalent.

Preview prints the complete digest-bound plan and changes nothing. That plan
publishes the executable, operator skill, and three Git hooks under the Git
common directory's `.spice-install`, clears admitted worktree-local hook
overrides, and switches the common `core.hooksPath` last. Do not apply it until
the repository-wide authority switch below is authorized.

Retired Rust `.spice-rust` state causes `install::state-refused`; preserve and
archive it first. Reverse an old Rust `.spice/install.json` receipt with its
owning executable. Current Rust never treats Python's `.spice` tree as a receipt
or install precondition; its state lives only in `.spice-rs`.

## Operator-authorized authority switch

Stop here unless the operator has explicitly authorized the repository-wide
transition. The switch has two postconditions: every effective
`core.hooksPath` names the installed Rust hooks, and the first `spice` on the
supervised search path is `$SPICE_BIN`.

First inspect every declaring scope. If a linked worktree explicitly selects a
foreign path such as the predecessor's `.spice/hooks`, the installer will refuse
without writing. After authorization, and only when the operator has chosen to
move every such worktree to Rust, replace those foreign selections with the
recognized tracked `.githooks` value. Then preview again and apply only that
newly printed digest; apply publishes the assets and selects the installed
common hook directory:

```sh
SPICE_REPO=/absolute/path/to/this/worktree
SPICE_COMMON=$(git -C "$SPICE_REPO" rev-parse --path-format=absolute --git-common-dir)
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
cd "$SPICE_REPO"
git config --local core.hooksPath .githooks
git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r SPICE_WORKTREE; do
  git -C "$SPICE_WORKTREE" config --worktree core.hooksPath .githooks
done
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
SPICE_HOOKS="$SPICE_COMMON/.spice-install/git-hooks"
test -x "$SPICE_BIN"
shasum -a 256 target/debug/spice "$SPICE_BIN"
test "$(git config --local --get core.hooksPath)" = "$SPICE_HOOKS"
git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r SPICE_WORKTREE; do
  test "$(git -C "$SPICE_WORKTREE" config --get core.hooksPath)" = "$SPICE_HOOKS"
done
```

The Rust supervisor already puts the installed directory first. Give the
operator shell the same meaning and verify it:

```sh
SPICE_BIN_DIR=${SPICE_BIN%/spice}
export PATH="$SPICE_BIN_DIR:$PATH"
hash -r
test "$(command -v spice)" = "$SPICE_BIN"
spice --json doctor
```

## Hooks are a repository-wide choice

Preview checks every linked worktree before writing. An absent selection, or
only repeated identical values naming tracked `.githooks`, the target installed
path, or an intact receipt's prior installed path, is recognized; conflicting
or foreign values refuse. Apply clears admitted worktree overrides and switches
the common path last. Python and Rust may share a repository but not effective
hook authority in one worktree. Moving a foreign selection is an operator
decision before apply.

The three installed hooks are two-line `/bin/sh` launchers that `exec` the
absolute Git-common Spice and preserve Git's argv and I/O. That explicit runtime
identity makes relocation fail closed. [SHELL.md](SHELL.md#generated-git-hook-launchers)
records the exact ABI.

## Hook and appliance recovery

Recovery never selects a substitute backend: installed hooks do not search
`PATH` or fall back to Cargo, `xtask`, the predecessor, or a stale sidecar.

- **Configured backend refusal.** Repair the exact tracked argv, source,
  toolchain, or local configuration named by the diagnostic. Trust command
  names differ across installed client versions. If the installed client
  exposes `spice trust`, run it; if it does not know the current digest, restart
  the resident to pend those exact tracked-plus-local bytes. Review the bytes
  and argv, then run `spice trust grant DIGEST`. That is an exact capability
  approval: any byte change produces a different digest and needs another
  approval.

  An installed client may not expose the top-level `spice trust` family. Do not
  fabricate its journal state or treat an unknown-command error as approval.
  Inspect the repository-shared authority with `spice config trust show`. For
  an exact approval, preview `spice init`, review the ordered operations, then
  apply that same plan with `spice init --apply=PLAN_DIGEST`. Select the one
  exact-approval command family exposed by the installed client's help; do not
  run both versioned paths or retain one as a silent fallback.

  `spice config trust grant` is the alternative only when the operator intends
  a standing signed-provenance delegation. It is not another spelling of
  `spice trust grant DIGEST`. Name only the needed capability and a signer
  fingerprint that Git can already verify, preview the authored-input plan,
  review its repository URL, upstream ref, anchor, capabilities, signer, and
  authority path, then apply that exact plan digest:

  ```sh
  spice config trust grant \
    --path policy.pre_commit \
    --signer SHA256:OPERATOR_APPROVED_KEY \
    --json
  spice config trust grant \
    --path policy.pre_commit \
    --signer SHA256:OPERATOR_APPROVED_KEY \
    --apply=PLAN_DIGEST
  ```

  The standing route requires an origin-backed tracked branch whose clean HEAD
  equals its fetched upstream ref. Later capability changes are admitted only
  when every relevant commit has a verifiable Git signature from a fingerprint
  named in the grant. Without that trusted-signer prerequisite, use exact
  approval instead. Neither preview grants authority, and a changed or stale
  plan must be previewed and reviewed again rather than applied with a new
  guessed digest.

- **Missing, stale, or tampered appliance.** Build current source and run that
  exact `target/debug/spice install` preview. Apply only its fresh digest when it
  admits repair. Receipt, binary, hook, fixed-asset, and other-build failures are
  distinct. Drift may refuse instead of overwriting evidence; preserve the
  replacement and restore receipt-owned bytes, or deliberately reverse with the
  owning release. Never substitute a nearby executable.
- **Foreign hook selection.** Inspect
  `git config --show-origin --get-all core.hooksPath` in every linked worktree.
  Preserve it until the operator chooses one owner for the whole repository,
  then normalize declaring scopes to `.githooks`, preview, and apply. Never
  clear an unknown value merely to pass admission.
- **Repository relocation.** The hook launchers and selected common hook path
  are absolute, so a move fails closed. At the new location, build, preview,
  inspect the rebased one-to-one topology, and apply. Ambiguous topology or a
  foreign selection refuses; no path is guessed.
- **Break glass.** `git commit --no-verify` is acceptable only for an explicitly
  authorized emergency after equivalent checks are run and recorded by hand.
  It bypasses `pre-commit` and `commit-msg`, not `reference-transaction`, and is
  never recovery or completion evidence.

## Start and operate

```sh
cargo xtask app
"$SPICE_BIN" up
"$SPICE_BIN" --json journal verify
"$SPICE_BIN" --json board
"$SPICE_BIN" down
```

`up` returns after authenticated readiness and never falls back from its chosen
endpoint. Detached diagnostics are in `.spice-rs/resident.log`; README covers
foreground, port, browser, and non-loopback options. Rust state and board remain
disjoint from Python's `.spice/data`; `down` stops only Rust Spice.

## Upgrade

From any linked worktree, use one freshly built executable for both steps:

```sh
cargo build --locked -p spice
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
```

Do not unapply first. An intact completed receipt seeds an in-place refresh;
receipt edits, non-binary drift, and interrupted transitions refuse. Version 7
migrates through one digest-bound compressed reversal snapshot: current assets
publish, legacy runtime files retire, then hooks activate. Reversal restores the
exact old assets and receipt without runnable sidecars.

Apply never changes `PATH` and changes hooks only after asset work succeeds.
This shortcut also restores the resident; `--no-resident` omits that lifecycle:

```sh
cargo xtask install --port PORT
```

It retains the endpoint unless `--host` or `--port` overrides it; `--release`
selects release build. Manual apply accepts only its exact preview digest.
Refresh retains the first install's reversal images and replaces files in place,
so every linked worktree sees one Git-common upgrade. The installed pre-commit
worker checks its receipt, assets, and exact current binary before selected
source verification; freshness failures name the repair preview. Nothing
outside Rust Spice stops.

## Retirement is separate from the authority switch

Preview retires nothing; apply changes command and hook authority but deletes no
predecessor process or state. Retiring Python later can destroy its lanes,
`~/.local/bin/spice`, and `.spice/data`; archive that state first. Retiring Rust
can destroy `.spice-rs`, `.spice-install`, and its hook selection. Either act
needs separate authorization and evidence. Never use pattern-wide process kills
that can match a live peer worktree.
