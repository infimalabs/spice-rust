# Preparing and transitioning Spice

Spice installs beside the Python predecessor so the Rust appliance can be
built, inspected, and exercised without changing live command, hook, process,
or state authority. Installation is preparation, not an authority transfer: it
does not stop, drain, import, or replace the other system, and neither system is
a precondition for the other.

One later, explicit operator decision transfers the bare `spice` command and
this repository's Git hooks to the installed Rust appliance. That transition
does not delete predecessor processes or state; retirement remains a separate
destructive decision.

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

That must print nothing. `git status` and `git diff` both report a
`skip-worktree` file clean, and checkout and merge decline to update it, so
`git ls-files -v` is the only one of the three instruments that can still see
the pin.

## Install

```sh
cargo build --locked -p spice
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
test -x "$SPICE_BIN"
shasum -a 256 target/debug/spice "$SPICE_BIN"
```

The build is part of install admission, not an optional warm-up. Do not reuse
an existing `target/debug/spice` without it: that executable can carry a retired
planner contract from an older tree and therefore refuse state the current
source no longer owns. `cargo run --locked -p spice -- install` is the
single-command equivalent for preview because Cargo rebuilds before invoking
the planner.

The two hashes must match. The plan publishes the executable, the operator
skill, and three Git hooks under the Git common directory's `.spice-install`.
Applying that plan alone does not change `PATH` or `core.hooksPath`: the
predecessor keeps the bare `spice` name and current hooks throughout
preparation, and this appliance is reached through `$SPICE_BIN`.

The installer refuses with `install::state-refused` while a retired
release-one `.spice-rust` tree exists under the common directory. That tree is
this port's own earlier state, not the predecessor's, and archiving it aside is
what clears the refusal. Ordinary Python state in `.spice` blocks nothing. A
same-version Rust receipt from before the namespace split at
`.spice/install.json` is different: reverse that installation with its owning
old installed binary before applying this build. The new binary never inspects,
reads, or deletes that path now owned by Python, and does not treat it as an
alternate receipt. After that one-time cutover, Rust state lives only in
`.spice-rs`; installing requires no empty authority on either side.

## Operator-authorized authority switch

Stop here unless the operator has explicitly authorized the repository-wide
transition. The switch has two postconditions: every effective
`core.hooksPath` names the installed Rust hooks, and the first `spice` on the
supervised search path is `$SPICE_BIN`.

On this fleet, each linked worktree currently carries a worktree-scoped
`.spice/hooks` selection. After authorization, move every declaring scope to
the tracked Rust hooks so activation can preflight one consistent value, then
let activation publish and select the installed common hook directory:

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
cargo xtask activate-hooks
SPICE_HOOKS="$SPICE_COMMON/.spice-install/git-hooks"
test "$(git config --local --get core.hooksPath)" = "$SPICE_HOOKS"
git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r SPICE_WORKTREE; do
  test "$(git -C "$SPICE_WORKTREE" config --get core.hooksPath)" = "$SPICE_HOOKS"
