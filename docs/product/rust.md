# Rust implementation profile

[Spice product model](README.md)

This file records how the Rust appliance applies the product model. The model is
the default: every implementation-independent invariant applies unless this
file names an exception. An exception is not permission to drift further; it is
the exact replacement contract Rust must preserve.

The corpus was imported from `spice-z` at
`8119859e21eaff5058274473eaa8b56a144eea3a`; its curated product-model commit is
`9394ea696dce37b19efbb163b0ccc96b8b568e78`. Rust-specific status is intentionally
not written back into the shared model.

## Applicability decision

| Corpus area | Rust disposition |
| --- | --- |
| Value, loops, model, planes, interfaces, stories | **Applies.** These state product behavior independently of language and storage. |
| Invariant register and case battery | **Applies except for the two exception families below.** Existing Rust tests are the implementation evidence; the case prose is not a second test runner. |
| Checks | **Applies with the selected-gate and accepted-state replacements below.** Instrument membership and thresholds remain repository policy. |
| Surface mechanics | **Not imported.** That tree named another client's subscription protocol, facet authorities, functions, and constants. The surface and jitter invariants remain applicable; Rust mechanics live in `wire`, `app`, `canvas`, and their tests. |

No other copied product document is implementation-specific. A disagreement
outside the exception register is a Rust defect, not an undocumented choice.

## Exception register

### Checks: local selected evidence replaces the complete commit-time roster

