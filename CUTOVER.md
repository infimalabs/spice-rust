# Spice cutover

This is the one-time clean break from Spice 0.31.0 to the installed Spice
appliance. Run it from an operator shell outside every lane being stopped. It
does not import predecessor tasks, claims, sessions, handles, databases, or
executables.

## Stage without starting

Choose the reviewed worktree and keep every path explicit:

```sh
CUTOVER_REPO=/absolute/path/to/the/reviewed-worktree
CUTOVER_COMMON=$(git -C "$CUTOVER_REPO" rev-parse --path-format=absolute --git-common-dir)
CUTOVER_STAMP=$(date -u +%Y%m%dT%H%M%SZ)
CUTOVER_EVIDENCE="$CUTOVER_COMMON/cutover-$CUTOVER_STAMP"
mkdir -p "$CUTOVER_EVIDENCE"
cd "$CUTOVER_REPO"
cargo build --locked -p spice
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
test -x .spice/bin/spice
shasum -a 256 target/debug/spice .spice/bin/spice | tee "$CUTOVER_EVIDENCE/new-binary.sha256"
```

The two hashes must match. Do not run `.spice/bin/spice up` yet: that is the
forward-only boundary.

## Prove the predecessor is dry

Run the predecessor binary while it is still the ambient `spice` command:

```sh
spice --version | tee "$CUTOVER_EVIDENCE/predecessor-version.txt"
CUTOVER_STATUS=$(spice task status)
printf '%s\n' "$CUTOVER_STATUS" | tee "$CUTOVER_EVIDENCE/predecessor-task-status.txt"
printf '%s\n' "$CUTOVER_STATUS" | awk '
  $1 ~ /^(active|ready|review|blocked|waiting|stale|oops)$/ { count[$1] = $2 }
  END {
    split("active ready review blocked waiting stale oops", required, " ")
    for (index in required) {
      name = required[index]
      if (!(name in count) || count[name] != "0") exit 1
    }
  }
'
spice task next | tee "$CUTOVER_EVIDENCE/predecessor-task-next.txt"
```

The final command must report no available task. Any nonzero or missing count
stops the cutover.

Capture every predecessor lane, then terminate each recorded process group:

```sh
git -C "$CUTOVER_REPO" worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r CUTOVER_WORKTREE; do
  printf '=== %s ===\n' "$CUTOVER_WORKTREE"
  spice --worktree "$CUTOVER_WORKTREE" agent show
done | tee "$CUTOVER_EVIDENCE/predecessor-lanes.before.txt"
```

For every `pgid=N` in that file, run `/bin/kill -TERM -- -N`. After those
processes report completion, repeat the capture into
`predecessor-lanes.after.txt`. Continue only when every row reports `pid=-`;
its settled status may be `idle`, `stopped`, or `unstarted`. An idle row with a
PID is not drained.

## Back up and disable the predecessor

No predecessor writer may remain when this archive is taken:

```sh
CUTOVER_OLD_BIN=$(command -v spice)
ls -l "$CUTOVER_OLD_BIN" | tee "$CUTOVER_EVIDENCE/predecessor-entrypoint.txt"
shasum -a 256 "$CUTOVER_OLD_BIN" | tee "$CUTOVER_EVIDENCE/predecessor-binary.sha256"
tar -C "$CUTOVER_COMMON" -cpf "$CUTOVER_EVIDENCE/predecessor-state.tar" .spice
shasum -a 256 "$CUTOVER_EVIDENCE/predecessor-state.tar" > "$CUTOVER_EVIDENCE/predecessor-state.tar.sha256"
shasum -a 256 -c "$CUTOVER_EVIDENCE/predecessor-state.tar.sha256"
mv "$CUTOVER_OLD_BIN" "$CUTOVER_OLD_BIN.pre-spice-0.31.0"
if test -e "$CUTOVER_COMMON/.spice-rust"; then
  tar -C "$CUTOVER_COMMON" -cpf "$CUTOVER_EVIDENCE/retired-release-one-state.tar" .spice-rust
  shasum -a 256 "$CUTOVER_EVIDENCE/retired-release-one-state.tar" > "$CUTOVER_EVIDENCE/retired-release-one-state.tar.sha256"
  shasum -a 256 -c "$CUTOVER_EVIDENCE/retired-release-one-state.tar.sha256"
  mv "$CUTOVER_COMMON/.spice-rust" "$CUTOVER_EVIDENCE/retired-release-one-state"
fi
test ! -e "$CUTOVER_COMMON/.spice-rust"
test ! -e "$CUTOVER_COMMON/.spice/data/spicejournal.sqlite3"
```

The archive, its checksum, lane identities, process identities, old executable
identity, and staged binary identity are the rollback record. The last
assertion proves the current authority is absent.

## Start the singular appliance

In terminal A, keep the foreground resident attached:

```sh
cd "$CUTOVER_REPO"
.spice/bin/spice up
```

Once `up` has been invoked, the transition is forward-only. In terminal B,
read readiness and the empty board through the installed binary:

```sh
cd "$CUTOVER_REPO"
.spice/bin/spice --json version
.spice/bin/spice --json journal verify
.spice/bin/spice --json board
```

The board must contain no task rows. Create only wanted work with `task add` or
preview and apply a native task document; every resulting handle is new. Seat
fresh lanes and verify steering by their returned lane identities:

```sh
.spice/bin/spice --json lane add --vendor claude
.spice/bin/spice --json lane add /absolute/path/to/codex-worktree --vendor codex
.spice/bin/spice --json steer LANE_ID 'cutover readiness'
.spice/bin/spice --json inbox LANE_ID
```

Shutdown remains a separate operator command. Terminal A exits only after the
durable shutdown sequence completes:

```sh
.spice/bin/spice down
```

## Rollback boundary

Rollback is permitted only while
`$CUTOVER_COMMON/.spice/data/spicejournal.sqlite3` is still absent and `up` has
never been invoked. In that state, move the saved 0.31.0 entrypoint back to
`$CUTOVER_OLD_BIN`, move `retired-release-one-state` back to
`$CUTOVER_COMMON/.spice-rust` when that archived tree exists, and verify both
archived checksums again. If `up` was invoked or the current journal exists,
do not restore predecessor authority or its entrypoint; diagnose and continue
forward with the installed appliance.

The process-level rehearsal is:

```sh
cargo test --locked -p spice --test resident \
  staged_installed_spice_rehearses_clean_break_without_predecessor_tools
```

It uses a normal worktree plus a real linked worktree, applies the staged
binary, proves the journal absent before startup, launches fresh Claude and
Codex fixtures, claims exactly two newly created handles, proves lane-scoped
steering, starts the resident with only `git` and `sleep` available, and uses
an empty executable search path for native client commands, drain, and
shutdown. Python and Taskwarrior are absent from both paths.