done
```

The supervising Rust launcher already puts the installed executable's
directory first for every agent shell. Give the operator shell the same single
meaning, and prove the bare name resolves to the installed appliance:

```sh
SPICE_BIN_DIR=${SPICE_BIN%/spice}
export PATH="$SPICE_BIN_DIR:$PATH"
hash -r
test "$(command -v spice)" = "$SPICE_BIN"
spice --json doctor
```

The generated installed skill can now use bare `spice recall`, `spice board`,
`spice inbox`, and `spice ack` commands without crossing runtimes. It uses the
Rust command tree directly; the transition creates no Python-shaped nested
command forms.

## Hooks are a repository-wide choice

`cargo xtask activate-hooks` is not scoped to the worktree that runs it. It
walks `git worktree list --porcelain` and reads the effective `core.hooksPath`
of every linked worktree before publishing or configuring anything
(`preflight_worktree_hooks`, `xtask/src/hooks.rs`). Two values pass: the tracked
`.githooks` a fresh checkout runs, and the path being activated. Any other
value, or more than one value from any scope, is another system's selection, and
activation refuses naming that worktree rather than clearing it. Nothing is
published and no configuration is written when it refuses, so the command cannot
take a lane away from the predecessor.

Run it only when the whole repository is meant to run this appliance's hooks.
`core.hooksPath` is single-valued per scope, so a worktree gets one system's
gate or the other's and never both: shared repository yes, shared worktree no.
On this fleet every lane selects
the predecessor's `.spice/hooks`, so activation refuses on the first one it
reads and changes nothing; moving a lane across is an operator decision that
precedes the command rather than something the command performs.

## Start

```sh
cargo xtask app
"$SPICE_BIN" up
```

`up` launches this exact executable as a detached resident in its own process
group and returns one endpoint-and-epoch reply only after discovery publication
and an authenticated bootstrap probe. The default `127.0.0.1:8767` avoids the
predecessor serve resident's ports. It binds or refuses — there is no fallback
resolver. Name `--port PORT` to override it; `--port 0` takes any free port.

`cargo xtask app` publishes one immutable generation into this worktree's
ignored `dist/stage-app`, and the resident refuses to start without a complete
one. `--open` opens the default browser. Use `up --foreground` for an attached
process whose caller owns signals, the streaming endpoint announcement, and its
terminal outcome. Detached diagnostics append to the owner-only
`$SPICE_COMMON/.spice-rs/resident.log`. A failed start returns its exact refusal;
inspect that log for preceding runtime diagnostics, repair the named cause, and
run `up` again. [README.md](README.md) carries the non-loopback bind and its
one-shot bearer-token handoff.

Discovery is a single file, `$SPICE_COMMON/.spice-rs/canvas`. A resident withdraws
it on exit, `up` clears a stale one after taking the resident lock
(`clear_stale_surfaces` in `spice/src/resident/runtime.rs`), and `down` faults
with `canvas::release-incomplete` when a record survives the release, in that
same file. Neither path covers a resident that died without unwinding, so the
client does not trust the record alone: before dialing it probes the resident
lock beside the record and faults `canvas::publisher-gone` when the kernel
reports that lock free (`require_live_publisher` in
`spice/src/command_client.rs`). `flock` is released on any process death, so a
killed resident answers the same as an exited one. A client that faults this
way has been kept off whatever answers the port next — on this fleet, the
predecessor.

## Operate and stop

```sh
"$SPICE_BIN" --json version
"$SPICE_BIN" --json journal verify
"$SPICE_BIN" --json board
"$SPICE_BIN" --json lane add --vendor claude
"$SPICE_BIN" --json steer LANE_ID 'readiness'
"$SPICE_BIN" --json inbox LANE_ID
"$SPICE_BIN" down
```

The board carries whatever work this system holds. It is not required to be
empty, and it neither imports nor mirrors the predecessor's board: the two
authorities are in physically disjoint roots. Python owns
`$SPICE_COMMON/.spice/data`; Rust owns
`$SPICE_COMMON/.spice-rs/data`. `down` stops this resident and nothing else.

## Upgrade

From any linked worktree, build once and use that same executable for preview
and apply:

```sh
cargo build --locked -p spice
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
```

Do not `--unapply` first. A completed binary-bearing receipt may seed a refresh
only while its recorded non-binary generated files still match disk; binary
reconciliation names the exact observed executable as superseded. Preview then
derives one current digest, and apply exchanges the assets in place. Receipt
edits, non-binary drift, and interrupted directions still refuse before writes.

Neither command changes `PATH` or `core.hooksPath`; activation remains the
separate `cargo xtask activate-hooks` operation. Apply publishes common assets
only for the exact preview digest. `cargo xtask install --no-resident` is the
one-command build-and-apply shortcut; without that flag it also restores the
resident lifecycle below.

```sh
cargo xtask install --port PORT
```

That builds the current tree, republishes the executable, and restarts the
resident on the endpoint the running one had published. `--host` and `--port`
each override their own half of that endpoint and are honoured whether or not
anything was published, so the first refresh of a repository still lands where
it was told rather than on the default. `--release` installs the release
profile, and `--no-resident` refreshes the executable without starting
anything.

Rebuild, preview, and apply by hand as above when the plan itself is what you
want to read; an apply is refused unless its digest matches the plan just
previewed. Previewing a rebuilt executable or changed embedded hook/skill
against an intact complete install plans a refresh rather than a second
installation: it keeps the reversal images that install recorded and exchanges
each published file for its replacement in place, so a reversal after any
number of refreshes still restores the tree the first install found rather than
the build before this one. The proofs include
`a_linked_worktree_refreshes_all_intact_prior_build_assets_without_unapplying`
and `refresh_publishes_the_rebuilt_executable_without_withdrawing_anything`,
the latter of which reads the installed tree from another thread while the
refresh runs:

```sh
cargo test --locked -p spice --lib install::tests::refresh
```

The previous executable is copied to `target/spice-develop/spice.rollback`
before anything moves and reinstalled if installation or the sole daemon
launcher fails; the reported failure is the one that forced the restore, not
the restore. Upgrade consumes the same typed authenticated readiness reply as a
direct `up` and adds no second discovery wait or connection probe. The restarted
resident writes the same Git-common `.spice-rs/resident.log` as every detached
start.

`.spice-install` lives under the Git common directory, so an upgrade is
repository-wide: every linked worktree sees the new executable at once, and no
worktree sees it missing, because the refresh replaces each published file in
place rather than withdrawing the plan and reinstalling it. The normal
`cargo xtask verify` battery first builds the current Spice executable, then
validates the installed receipt and every asset it owns and compares the
installed executable with those exact built bytes. Missing, stale, tampered,
or other-build installs refuse before source verification and name the
current-build `spice install` preview; apply the digest it prints to refresh.
Re-run `cargo xtask activate-hooks` only when `core.hooksPath` itself needs to
select the installed hook directory. An upgrade stops nothing outside this
appliance.

## The independence proof

Nothing above is a precondition for the predecessor, and nothing the predecessor
does is a precondition for this appliance. One test carries that claim:

```sh
cargo test --locked -p spice --test resident \
  installed_spice_lifecycle_leaves_the_predecessor_system_untouched