The product description presents a [fixed commit-time roster](checks.md#what-it-is).
Rust's ordinary hook instead captures the staged source once and maps recognized
paths to a deterministic local set of canonical gate arms and Cargo targets.
Selector or toolchain authority, an unclassified path, or a Rust/manifest path
outside the workspace refuses before any selected work begins. It never widens
an unknown change into an unbounded repository run.

The replacement contract is:

- one immutable staged tree and digest feed selection, policy, tests, and the
  final report;
- selection is a pure exact-path mapping, and unknown impact is a refusal;
- selected arms retain canonical order and collect their selected findings;
- package checks and explicitly named study arms are separate, bounded
  surfaces; the unscoped repository verifier is retired and refuses before it
  launches work; and
- ordinary completion evidence never escalates from the changed slice to a
  complete repository battery.

This also replaces the product study rule that [selects names from Git but reads
content from the working tree](checks.md#what-a-study-sees). Rust measures the
captured staged bytes. A moving index or a mismatch between selection and
content refuses; it cannot report on a hybrid tree.

Finally, Rust makes durable gate state stricter than the product's immediate
in-scan latch retirement. Latch creation, hold, healing, rename transfer, and
claim changes are proposed in memory and published as one typed transaction
only when the enclosing selected run accepts. A refused run is byte-preserving,
including proposed healing. Partial accepted runs reconcile only the keys they
observed and cannot prune unrelated state.

Authorities: [`gate/src/change_selection.rs`](../../gate/src/change_selection.rs),
[`gate/src/state.rs`](../../gate/src/state.rs),
[`spice/src/change_gate.rs`](../../spice/src/change_gate.rs), and
[`xtask/src/lib.rs`](../../xtask/src/lib.rs). Executable coverage lives in
[`gate/tests/policy.rs`](../../gate/tests/policy.rs),
[`gate/tests/report_authority.rs`](../../gate/tests/report_authority.rs), and
[`spice/tests/gate_selection_cli.rs`](../../spice/tests/gate_selection_cli.rs).

### Session: completion learnings are not a current Rust product

The model includes [completion-time learning distillation and a bounded
store](invariants/session.md#session-learnings-are-distilled-at-task-completion-and-held-in-a-bounded-store).
Rust deliberately does not distil or persist completion learnings. Its session
surface is daemonless and read-only; the optional learning field in older task
answers is decoding compatibility only and current completion never produces
it. The retired `.spice-rs/learnings` path is not owned or created.

Briefing, bounded transcript windows, compaction reconstruction, message-shape
classification, task-effort accounting, the working-state meter, taste
evidence, and maxim mining remain current. Adding completion learnings requires
a new product decision and a durable authority; it must not be smuggled in as a
session-report side effect.

Authorities: [`spice/src/session_command.rs`](../../spice/src/session_command.rs),
[`wire/src/intent/task.rs`](../../wire/src/intent/task.rs), and
[`spice/src/repository_tests.rs`](../../spice/src/repository_tests.rs).

## Rust selections that do not change the product contract

These choices are free under [What is fixed and what is
free](planes/principles.md#what-is-fixed-and-what-is-free), but they are
load-bearing for this implementation.

### Authority and storage

One foreground resident holds the kernel lock and is the sole writer to one
SQLite journal. A command transaction commits immutable facts, authoritative
heads, coordination heads, its result, and the wake cursor together. The board
reads those heads directly. Disposable folds live in a separate store and can
be rebuilt from admitted journal history; transcript tape preserves vendor
bytes without becoming semantic authority.

The journal accepts one current schema and an ordered one-way migration chain.
Wire and browser compatibility have independent current versions and resident
epochs; there are no negotiated or release-specific runtime arms.

Authorities: [`journal/src/guard.rs`](../../journal/src/guard.rs),
[`journal/src/replay.rs`](../../journal/src/replay.rs),
[`journal/src/migrate.rs`](../../journal/src/migrate.rs),
[`fold/src/store.rs`](../../fold/src/store.rs), and
[`wire/src/hello.rs`](../../wire/src/hello.rs).

### Principals and process life

Rust assumes cooperative occupants running as one Unix user. Capabilities bind
attribution, lane, occupant, and epoch; they are workflow interlocks, not a
claim of adversarial isolation between same-user processes. Session authority
comes only from an admitted live vendor identity. Transcript prose may select
evidence but can never impersonate the caller.

The resident runs in the foreground of its owning process boundary. `spice up`
may detach that exact process; `spice down` owns the semantic shutdown. Operating
system service managers may own restart policy outside the appliance. Rendering
and observation never launch an occupant.

Authorities: [`spice/src/session_command/authority.rs`](../../spice/src/session_command/authority.rs),
[`spice/src/resident.rs`](../../spice/src/resident.rs), and
[`spice/src/resident/lifecycle.rs`](../../spice/src/resident/lifecycle.rs).

### Extensions and command routing

Vendors, wrappers, and gates are typed values in one compiled registry.
Repository command mounts merge into the generated CLI only after collision and
configuration admission. Built-in verbs win at every depth. Rust does not use a
language package's entry-point discovery as runtime command authority.

Authorities: [`spice/src/extension.rs`](../../spice/src/extension.rs),
[`spice/src/cli.rs`](../../spice/src/cli.rs), and
[`spice/src/cli/mounts.rs`](../../spice/src/cli/mounts.rs).

### Canvas and transport

Rust/WASM is the only production browser client. JavaScript is browser-test
code, not a second product runtime. `wire` is the sole protocol vocabulary; the
browser imports its closed DTOs and the resident serves one immutable app
generation. The client uses one authenticated, epoch-bound stream socket with
bounded pages and retention, one correlation identity per request, explicit
reconnect pacing, and a liveness probe that turns a half-open connection into
the ordinary reconnect path.

The model's lane/team, durable draft, attribution, reading, motion, scroll, and
no-launch invariants still bind this implementation. The removed mechanics
documents' `lanes.subscribe` frames, per-facet chrome reducer, and numeric
constant table do not.

Authorities: [`wire/src/stream.rs`](../../wire/src/stream.rs),
[`canvas/src/stream.rs`](../../canvas/src/stream.rs),
[`app/src/browser.rs`](../../app/src/browser.rs),
[`app/src/runtime.rs`](../../app/src/runtime.rs), and
[`app/src/mosaic.rs`](../../app/src/mosaic.rs). Browser-edge coverage lives in
[`app/browser/canvas.spec.mjs`](../../app/browser/canvas.spec.mjs) and the other
focused modules beside it.

### Steering and claim warnings

Durable direction and semantic retirement follow the product loop. Filesystem
inboxes are delivery materializations; journal facts own lifecycle history.
Current lease warnings are read-only command-stderr decoration produced by the
working-state meter. The resident does not append a warning fact or turn a
warning into steering. Older Rust warning facts remain decodable as retained
journal evidence but have no current producer.

Authorities: [`steer/src/directives.rs`](../../steer/src/directives.rs),
[`spice/src/session_meter.rs`](../../spice/src/session_meter.rs), and
[`spice/src/run_meter.rs`](../../spice/src/run_meter.rs).

### Installation, coexistence, and current command ownership

Install preview is read-only and apply is bound to the exact preview digest.
Assets publish before the common Git hook path switches; reversal restores the
first captured prior state and reports modified residue. Installing Rust does
not import, stop, or delete predecessor state. Python and Rust keep distinct
state roots, databases, inboxes, discovery, configuration, Git shadows, and
receipts. Retiring either runtime is a separate operator-authorized act.

The repository is currently hybrid. In an ambient Python-controlled harness,
unqualified `spice session`, `spice task`, and `spice steering` remain Python
control-plane commands. Rust-only `dev cargo` and `dev xtask` must use the exact
non-symbolic executable at the Git-common `.spice-install/bin/spice`. Its
existence does not establish a Rust lane, session, or task claim. A missing or
invalid asset is a hard stop; do not search `PATH` or select another Cargo home.

Authorities: [`spice/src/repository.rs`](../../spice/src/repository.rs),
[`spice/src/install.rs`](../../spice/src/install.rs),
[`spice/src/cargo_isolation.rs`](../../spice/src/cargo_isolation.rs),
[`spice/tests/coexistence.rs`](../../spice/tests/coexistence.rs), and
[`spice/CARGO_ISOLATION.md`](../../spice/CARGO_ISOLATION.md).

### Release and proof

Rust specializes the generic release loop as a multi-target sealed appliance.
Each target payload binds the native binary, app generation, toolchain, and
environment. Proof runs outside the candidate, emits a detached receipt, and a
post-proof seal composes the target manifest. Publication consumes only that
sealed object through the same digest-bound, resumable plan it previewed.

Authorities: [`dist/src/release.rs`](../../dist/src/release.rs),
[`proof/src/lib.rs`](../../proof/src/lib.rs), and
[`spice/src/release_command.rs`](../../spice/src/release_command.rs).

## Review rule

When Rust changes in one of these areas, review the product invariant first and
this profile second. A new disagreement must be resolved as one of three things
in the same change: repair Rust, amend the shared product model at its source,
or add a narrowly stated exception here with executable evidence. Silence is
not a fourth disposition.
