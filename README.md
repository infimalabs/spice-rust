# Spice

## Install and start

Spice is this repository's installed appliance. Preview and apply its
digest-bound install plan, then activate the installed hooks once for the
repository:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
cargo xtask activate-hooks
```

The plan publishes the executable, operator skill, and the `pre-commit`,
`commit-msg`, and `reference-transaction` hooks once under the Git common
directory's `.spice-install`. Every linked worktree uses those same assets.
Activation publishes one file more, the `xtask` the `reference-transaction`
hook execs, and records which guard that copy carries. Run it: a repository
with the plan applied but activation never run has a `reference-transaction`
hook whose entry point is absent, and Git aborts every ref update rather than
degrading to no guard. Nothing republishes that copy afterwards, so a landed
change to the guard leaves Git enforcing the older one until this command runs
again; `cargo xtask verify` refuses while that is true and names the command
above. Activation is repository-wide rather than per-worktree, so it inspects
every linked worktree first and refuses, before publishing anything or writing
any configuration, when one selects a `core.hooksPath` it does not recognize.
Only two are recognized: the tracked `.githooks` a fresh checkout runs, and the
path being activated. Everything else belongs to another system and is reported
instead of replaced, which is why the command is safe to run — and why it does
nothing on a repository whose worktrees select the predecessor's `.spice/hooks`
until an operator moves them across. What it does clear, once that inspection
passes, is a recognized worktree-scoped override, so run it only when the whole
repository is meant to run these hooks. Build this lane's Canvas app, then start
Spice with:

```sh
cargo xtask app
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" up
```

That resident serves the HTML, JavaScript, stylesheet, and WASM built in this
worktree's ignored `dist/stage-app` generation store. `cargo xtask app`
finalizes all four files away from the served path and atomically publishes one
immutable generation, so rebuilding cannot mix a page's assets. The resident
refuses to start when no complete generation is active. There is no separate
command to open the app: the start above announces the exact URL on its
standard output, and adding `--open` has the resident open the default browser
too.

```
canvas endpoint: http://127.0.0.1:8767
```

That URL is announced only after the resident reads back the discovery file it
just published and admits it, so it has already passed the same ownership,
loopback-origin, and generation checks a client applies. `127.0.0.1:8767` is
the default; `--host` and `--port` bind a different address, and `--port 0`
takes any free port. The default avoids the predecessor serve resident's ports;
it binds or refuses with no fallback. The listener is IPv4 loopback unless
`--host` names another address of this machine, so by default the app is
reachable only from the host running it.

A non-loopback listener requires explicit consent and a URL-safe bearer token:

```sh
"$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice" \
  up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

The announced and opened local URL ends in
`#correct-horse-battery-staple`. The fragment stays outside the HTTP origin;
the Canvas app presents it only on authenticated bootstrap and speech requests.
Bootstrap returns the capability used for the socket. A wildcard bind
intentionally announces loopback because it has no single routable address.
From a VPN browser, replace `127.0.0.1` with the resident host's VPN address,
keep the announced port and `#TOKEN`, and open that URL. Never put the token
before `#`.

This appliance installs beside the Python predecessor rather than replacing it,
and both are meant to keep running. [INSTALL.md](INSTALL.md) is that runbook: it
names the port the predecessor already holds, the hook path the two cannot
share, and what a retirement would destroy if one were ever wanted.

The activation command refuses unless all three hooks are executable. The
pre-commit hook runs the canonical verifier, which performs
formatting, Clippy, native and browser-target builds, deterministic app
verification, workspace tests, doctests, the machine-readable
contract-landing check. The checked gate manifest maps each of the 25 retired
standard policy checks to one compiled gate arm, and workspace tests exercise
the exact mapping and production executor.

Verification uses an explicit dependency graph. The warm compiler-evidence
chain remains serial through native Clippy, Wasm Clippy, the Wasm workspace
check, the prebuilt Canvas app, and one native resident build. Browser
verification consumes that exact resident through `SPICE_CANVAS_BINARY`, so it
performs no Cargo build while deterministic workspace tests run. The browser
and workspace branches then fork. Documentation and doctests follow the
workspace tests and may continue while the browser branch runs; the canonical
battery waits for both branches and consumes the compiler evidence before
contract landing crosses the final barrier.

The outer scheduler has a hard ceiling of three actions. It counts peer Cargo
processes, reduces that ceiling from three to two to one as peer pressure
rises, and sets the local worker count no higher than the logical CPUs left
after those peers. One worker remains the progress floor when peers already
saturate that budget. The scheduler sets `CARGO_BUILD_JOBS` to the logical-CPU
budget divided across the peer and local participants; an unavailable peer
probe fails closed to one worker, one Cargo job, and one test shard. After
reserving every possible concurrent outer sibling's Cargo grant, the scheduler
gives the workspace-test coordinator the remaining logical-CPU slots, capped
at four, and fixes each shard to one Cargo job. Package or target count
therefore cannot create an unbounded process or CPU fan-out.

