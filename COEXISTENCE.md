# Coexistence — this port beside the Python predecessor

[INSTALL.md](INSTALL.md) installs this port beside the Python predecessor,
keeping both. This is where the fleet stands today: both systems installed,
both reading this repository, neither decommissioned. It names every boundary
where the two touch, gives each a verdict, and records the command behind it.

Every command here is read-only, and every output was taken on 2026-08-07; the
PIDs and mtimes below belong to that snapshot and will not survive it. The one
command that would change a boundary is named where it applies, and was not
run.

## Register

| # | Boundary | This port | Predecessor | Verdict |
| --- | --- | --- | --- | --- |
| 1 | Executables | `$COMMON/.spice-install/bin/xtask` — **absent** | `~/.local/bin/spice`, editable from `~/devel/spice-z` | disjoint |
| 2 | Durable state | `$COMMON/.spice/data/spicejournal.sqlite3` | `spiceacks`, `spicemaxims`, `spiceprojections`, `spiceteams`, `taskchampion` in the same directory | shared-by-design |
| 3 | Discovery | writes `$COMMON/.spice/canvas` | writes no such file | **conflicting** |
| 4 | Hooks | `.spice-install/git-hooks` via `core.hooksPath` | `.spice/hooks` via `core.hooksPath` | **conflicting** |
| 5 | Configuration | authors root `spice.toml`, predecessor roots only | reads that same file as its `repository` layer, and refuses unknown keys | **decided** |
| 6 | Processes | transient CLI invocations, plus listeners on non-default ports | long-lived serve residents, PIDs 10865 and 25690 | disjoint |
| 7 | Ports | `127.0.0.1:8765` (`resident/runtime.rs:195`) | `serve.port = 8765` | **conflicting** |
| 8 | Browser assets | `dist/stage-app/`, per worktree | embedded in `spice-z/spice/serve/web.py` | disjoint |

## Evidence

Outputs below are as produced, with one substitution: the home prefix is shown
as `~`, because the commit gate refuses absolute user-path literals.

Boundary 1 — executables.

```
$ command -v spice
~/.local/bin/spice
$ ls -d "$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install"
ls: ~/devel/spice-rust.git/.spice-install: No such file or directory
```

This port is not installed. It runs from a build directory; the predecessor
owns the `spice` name on `PATH`.

Boundary 2 — durable state.

```
$ ls "$COMMON/.spice/data/"
spiceacks.sqlite3  spicejournal.sqlite3  spicemaxims.sqlite3
spiceprojections.sqlite3  spiceteams.sqlite3  taskchampion.sqlite3
```

One directory, six databases, disjoint filenames. Shared by design: neither
system opens the other's file. Both source trees independently name `.spice`
(`spice/src/repository.rs:10`, `spice-z/spice/paths.py:21`), so the sharing is
convergent, not negotiated.

Boundaries 3 and 7 — discovery and ports, one realized failure.

```
$ cat "$COMMON/.spice/canvas"
{"endpoint":"http://127.0.0.1:8765","capability":"0386d1ba-…","epoch":"a3b7…"}
$ ls -l "$COMMON/.spice/canvas"
-rw------- 1 c staff 135 Aug 6 04:05 …/.spice/canvas
$ lsof -nP -iTCP:8765 -sTCP:LISTEN
COMMAND   PID USER  FD  TYPE  … NAME
Python  10865    c   3u  IPv4  … TCP *:8765 (LISTEN)
$ curl -sS -i --max-time 5 http://127.0.0.1:8765/ | head -2
HTTP/1.1 200 OK
Server: spice-serve Python/3.14.6
```

`canvas` is written only by this port — `grep -rln canvas ~/devel/spice-z/spice/`
returns nothing, against a control of `grep -c STATE_DIRNAME …/paths.py` → `4`,
so the empty result is absence and not a broken search. Yet the endpoint it
advertises is answered by the predecessor.

This is not a latent clash. It has already happened: a stale discovery file
from Aug 6 04:05 outlived the resident that wrote it, was never invalidated on
exit, and now points this port's clients at a foreign server. The record
carries a `capability` token, and `capability` appears nowhere in the
predecessor's serve code — so a client that follows discovery presents a
capability to a process with no concept of one.

Both sides default to the same port independently: `resident/runtime.rs:195`
pins `127.0.0.1:8765`, and `spice config show` reports
`effective.serve.port = 8765`. There is no resolver to fall back through, by
intent — the comment above that constant says the port is fixed rather than
ephemeral because the operator's browser holds the URL, and the ephemeral
address next to it is reserved for demos. So the default binds or it refuses.

Boundary 6 — processes. Filters must be ANDed with `-a`; `lsof` ORs them
otherwise, which silently returns every listener on the machine.

```
$ lsof -nP -a -c spice -iTCP -sTCP:LISTEN
spice   49177  c  11u  IPv4  … TCP 127.0.0.1:8791 (LISTEN)
spice   57538  c  11u  IPv4  … TCP 127.0.0.1:63524 (LISTEN)
$ lsof -nP -a -c Python -iTCP -sTCP:LISTEN
Python  10865  c   3u  IPv4  … TCP *:8765 (LISTEN)
Python  25690  c   3u  IPv4  … TCP *:8766 (LISTEN)
```

The processes are disjoint: separate executables, no shared handles, and this
port binds loopback where the predecessor binds every interface. Repeating the
probe turns this port's PIDs over while the predecessor's two hold steady,
which is what marks one side transient and the other resident.

But neither live `spice` listener is on 8765, and no config names a port —
`grep -n port spice.toml` is empty and the worktree layer sets only `agent`
and `say`. Peer lanes pass `--port` per invocation to step around a default
they cannot bind, because the predecessor holds 8765 and 8766 permanently. The
collision is not absent; it is worked around by hand, once per lane, with
nothing recording that it must be.

