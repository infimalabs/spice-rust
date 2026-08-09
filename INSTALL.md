# Installing Spice beside the predecessor

Spice and the Python predecessor are two systems on one machine, not two stages
of one. Both are installed, both read this repository, and both run
indefinitely: there is no date, release, or step at which one of them ends.
Nothing here stops, drains, imports, or replaces the other, and neither is a
precondition for the other.

Run the commands below from the worktree being installed. Paths stay explicit
because this appliance is invoked by path, never by name.

## Before installing

One tracked `spice.toml` serves both readers, so a lane that pinned it with
`git update-index --skip-worktree` carries stale configuration invisibly. Prove
no worktree holds a pin:

```sh
SPICE_REPO=/absolute/path/to/this/worktree
SPICE_COMMON=$(git -C "$SPICE_REPO" rev-parse --path-format=absolute --git-common-dir)
SPICE_BIN="$SPICE_COMMON/.spice-install/bin/spice"
cd "$SPICE_REPO"
git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r SPICE_WORKTREE; do
  SPICE_ENTRY=$(git -C "$SPICE_WORKTREE" ls-files -v spice.toml)
  test -n "$SPICE_ENTRY" || continue
  printf '%s %s\n' "$SPICE_ENTRY" "$SPICE_WORKTREE"
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

The two hashes must match. The plan publishes the executable, the operator
skill, and three Git hooks under the Git common directory's `.spice-install`,
so this appliance never takes the `spice` name on `PATH`. The predecessor keeps
that name, and its entrypoint is not moved, renamed, or shadowed by anything
here; reach this system through `$SPICE_BIN`.

The installer refuses with `install::state-refused` while a retired
release-one `.spice-rust` tree exists under the common directory. That tree is
this port's own earlier state, not the predecessor's, and archiving it aside is
what clears the refusal. The predecessor's `.spice` directory blocks nothing,
and the native journal may already hold work: installing requires no empty
authority on either side.

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
"$SPICE_BIN" up --port PORT
```

Name a port. The default `127.0.0.1:8765` is held by the predecessor's serve
resident, and that default binds or refuses — there is no fallback resolver, by
intent, because the operator's browser holds the URL. `--port 0` takes any free
port, at the cost of a URL that changes on every start.

`cargo xtask app` publishes one immutable generation into this worktree's
ignored `dist/stage-app`, and the resident refuses to start without a complete
one. The start announces the exact URL on standard output, after reading back
the discovery file it just published and admitting it; `--open` has it open the
default browser too. [README.md](README.md) carries the non-loopback bind and
its bearer token.

Discovery is a single file, `$SPICE_COMMON/.spice/canvas`. A resident withdraws
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
authorities are separate databases with disjoint filenames sharing one
`$SPICE_COMMON/.spice/data` directory. `down` stops this resident and nothing
else.

## Upgrade

```sh
cargo xtask install --port PORT
```

That builds the current tree, republishes the executable, and restarts the
resident on the endpoint the running one had published. `--host` and `--port`
override that endpoint, `--release` installs the release profile, and
`--no-resident` refreshes the executable without starting anything.

The preview and apply above cannot express an upgrade. A completed receipt pins
the executable bytes it installed, so once the tree is rebuilt both `install`
and `install --apply` answer `install::input-refused` with `Spice executable
selection differs from the recorded plan`. The preview refuses too, which
leaves no digest to apply. The command withdraws the recorded plan, derives a
fresh one over the new binary, and applies that.

The previous executable is copied to `target/spice-develop/spice.rollback`
before anything moves and reinstalled if the install or the readiness wait
fails; the reported failure is the one that forced the restore, not the
restore. The restarted resident logs to `target/spice-develop/resident.log`,
and readiness means it published a discovery it also answers on.

`.spice-install` lives under the Git common directory, so an upgrade is
repository-wide: every linked worktree sees the new executable at once, and the
hooks are withdrawn for as long as the plan is being replaced. Re-run
`cargo xtask activate-hooks` when the hook assets or the verifier guard change
— `cargo xtask verify` refuses while the published copy is not the running
build's, and names the command. An upgrade stops nothing outside this
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

## Retiring either system is not part of normal operation

Nothing above retires anything, and no instruction in this repository retires
either system. Should an operator decide to, that is a separate act carrying
its own evidence. These are the states it destroys.

Retiring the predecessor destroys every running predecessor lane and the
process group behind it, and the `spice` entrypoint under `~/.local/bin`. If
its state is removed rather than left in place, it also destroys the
`spiceacks`, `spicemaxims`, `spiceprojections`, `spiceteams`, and
`taskchampion` databases in `$SPICE_COMMON/.spice/data`. Archive that directory
under a verified checksum before touching any of it, because this appliance's
own journal lives beside those files.

Retiring this appliance destroys `spicejournal.sqlite3` in that same directory,
the published `.spice-install` tree, the discovery record
`$SPICE_COMMON/.spice/canvas`, and whichever `core.hooksPath` this repository
selected.

Retiring either system means stopping its lanes, and a stopped battery leaves
loose vendor fixtures running outside every lane's process group. Sweep those
by parent rather than by pattern: a fixture abandoned when its battery died
reports parent PID 1, while a battery still in flight is the parent of its own
children, so a row with any other parent belongs to a live run and must
survive. Kill the PID 1 rows one PID at a time. `pkill -f` matches a peer
worktree's live run as readily as a loose fixture and must not be used here.
