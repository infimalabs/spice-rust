# Coexistence — this port beside the Python predecessor

`CUTOVER.md` describes a one-time clean break. This describes where the fleet
is before that break, and is today: both systems installed, both reading this
repository, neither decommissioned. It names every boundary where the two
touch, gives each a verdict, and records the command behind it.

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
| 5 | Configuration | owns root `spice.toml` | reads that same file as its `repository` layer, and refuses unknown keys | **conflicting** |
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
```

The predecessor reads **this repository's** `spice.toml` as its `repository`
layer, and `spice-z/spice/config/schema.py:357` is documented as *"Refuse the
first structurally unknown key in one source layer."* One file, two readers,
one strict validator that does not know the other's schema. A key added here
for this port can refuse the predecessor across the whole fleet.

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

**The command that violates this is `cargo xtask activate-hooks`.**
`xtask/src/hooks.rs:106-133` walks `git worktree list --porcelain` and runs
`git config --worktree --unset-all core.hooksPath` in *every* worktree before
setting one repository-local path. Run in one lane, it strips the predecessor's
hooks from all of them. It must refuse while a foreign hook path is live in any
worktree it would clear, rather than clearing and reporting. Until then it is
not safe to run unattended, and it was not run to produce this document.

## May the two systems share a repository?

Yes, and they already do — but under conditions held by accident, not by
enforcement.

Sharing works for state, executables, processes and assets: those boundaries
are disjoint or disjoint-by-filename and need nothing. It does not work for
hooks, ports, discovery or configuration. Of those four, exactly one is
protected by anything the machine enforces — hooks, by a worktree-scoped
setting that this port's own activation command is written to delete
fleet-wide. Ports are protected only by hand, one `--port` per invocation.
Discovery and configuration are not protected at all.

So the honest answer is *shared repository yes, shared worktree no, shared
resident never*. Three of those four are already failing quietly: a stale
`canvas` record points this port's clients at a foreign server, one strict
config validator reads a file the other system owns, and the fixed default
port is unbindable in every lane. None of the three raises an error today,
which is exactly why they are written down here.

The follow-ups this implies — invalidate `canvas` on exit, refuse in
`activate-hooks` rather than clear, and decide who owns the root `spice.toml`
schema — are separate work, not done here.
