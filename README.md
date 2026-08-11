# Spice

## Install and start

Spice is this repository's installed appliance. Preview and apply its
digest-bound install plan without changing live command or hook authority:

```sh
cargo run --locked -p spice -- install
cargo run --locked -p spice -- install --apply=DIGEST_FROM_PREVIEW
```

When the operator explicitly authorizes the authority switch, follow
[INSTALL.md](INSTALL.md#operator-authorized-authority-switch). That step makes
the installed Rust appliance the bare `spice` command, moves every linked
worktree through one recognized hook selection, and activates the installed
common hooks. Installation alone performs none of those mutations.

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

This appliance installs beside the Python predecessor for reversible
preparation. Coexistence is not a permanent command-routing contract: the
operator-authorized switch gives the Rust appliance the bare `spice` name and
the repository's single hook path. Retirement remains separate; the switch
does not stop predecessor processes or delete predecessor state.

The activation command refuses unless all three hooks are executable. The
pre-commit hook runs the canonical verifier, which performs
formatting, Clippy, native and browser-target builds, deterministic app
verification, workspace tests, doctests, the machine-readable
contract-landing check. The checked gate manifest maps each of the 25 retired
standard policy checks to one compiled gate arm, and workspace tests exercise
the exact mapping and production executor.

Verification uses an explicit dependency graph. The warm compiler-evidence
chain remains serial through native Clippy, Wasm Clippy, the Wasm workspace
check, and the prebuilt Canvas app. The native resident build follows that
chain while the selected-root Cargo cache is still warm. Browser verification
then consumes that exact resident through `SPICE_CANVAS_BINARY`, so it performs
no Cargo build while the Rust test branch runs. Deterministic workspace tests
wait until those Cargo build stages are finished, prebuild all test binaries
once in the native compiler output root at an explicit supervised setup
boundary, and then start the Canvas and package-shard actions together against
that same selected-root cache. Documentation and doctests follow the workspace
tests so the native Cargo root is not forced into competing test and
documentation publication work. The canonical battery consumes its completed
browser, workspace-test, and compiler evidence alongside that documentation
tail; contract landing still waits for every action before crossing the final
barrier.

The outer scheduler has a hard ceiling of three actions. It counts peer Cargo
processes, reduces that ceiling from three to two to one as peer pressure
rises, and sets the local worker count no higher than the logical CPUs left
after those peers. One worker remains the progress floor when peers already
saturate that budget. The scheduler sets `CARGO_BUILD_JOBS` to the logical-CPU
budget divided across the peer and local participants; an unavailable peer
probe fails closed to one worker, one Cargo job, and one test shard. After the
serialized workspace-test prebuild, it observes peer Cargo pressure once more
and publishes the resulting budget before launching Canvas and test shards.
This lets a transient cold-build burst release capacity at the stable cache
boundary while applying the same constraints to peers that remain. After
reserving every possible concurrent outer sibling's Cargo grant, the scheduler
gives the workspace-test coordinator the remaining logical-CPU slots. In the
full verifier that branch may use the coordinator's four-lane ceiling when no
peer Cargo process exists, is capped at two lanes with one peer, and collapses
to one lane with two or more peers. An unreadable observation uses the same
single deterministic degraded mode: one outer worker, one Cargo job, and one
workspace lane. That preserves zero-peer throughput while retaining
deterministic Canvas and fleet headroom as peer pressure rises.
Full-verifier lanes share the selected root and completed Cargo build cache only
after the one prebuild has published the test binaries they execute. Each lane
keeps a separate target-publication path so concurrent same-package shards
cannot replace one another's final binaries. Concurrent standalone coordinator
callers still receive separate Cargo output lanes and retain the coordinator's
four-lane ceiling; later shards recycle the lane whose prior process group
already exited, so package or target count cannot create an unbounded process,
CPU, or compiler-cache fan-out.

The complete verifier runs inside one supervised child process. Its test-phase
timer starts only after the shared workspace-test prebuild has completed, when
Canvas and the workspace-test coordinator launch; cold compiler setup is
serialized for low contention and does not consume the actual test budget. At
180 seconds of test-phase runtime the child-owned scheduler
stops new work and cancels and reaps every independently grouped action. The
parent supervisor waits until 296 seconds before treating the verifier child
itself as non-cooperative, then reserves one second each for final termination,
reap, and output drain. The resulting worst case is 299 seconds, so entry-point
audit, battery, landing, sibling termination, reap, and output recovery all
remain inside the hard five-minute wall envelope.

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
tests are packed deterministically by source weight across the minimum
per-package shard count, keeping every shard at no more than 8 targets while
spreading larger integration targets across lanes. Batches of at least 3 targets
start largest-first, with equal sizes favoring the package that has the larger
complete target inventory so its final batch cannot pin the tail. One- and
two-target tail batches retain stable metadata order because their measured Cargo
startup cost dominates cardinality. Execution uses no more than the
scheduler-granted workers. Subject to the logical-CPU reservation, the full
verifier grants up to four workspace lanes over the native selected root with no
Cargo peers, up to two with one peer, and one with two or more peers; the
coordinator's reusable safety ceiling remains four owned process groups. Every
shard prints its package, target
inventory, and output lane before launch, reports progress every 30 seconds,
and has a hard 180-second deadline that terminates and reaps the complete
process group.

`cargo xtask verify-action selection` is the ordinary staged-change gate used
by the tracked pre-commit hook. It captures the index once, maps every changed
path to canonical Cargo test targets and focused policy arms, and retains the
path-to-check evidence in one typed report. Known backend, frontend,
documentation, and configuration changes do not trigger an ambient
whole-repository selector. A gate/toolchain authority or an unclassified path
fails closed by selecting every workspace test target and all 27 policy arms.

`cargo xtask verify-action battery` remains the explicit complete policy
surface. It does not share the full verifier's retained compiler evidence and
neither action is a completion verifier. The native `spice gate pre-commit`
command executes the same selected-change report in one isolated worker process
with a 150-second work allowance and cleanup reserves inside its fixed
180-second envelope. A clean worker is silent; a refusal names only selected
failures plus a private canonical report path. The supervisor owns the worker's
complete process group, including Cargo, rustc, Trunk, parsers, and descendants.

For fast feedback while changing one Cargo package, scope Clippy, all-target
tests, and doctests to that package:

```sh
cargo xtask verify --package gate
```

The scoped command identifies every full-verifier action and compiled gate arm
it skips. It is feedback only; run the zero-argument command before completing
or handing off work. Formatting remains a workspace check; run
`cargo fmt --all -- --check` at the package checkpoint.

[DEVELOPMENT.md](DEVELOPMENT.md) maps the crate ownership seams, generated
surfaces, complete test portfolio, validation tiers, measured concurrency
limits, and safe disk-cleanup rules used by development agents.

[SHAPE_CONFORMANCE.md](SHAPE_CONFORMANCE.md) governs the Rust port's matches,
intentional deviations, open gaps, and current self-hosting GO decision against
the authoritative product shape.

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
