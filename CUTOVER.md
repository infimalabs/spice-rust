# Spice cutover

This is the one-time clean break from Spice 0.31.0 to the installed Spice
appliance. Run it from an operator shell outside every lane being stopped. It
does not import predecessor tasks, claims, sessions, handles, databases, or
executables.

## Capture and build without staging

Choose the reviewed worktree and keep every path explicit:

```sh
CUTOVER_REPO=/absolute/path/to/the/reviewed-worktree
CUTOVER_COMMON=$(git -C "$CUTOVER_REPO" rev-parse --path-format=absolute --git-common-dir)
CUTOVER_BIN="$CUTOVER_COMMON/.spice-install/bin/spice"
CUTOVER_HOOKS="$CUTOVER_COMMON/.spice-install/git-hooks"
CUTOVER_STAMP=$(date -u +%Y%m%dT%H%M%SZ)
CUTOVER_EVIDENCE="$CUTOVER_COMMON/cutover-$CUTOVER_STAMP"
mkdir -p "$CUTOVER_EVIDENCE"
cd "$CUTOVER_REPO"
test -z "$(git status --porcelain)"
git diff --exit-code -- spice.toml
CUTOVER_PINNED=$(git worktree list --porcelain |
awk '/^worktree / { sub(/^worktree /, ""); print }' |
while IFS= read -r CUTOVER_WORKTREE; do
  CUTOVER_ENTRY=$(git -C "$CUTOVER_WORKTREE" ls-files -v spice.toml)
  test -n "$CUTOVER_ENTRY" || continue
  printf '%s %s\n' "$CUTOVER_ENTRY" "$CUTOVER_WORKTREE"
done | grep -v '^H ' || true)
printf '%s' "$CUTOVER_PINNED" | tee "$CUTOVER_EVIDENCE/pinned-configuration.txt"
test -z "$CUTOVER_PINNED"
spice config show > "$CUTOVER_EVIDENCE/predecessor-config.json"
python3 - "$CUTOVER_EVIDENCE/predecessor-config.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    config = json.load(source)
admission = config["effective"]["policy"]["admission"]
expected = ["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1"]
assert admission == expected, (admission, expected)
print(json.dumps(admission))
PY
CUTOVER_OLD_HOOKS=$(git config --worktree --get core.hooksPath)
test -n "$CUTOVER_OLD_HOOKS"
printf '%s\n' "$CUTOVER_OLD_HOOKS" |
  tee "$CUTOVER_EVIDENCE/predecessor-hooks-path.txt"
cargo build --locked -p spice
```

The predecessor configuration capture proves the ambient Python reader accepts
the exact tracked `policy.admission` command before any native install or
authority mutation. No index bit or alternate working-tree document hides the
reviewed policy. Building is the last step that runs while lanes are live: the
installer resolves repository state before it plans and refuses with
`install::state-refused` while a retired release-one `.spice-rust` exists, and
that tree cannot be cleared until the drain below stops every writer that would
put it back. Staging therefore happens in the third section, still before the
predecessor entrypoint is disabled.

One tracked `spice.toml` serves both runtimes, so no lane needs an index-level
parser bridge: the predecessor admits `policy.admission` into its typed
registry without evaluating it, exactly as it already admits `policy.taste`. An
earlier revision of this runbook instead had every predecessor-driven lane pin
the preceding document with `git update-index --skip-worktree`. That pin now
only hides staleness, and it hides it from the instruments above:
`skip-worktree` makes `git status` and `git diff` both report the file clean,
and makes checkout and merge decline to update it, so a lane left pinned
carries stale configuration indefinitely, invisibly, and across the cutover.
The worktree sweep is therefore spelled with `git ls-files -v`, the only one of
the three that can still see a pin, and it stops the cutover while any worktree
holds one.

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

## Back up, stage, and disable the predecessor

No predecessor writer may remain when these archives are taken:

```sh
CUTOVER_OLD_BIN=$(command -v spice)
ls -l "$CUTOVER_OLD_BIN" | tee "$CUTOVER_EVIDENCE/predecessor-entrypoint.txt"
shasum -a 256 "$CUTOVER_OLD_BIN" | tee "$CUTOVER_EVIDENCE/predecessor-binary.sha256"
tar -C "$CUTOVER_COMMON" -cpf "$CUTOVER_EVIDENCE/predecessor-state.tar" .spice
shasum -a 256 "$CUTOVER_EVIDENCE/predecessor-state.tar" > "$CUTOVER_EVIDENCE/predecessor-state.tar.sha256"
shasum -a 256 -c "$CUTOVER_EVIDENCE/predecessor-state.tar.sha256"
if test -e "$CUTOVER_COMMON/.spice-rust"; then
  tar -C "$CUTOVER_COMMON" -cpf "$CUTOVER_EVIDENCE/retired-release-one-state.tar" .spice-rust
  shasum -a 256 "$CUTOVER_EVIDENCE/retired-release-one-state.tar" > "$CUTOVER_EVIDENCE/retired-release-one-state.tar.sha256"
  shasum -a 256 -c "$CUTOVER_EVIDENCE/retired-release-one-state.tar.sha256"
  mv "$CUTOVER_COMMON/.spice-rust" "$CUTOVER_EVIDENCE/retired-release-one-state"
fi
test ! -e "$CUTOVER_COMMON/.spice-rust"
target/debug/spice install
target/debug/spice install --apply=DIGEST_PRINTED_BY_PREVIEW
test -x "$CUTOVER_BIN"
shasum -a 256 target/debug/spice "$CUTOVER_BIN" | tee "$CUTOVER_EVIDENCE/new-binary.sha256"
mv "$CUTOVER_OLD_BIN" "$CUTOVER_OLD_BIN.pre-spice-0.31.0"
test ! -e "$CUTOVER_COMMON/.spice/data/spicejournal.sqlite3"
git diff --exit-code -- spice.toml
target/debug/spice --json config policy.admission |
  tee "$CUTOVER_EVIDENCE/native-admission-policy.json"
cargo metadata --no-deps --offline --format-version 1 \
  > "$CUTOVER_EVIDENCE/native-admission-metadata.json"
./scripts/install-hooks.sh
test "$(git config --get core.hooksPath)" = "$CUTOVER_HOOKS"
```