Boundary 5 — configuration.

```
$ spice config show   # grouped by scope
repository  ~/devel/spice-rust-a/spice.toml   (31 keys)
system      ~/devel/spice-z/spice/spice.toml  (249 keys)
worktree    …/worktrees/spice-rust-a/.spice/config/spice.toml (7 keys)

$ cargo run -q -p spice -- config --json \
  | jq -r '.body.provenance[]|select(.source=="tracked")|.path' | cut -d. -f1 | sort -u
policy
tasks
taste
```

The predecessor reads **this repository's** `spice.toml` as its `repository`
layer; this port reads the same bytes as its `tracked` layer. One file, two
readers, one strict validator that does not know the other's schema. Planting
one root this port's registry admits, in memory and never in the file:

```
$ cd ~/devel/spice-z   # this repo's own spice/ shadows the Python package
$ REPO=~/devel/spice-rust-b python3 -c 'import os, tomllib
from pathlib import Path
from spice.config.schema import validate_config_keys as check
p = Path(os.environ["REPO"]) / "spice.toml"; d = tomllib.loads(p.read_text())
check(d, source_name="repository", source_path=p); print("accepted as written")
check(dict(d, gate={}), source_name="repository", source_path=p)'
accepted as written
SpiceError: unknown configuration key gate (source=repository path=…/spice.toml);
            did you mean taste?
```

The file is accepted as written and refused with one port root added, so the
refusal is the schema gap and nothing else. Its advice points at `taste`, an
unrelated table — and because the shared hook runs this validator on every
agent tool call, that advice lands on every lane at once.

Boundary 8 — browser assets.

```
$ ls dist/stage-app/
generations/  current
$ grep -c "<!doctype html>" ~/devel/spice-z/spice/serve/web.py
1
```

`canvas/src/asset.rs:27` stages to `dist/stage-app` inside each worktree; the
predecessor's UI comes from its own package source. No shared path.

## The `core.hooksPath` decision

`core.hooksPath` is single-valued per scope. Live:

```
$ git config --show-origin --get-all core.hooksPath
file:…/worktrees/spice-rust-a/config.worktree   .spice/hooks
$ git config --show-origin --get extensions.worktreeConfig
file:~/devel/spice-rust.git/config       true
```

Because `extensions.worktreeConfig` is enabled, each worktree selects its own
hook directory, and every lane currently selects the predecessor's. Those hooks
are thin shims — `.spice/hooks/pre-commit` is `exec spice dev pre-commit`, so
they dispatch by name through `PATH`. This port's hooks do not:
`spice/src/install/git-hooks/pre-commit` is `exec cargo xtask verify`, and
`reference-transaction` execs `$common/.spice-install/bin/xtask` directly.

**Decision: same-worktree coexistence is unsupported for hooks.** Declaring it
unsupported per *repository* would be the stronger claim, but the evidence
refuses it: `extensions.worktreeConfig` makes per-worktree selection real, and
that is what every lane runs on today. The other direction — one hook path
dispatching to both — is rejected on the evidence in `reference-transaction`'s
own header comment: that hook runs inside the ref transaction while the
repository-global `packed-refs` lock every worktree
contends for is held, and it therefore execs a prebuilt binary and never a
build system. A dispatcher would pay a second interpreter start under that
lock on every ref update, in all lanes at once. The gates also disagree on
rules, so running both means the stricter one silently governs.

What the decision costs: a lane gets one system's guarantees and not the
other's, and moving it across is a config change with no partial state.

**The command that enforces this is `cargo xtask activate-hooks`.**
`preflight_worktree_hooks` in `xtask/src/hooks.rs` walks `git worktree list
--porcelain` and reads each worktree's effective `core.hooksPath` before
anything is published or written. It accepts two values — the tracked
`.githooks` and the path being activated — and refuses on any other, naming the
worktree. The check is on the effective value across all scopes, so two values
configured anywhere refuse even when each alone would pass. It once cleared
every worktree unconditionally; it now cannot strip the predecessor's hooks,
because it stops before the first write. On this fleet every lane selects
`.spice/hooks`, so the command refuses immediately and changes nothing, which is
also why it was not needed to produce this document.

## May the two systems share a repository?

Yes, and they already do — but under conditions held by accident, not by
enforcement.

Sharing works for state, executables, processes and assets: those boundaries
are disjoint or disjoint-by-filename and need nothing. It does not work for
hooks, ports, discovery or configuration. Three of those four are now protected
by something the machine enforces: hooks, by a worktree-scoped setting that
this port's activation command refuses to overwrite; discovery, by a client
that will not dial a record whose publisher no longer holds the resident lock;
and configuration, by the schema-ownership test below. Ports are protected only
by hand, one `--port` per invocation.

So the honest answer is *shared repository yes, shared worktree no, shared
resident never*. What each protection leaves open is narrower than the boundary
itself: a stale `canvas` record is refused rather than dialed, but only once
its publisher is actually gone; the config validator still reads a file the
other system owns, decided rather than prevented; and the fixed default port
remains unbindable in every lane with nothing to raise it.

Schema ownership is decided: the predecessor owns root `spice.toml`, because
its validator runs on every agent tool call fleet-wide while this port's
refuses one command locally. Ten roots this registry admits are barred from
that file and belong in `.spice/config.toml`.
`spice/tests/config_coexistence.rs` carries the decision and fails on a
violation.

Both follow-ups this document once listed are landed: `activate-hooks` refuses
instead of clearing, and a `canvas` record outliving its publisher now faults
`canvas::publisher-gone`. The port collision is what remains, held by hand.