```

That test never stopped anything belonging to the predecessor. It builds its
own fixture repository plus a real linked worktree, applies the staged binary
there, starts the resident with only `git` and `sleep` on `PATH`, runs the
client commands with an empty executable search path, and proves Python and
Taskwarrior absent from both. The empty authority it asserts is its fixture's.
Read as an independence proof — this appliance needs no predecessor tool to
install, seat lanes, steer them, or shut down — it is exactly the evidence
coexistence rests on.

## Retirement is separate from the authority switch

Preparation retires nothing. The operator-authorized switch changes command
and hook authority but does not stop a predecessor process or remove its state.
Should an operator later decide to retire either system, that is a separate act
carrying its own evidence. These are the states it destroys.

Retiring the predecessor destroys every running predecessor lane and the
process group behind it, and the `spice` entrypoint under `~/.local/bin`. If
its state is removed rather than left in place, it also destroys the
`spiceacks`, `spicemaxims`, `spiceprojections`, `spiceteams`, and
`taskchampion` databases in `$SPICE_COMMON/.spice/data`. Archive that directory
under a verified checksum before touching any of it. This appliance does not
read or write that directory.

Retiring this appliance destroys `$SPICE_COMMON/.spice-rs`, including
`spicejournal.sqlite3` and the discovery record, the published `.spice-install`
tree, and whichever `core.hooksPath` this repository selected.

Retiring either system means stopping its lanes, and a stopped battery leaves
loose vendor fixtures running outside every lane's process group. Sweep those
by parent rather than by pattern: a fixture abandoned when its battery died
reports parent PID 1, while a battery still in flight is the parent of its own
children, so a row with any other parent belongs to a live run and must
survive. Kill the PID 1 rows one PID at a time. `pkill -f` matches a peer
worktree's live run as readily as a loose fixture and must not be used here.