The two staged-binary hashes must match. Do not run `$CUTOVER_BIN up` yet:
that is the forward-only boundary. The retired release-one root is archived
between the drain and the install because the installer refuses while it exists
and a live lane holding a release-one binary writes it back. Both archives,
their checksums, lane identities, process identities, old executable identity,
staged binary identity, predecessor configuration and hook path, and native
admission evidence are the rollback record. The journal assertion proves the
current authority is absent. The clean diff proves the worktree still carries
exactly the policy committed at `HEAD`. The hook activation follows the install
because its three executable inputs are installer-owned assets. It clears
worktree-specific hook overrides and configures one absolute common hook path,
so existing and future linked worktrees resolve the same installed hooks.

## Start the singular appliance

In terminal A, keep the foreground resident attached:

```sh
cd "$CUTOVER_REPO"
cargo xtask local-app
"$CUTOVER_BIN" up
```

Once `up` has been invoked, the transition is forward-only. In terminal B,
read readiness and the empty board through the installed binary:

```sh
cd "$CUTOVER_REPO"
"$CUTOVER_BIN" --json version
"$CUTOVER_BIN" --json journal verify
"$CUTOVER_BIN" --json board
```

The Canvas web app is built natively into ignored `dist/stage-local`; one atomic
pointer selects a complete immutable generation, and `up` serves only those
lane-local assets. There is no separate opener command: the start already
running in terminal A announces the exact URL on its standard error:

```
canvas endpoint: http://127.0.0.1:8765
```

The resident prints that line only after reading back the discovery file it
just published and admitting it, so the URL named there has already passed the
same ownership, loopback-origin, and generation checks a client applies. Open
it in a browser, or start with `up --open` to have the resident open the
default browser itself.

`127.0.0.1:8765` is the default address; `--host` and `--port` bind a different
one, and `--port 0` takes any free port. The listener is IPv4 loopback unless
`--host` names another address of this machine, so by default the app is
reachable only on the resident host.

The board must contain no task rows. Create only wanted work with `task add` or
preview and apply a native task document; every resulting handle is new. Seat
fresh lanes and verify steering by their returned lane identities:

```sh
"$CUTOVER_BIN" --json lane add --vendor claude
"$CUTOVER_BIN" --json lane add /absolute/path/to/codex-worktree --vendor codex
"$CUTOVER_BIN" --json steer LANE_ID 'cutover readiness'
"$CUTOVER_BIN" --json inbox LANE_ID
```

Shutdown remains a separate operator command. Terminal A exits only after the
durable shutdown sequence completes:

```sh
"$CUTOVER_BIN" down
```

## Rollback boundary

Rollback is permitted only while
`$CUTOVER_COMMON/.spice/data/spicejournal.sqlite3` is still absent and `up` has
never been invoked. In that state, move the saved 0.31.0 entrypoint back to
`$CUTOVER_OLD_BIN`, move `retired-release-one-state` back to
`$CUTOVER_COMMON/.spice-rust` when that archived tree exists, verify the saved
entrypoint and archive checksums again, and leave the tracked policy unchanged.
Verify the restored predecessor against that policy and restore its recorded
Git hook path before returning it to service:

```sh
cd "$CUTOVER_REPO"
mv "$CUTOVER_OLD_BIN.pre-spice-0.31.0" "$CUTOVER_OLD_BIN"
shasum -a 256 -c "$CUTOVER_EVIDENCE/predecessor-binary.sha256"
shasum -a 256 -c "$CUTOVER_EVIDENCE/predecessor-state.tar.sha256"
if test -e "$CUTOVER_EVIDENCE/retired-release-one-state"; then
  mv "$CUTOVER_EVIDENCE/retired-release-one-state" "$CUTOVER_COMMON/.spice-rust"
  shasum -a 256 -c "$CUTOVER_EVIDENCE/retired-release-one-state.tar.sha256"
fi
git diff --exit-code -- spice.toml
spice config show > /dev/null
git config --local --unset-all core.hooksPath || true
git config --worktree core.hooksPath "$(cat "$CUTOVER_EVIDENCE/predecessor-hooks-path.txt")"
test "$(git config --worktree --get core.hooksPath)" = \
  "$(cat "$CUTOVER_EVIDENCE/predecessor-hooks-path.txt")"
```

If `up` was invoked or the current journal exists, do not restore predecessor
authority or its entrypoint; diagnose and continue forward with the installed
appliance.

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