The complete verifier runs inside one supervised child process. At three
minutes the supervisor sends termination; the child's signal bridge stops new
work and cancels and reaps every independently grouped action. The supervisor
allows 75 seconds for that orderly cleanup before killing any non-cooperative
in-process remainder, then reserves ten seconds to reap and 30 seconds to drain
output. The resulting worst case is 295 seconds, so entry-point audit, battery,
landing, sibling termination, reap, and output drain all remain inside the hard
five-minute wall envelope. Reaching the fourth minute is exceptional recovery
headroom, never a routine budget.

Every external action runs in a new process group under bounded output,
runtime, termination, and reap deadlines. The first failure or an operator
interrupt cancels and reaps every running sibling before verification returns;
contract landing cannot start afterward. Verification emits flushed start and
terminal lifecycle records for every child-process and in-process action,
including its exact identity, monotonic elapsed time, responsible family, and
exit disposition. A still-running action reports every 30 seconds; terminal
records and cleanup faults are rendered in stable action order even when work
finishes out of order. The native commit-msg hook preserves the current fixed
message-hygiene law and additionally requires every committed `Spice-Origin`
line to use the exact v1 grammar. The native reference-transaction hook refuses
a current-branch update that would abandon commits already merged into its
upstream.

## Workspace verification

Run the same verification outside a commit, and before handing work to
another agent, with:

```sh
cargo xtask verify
```

The command discovers the repository from the current directory and runs the
one canonical verifier; there is no wrapper or second suite. Its workspace Rust
tests are metadata-sorted into deterministic chunks of at most 12 targets and
run in no more than the scheduler-granted workers, with an absolute ceiling of
four owned process groups. Every shard prints its package and target inventory
before launch, reports progress every 30 seconds, and has a hard 180-second
deadline that terminates and reaps the complete process group.

`cargo xtask verify-action battery` is the visible public partial surface for
the canonical gate action. It does not share the full verifier's retained
compiler evidence, is not a completion verifier, and has no legacy alias;
ordinary callers use `cargo xtask verify`.

The configured commit command, `spice gate pre-commit`, runs that ordered
battery in one isolated worker process. It stops work at 135 seconds and
reserves termination, reap, and output-drain time inside a 150-second hard
envelope, below the fixed 180-second extension limit. A clean worker is silent;
a finding renders the complete refusal fold, while a deadline or lost worker
exits nonzero with its last exact gate phase and explicitly incomplete findings.
The supervisor owns the worker's complete process group, including Cargo,
rustc, Trunk, parsers, and their descendants.

For fast feedback while changing one Cargo package, scope the formatter,
Clippy, all-target tests, and doctests to that package:

```sh
cargo xtask verify --package gate
```

The scoped command identifies every full-verifier action and compiled gate arm
it skips. It is feedback only; run the zero-argument command before completing
or handing off work.

## Cargo build cache

Cargo's configured `build_directory` may point every worktree at a shared
machine cache shaped as `CargoBuild/<bucket>/<workspace-hash>`. Inventory it
without writes with:

```sh
cargo xtask build-cache --dry-run --max-bytes 40GiB --retain-live-worktrees 2
```

Apply the same deterministic oldest-first plan with `--apply`, or drive by
free space instead of total cache size with `--free-space-floor 100GiB`. The
collector retains the current worktree and the configured number of hottest
linked worktree roots, skips any root with a held `.cargo-build-lock`, and never
manages `CARGO_HOME`, `CARGO_TARGET_DIR`, or sccache.

[SHELL.md](SHELL.md) inventories the few shell-bound entrypoints that remain
and every orchestration script retired into these Rust commands.

The `gate` integration tests enforce repository contracts from stable product
inputs. Lattice conformance compares every local path dependency in
`Cargo.lock` with the acyclic graph compiled into
`gate/tests/dependency_lattice.rs`. That test's `EXPECTED` table is the source
of truth: it fails when a workspace member is added without an entry or when a
manifest adds, removes, reverses, or cycles a local dependency edge.

Doctrine conformance holds [DOCTRINE.md](DOCTRINE.md) to its compiled section,
kernel, and decision-record contract. It requires every invariant to be stated
rather than merely named, every decision to carry rationale and explicit
deletions, settled language throughout, and every relative record link to
resolve on disk. [TASTES.md](TASTES.md) is the companion record that governs
curated preferences.
