# Rust implementation profile

[Spice product model](README.md)

The product model and this profile form one contract. The model fixes portable
behavior; this profile fixes the concrete choices left to the Rust appliance.
A disagreement with either document is a defect, not an undocumented option.

Python implementations and their ambient state, configuration, and conventions
are non-authoritative and must be ignored.

## Verification boundary

The ordinary commit gate captures one immutable staged tree, computes its
digest, and maps every recognized path to the exact arms, Cargo targets, and
reach owned by its compiled classification. Exact selector-authority entries
select the exhaustive battery; authority-shaped paths outside those entries do
not inherit that reach. In particular, the exact `spice/src/change_gate.rs` is
authority, while the five `spice/src/change_gate/` submodules — `cargo_budget`,
`policy_schedule`, `progress`, `test_execution`, and `worker_scheduler` — remain
ordinary Rust sources with the seven-arm Rust reach. An unclassified path or a
Rust/manifest path outside the workspace refuses before work begins; unknown
impact never guesses at coverage.

Packaged selection, policy, tests, and the final report consume the same captured
bytes. Configured commands are separate: they run in authored order with captured
selection/digest metadata from the live repository working directory and must
establish their own source binding before reading bytes. A moving index or
packaged-source mismatch refuses. Returned typed arm reports retain all of their
findings in canonical order, and already-started independent work is joined. A
scheduler error lowers the canonical failure cutoff, leaves unstarted later arms
without invented rows, and returns the first canonical error without a complete
report. Package checks and explicitly named study arms are separate bounded
surfaces; the unscoped repository verifier refuses before launching work.

Gate-state changes are proposed in memory and published as one typed
transaction only when the packaged selected run accepts. A refused packaged
selected run is byte-preserving, including proposed healing. A later configured
command refusal does not roll back state already published by an admitted
packaged selected run. A partial run limits measurement and result
reconciliation to observed sticky keys and preserves omitted keys that survive
complete-ledger run-start rename, liveness, and expiry maintenance. All of
those changes remain transaction-local until the packaged selected run accepts.

Authorities: [`gate/src/change_selection.rs`](../../gate/src/change_selection.rs),
[`gate/src/state.rs`](../../gate/src/state.rs),
[`spice/src/change_gate.rs`](../../spice/src/change_gate.rs), and
[`xtask/src/lib.rs`](../../xtask/src/lib.rs). Executable coverage lives in
[`gate/tests/policy.rs`](../../gate/tests/policy.rs),
[`gate/tests/report_authority.rs`](../../gate/tests/report_authority.rs), and
[`spice/tests/gate_selection_cli.rs`](../../spice/tests/gate_selection_cli.rs).

## Task identity codec

Rust mints every current task's `born` identity as exactly nine ASCII
characters encoding nonnegative Unix microseconds in fixed-width base 52 with
the exact ordered alphabet
`0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz`. Ordinary string order
equals represented-instant order within that current generation, and the
omitted vowels keep a minted stamp out of prose-shaped words. The 52⁹ space
spans about 88.09 years from the Unix epoch; its most-significant digit advances
about every 1.69 years, the 2026 era begins with `d`, and the first instant
outside the space is near 2058-02-02. An out-of-range instant refuses instead of
wrapping.

Task allocation resolves a collision by probing exactly one microsecond earlier
until the stamp is free. It never claims a future instant or appends randomness
or entropy, and a rewind that would cross Unix zero refuses. A requested batch
gathers free candidates while rewinding, then returns them in ascending stamp
order so assignment preserves the batch's authored order. An independent later
mint may therefore sort before the task it collided with; journal sequence and
durable queue origin, not stamp order, remain creation chronology authority.

Retained eight-character base-52 Unix-millisecond stamps remain readable in
facts, heads, handles, CLI operands, origins, indexes, and app presentation.
They are never minted by current creation. Mixed-width equality, hashing, and
ordered collections remain deterministic, but ordinary string order across
codec generations or collision-adjusted current stamps is not creation
chronology.

Steering uses the same nine-character microsecond codec for current keys and
identity-derived inbox seals. Parsing also admits retained eight-character
alphanumeric keys; an eight-character vowel-free key contributes its decoded
millisecond multiplied by 1,000 to the microsecond high-water, while an older
base-62 key outside the current alphabet contributes no invented time. Current
durable steering heads and indexes store `minted_micros`; schema 13 is retired
and names release `d811711a3` for its one-time conversion.

Authority: [`wire/src/witness.rs`](../../wire/src/witness.rs). Executable
coverage lives in
[`spice/src/board_command/tests/task_listing.rs`](../../spice/src/board_command/tests/task_listing.rs).
Steering codec coverage lives in
[`steer/src/ack/keys.rs`](../../steer/src/ack/keys.rs) and
[`steer/tests/scan.rs`](../../steer/tests/scan.rs).

## Session boundary

Session reporting is daemonless and read-only. It briefs from admitted
transcript and repository evidence, reconstructs selected-horizon windows across
compaction, and reports message, phase, command, commit, and token views. It
never distils or persists completion learnings and owns no learning store.
Decodable fields that have no producer are inert input, not product behavior.

Authorities: [`spice/src/session_command.rs`](../../spice/src/session_command.rs),
[`wire/src/intent/task.rs`](../../wire/src/intent/task.rs), and
[`spice/src/repository_tests.rs`](../../spice/src/repository_tests.rs).

OMP capture separates physical RPC frames from durable logical records.
The vendor decoder reassembles ordered `rpc_chunk` envelopes before lane
capture applies restatement filtering, appends to tape, or admits a session
declaration. Physical frames remain bounded to 1 MiB; one assembly admits at
most 256 chunks of 256 KiB and 64 MiB of original logical JSON. Capture adds
one newline without reserializing the JSON, so live decoding and replay share
the same bytes and locus. Unchunked records retain their original bytes.

Invalid metadata, sequence identity/order, canonical base64, payload lengths,
UTF-8, or logical JSON refuse capture. An incomplete sequence at EOF also
refuses: SILL delivers stream finalization in order through the same bounded
observer worker and failure path as records, including newline-terminated
streams. Chunk envelopes presented directly to the logical decoder are
accounted residue, not silent content.

The lane states the wire it reads rather than inheriting one. Its opening
request negotiates RPC protocol version 2 before any other command, because
OMP switches its frame encoder on that reply and only that version chunks an
oversized frame whole instead of compacting, shrinking, or replacing it. The
same request states the steering, follow-up, and interrupt dispositions,
which OMP otherwise takes from whatever the host machine's own settings say:
steering and follow-up drain every queued message at a boundary rather than
one per turn, and an interrupt is noticed immediately rather than after the
running tool batch. A refused command answers explicitly, and a refused
negotiation is a lane failure naming the version Spice requires, not a
launch that proceeds on an undecodable wire.

Each launch also pins two files beside the occupant's supervisor record and
names them on the command line, so no global OMP setting is read or changed
for either. The configuration overlay states the compaction threshold,
language-server and service-tier dispositions, and turns OpenAI live steering
off, so steering waits for the next tool or turn boundary on every model. The
steering extension clears OMP's steering flag on the per-request copies OMP
hands the provider, so a steered message reaches the model as the lane wrote
it rather than inside OMP's generated "priority; supersedes conflicting prior
instructions" notice. Queueing, interruption, the stored transcript's steering
flag, and attachments are unchanged.

Authorities: [`vendor/src/omp.rs`](../../vendor/src/omp.rs),
[`vendor/src/omp/frame.rs`](../../vendor/src/omp/frame.rs),
[`vendor/src/omp/steering.mjs`](../../vendor/src/omp/steering.mjs),
[`lane/src/capture.rs`](../../lane/src/capture.rs), and
[`sill/src/spawn/capture.rs`](../../sill/src/spawn/capture.rs).

The app reads those records back under per-dialect allowances. A bounded
startup or history window pays for its own bytes plus one boundary
allowance: the tape index, then the longest record its dialect can land, so
a window that begins inside a record retreats over the whole of it. OMP is
sized from the reassembled logical record and every other dialect from the
physical capture limit, so one vendor's transport does not widen the budget
another lane reads under. The startup ceiling across lanes is sized from the
widest dialect and each lane is charged its own allowance. A record longer
than its dialect admits refuses with the dimension it exhausted rather than
reading on, and a window inside the tape's first record retreats to byte
zero, which is a boundary rather than an exhausted allowance.

Authorities:
[`spice/src/resident/message_surface/backlog.rs`](../../spice/src/resident/message_surface/backlog.rs)
and [`listen/src/stream/tail.rs`](../../listen/src/stream/tail.rs).

## Authored task documents

The Rust `doc` command classifies without an external family context, so its
project is `task.document`. Each non-synthetic node matches an existing row by
that exact project plus the internal `document-slug:<slug>` tag. A changed
document digest edits the same matching row; origin remains creation provenance
and is not a matching namespace.

Apply is a replacement projection, not a patch or a lossless board import. It
binds the named document digest and the board barrier captured during that apply
invocation; the public preview does not seal a board barrier across invocations.
New rows receive the ordinary acceptance-sensitive flow default when flow is
absent. Existing rows replace title, acceptance, authored tags, priority tag,
and parsed dependencies, while their workflow flow and current phase remain
unchanged. Description follows the same replacement semantics. Dialect
annotations append as exactly deduplicated `TaskNoted` evidence, so omission in
a later document does not erase prior annotations or later notes. Due values do
not enter board heads. The dialect accepts only `design`, `plan`, `todo`,
`verify`, and `review` in `Flow:`; that closed input grammar is separate from
the board's open phase vocabulary. `doc ledger HANDLE` emits a
dialect-representable current-task recovery view containing task prose, flow,
authored tags, priority, and append-only notes. It is not an inverse family
export; an unrepresentable current head set reports
`doc::ledger-unrepresentable` rather than widening the dialect.

Authorities: [`board/src/apply.rs`](../../board/src/apply.rs),
[`spice/src/board_command/client_document.rs`](../../spice/src/board_command/client_document.rs),
[`spice/src/board_command/progress.rs`](../../spice/src/board_command/progress.rs),
and [`spice/src/board_command/read_guard.rs`](../../spice/src/board_command/read_guard.rs).
Executable coverage lives in
[`dialect/tests/classification.rs`](../../dialect/tests/classification.rs) and
[`spice/src/board_command/tests/resolution.rs`](../../spice/src/board_command/tests/resolution.rs).

## Authority and storage

One resident holds the kernel lock and is the sole writer to one SQLite journal.
A command transaction commits immutable facts, authoritative heads,
coordination heads, its result, and the wake cursor together. The board reads
those heads directly, while resident read models advance from admitted journal
history in memory. Transcript tape preserves vendor bytes without becoming
semantic authority.

Board reads name their scope explicitly: `all`, `tasks`, or `trust`. A task
scope selects durable birth stamps and may include transitive dependencies and
the authenticated principal's active claims at the same snapshot barrier.
Completed tasks remain addressable, including completed prerequisites, terminal
completion replay, and historical taste proposals. Completion and review request
the dependency closure before constructing progress witnesses, including when
completion resolves its task from the principal's active claim. Trust reports
transfer no task rows. Allocation, board presentation, document barriers, and fresh
birth-stamp allocation retain explicit full-board reads; they do not substitute
a pending-only view for retained authority. Scoped reads omit global allocator
history rather than presenting partial history as complete.

Task-artifact commands remain daemonless. They read bounded current task-head
pages and their reply cursor inside one read-only journal transaction, so
concurrent publications cannot mix task status generations. The transaction
ends before artifact payload I/O or store locking; these commands neither start
a local Bus nor migrate the journal.

Journal schema 33 is the sole executable shape. `Journal::open` admits an
existing authority as exact-current before changing its journal mode or any
persistent setting. Schemas 1–32 refuse before authority mutation and name the
release that last migrated them:

| Journal schema | Retired owner |
| --- | --- |
| 1 | `122d2a842` |
| 2, 3 | `IDENTIT-1kLGl2S5` |
| 4 | `b6acb226d` |
| 5 | `d0c14dcc4` |
| 6 | `SESSION-1kLLCyqw` |
| 7, 8 | `MINING-1kLNTy1C` |
| 9 | `87636d7af` |
| 10 | `1kRNj6X7` |
| 11 | `257200978` |
| 12 | `6ad57c713` |
| 13 | `d811711a3` |
| 14 | `ENVIRON-dRyzKgHJw` |
| 15 | `SCHEMAR-dRyzNKsdp` |
| 16 | `b9a45a6ec` |
| 17 | `37cc1297f` |
| 18 | `ALLOCAT-dS8cvNplX` |
| 19 | `BOUNDS-dSHsMK0kz` |
| 20 | `eab8365a6` |
| 21 | `d431530eb` |
| 22 | `c64c48cac` |
| 23 | `TASKS-dSPJyjNSH` |
| 24 | `TASKS-dSPW61MXd` |
| 25 | `fb96c12a7` |
| 26 | `966d33bef` |
| 27 | `916081f8a` |
| 28 | `02715f9e1` |
| 29 | `SPEECH-dTQBpHHJq` |
| 30 | `a4abe071f` |
| 31 | `55ad00080` |
| 32 | `a15ed13f4` |

Wire and browser compatibility have independent versions and resident epochs;
runtime behavior never branches by release.

Authorities: [`journal/src/guard.rs`](../../journal/src/guard.rs),
[`journal/src/replay.rs`](../../journal/src/replay.rs),
[`journal/src/migrate.rs`](../../journal/src/migrate.rs), and
[`wire/src/hello.rs`](../../wire/src/hello.rs).

## Principals and process life

Occupants cooperate under one Unix user. Capabilities bind attribution, lane,
occupant, and epoch; they are workflow interlocks, not adversarial isolation
between same-user processes. Session authority comes only from an admitted live
vendor identity. Transcript prose may select evidence but cannot impersonate
the caller.

The resident runs in the foreground of its owning process boundary. `spice up`
may detach that exact process; `spice down` owns semantic shutdown. An operating
system service manager may own restart policy outside the appliance. Rendering
and observation never launch an occupant.

Authorities: [`spice/src/session_command/authority.rs`](../../spice/src/session_command/authority.rs),
[`spice/src/resident.rs`](../../spice/src/resident.rs), and
[`spice/src/resident/lifecycle.rs`](../../spice/src/resident/lifecycle.rs).

## Extensions and command routing

One compiled extension registry owns vendors, wrappers, and gates. A separate
closed command-contract registry owns built-in CLI verbs. Repository command
mounts merge into the generated CLI only after collision and configuration
admission. Built-in paths are reserved; any colliding repository mount refuses
before dispatch. The compiled registries and admitted repository mounts are the
entire command vocabulary.

Authorities: [`spice/src/extension.rs`](../../spice/src/extension.rs),
[`spice/src/cli.rs`](../../spice/src/cli.rs), and
[`spice/src/cli/mounts.rs`](../../spice/src/cli/mounts.rs).

## App and transport

Rust/WASM owns the app; JS assets only render. `wire` owns DTOs. Epoch-bound,
authenticated sockets carry bounded pages/retention and correlate requests.
Paced retries and liveness probes recover half-open links; app generations are
immutable. The Spice icon replaces the connection pip: gray when unhealthy,
pulsing during connect/retry at the fire's 5s cadence (steady with reduced motion).

The Info seating form has one upper-right submit action. Label/control pairs
share intrinsic grid tracks, with right-aligned labels capped at `8rem` and
single-line ellipsis; the full label remains accessible and available on hover.
Two pairs fit per row at a panel content width of `32rem` or more; narrower
panels use one pair per row without changing submission or draft state.

Generations have eight shell files and optional `app/renderer` members from
[`wire`'s closed vocabulary](../../wire/src/manifest/renderer.rs). HTML inventory
binds exact byte digests into build identity: ≤512 files/64 MiB; absent inventory
means no renderers. Admission rejects undeclared, missing, mismatched or non-regular
members. Only generation-qualified renderer routes admit origin `null`; children
use restrictive CSP and hashed owned import maps. Parent CSP and Host/origin
checks remain. Publication retains prior receipt-owned members, not current ones.

Mermaid uses full 12.0.0 ESM, not Tiny: 105 unchanged upstream modules, owned
`renderer-000.html` import map and `renderer-000.mjs` bridge. The same bridge
uses the checked-in, generation-pinned KaTeX 0.16.47 module already present in
that bundle. [`renderer-000.txt`](../../app/renderer/renderer-000.txt) records
tarball integrity, original members, renamed-file checksums, licenses and source
revisions. Reproduce recorded filenames and original `dist/` path/basename
import aliases. Builds use checked-in bytes; no runtime CDN/package requests.

Pulldown-cmark owns `$...$` and `$$...$$` math. The app maps model-common
`\(...\)` and `\[...\]` delimiters onto that parser only when its first pass
exposes both delimiters as prose; escaped text and inline or fenced code remain
literal. KaTeX produces native MathML with system fonts. The renderer wraps it
in bounded SVG, and the parent admits only the decoded image, never renderer
markup in its DOM. Invalid TeX retains its authored source.

[`mermaid.rs`](../../app/src/browser/mermaid.rs) owns a lazy, measurable,
offscreen `allow-scripts` sandbox pinned to the **loaded page's** generation and
effective spice theme. Protocol v3 checks window, origin, generation, render
kind, channel, request ID, revision and theme. Strict child security locks host
configuration, never invokes click bindings, normalizes SVG namespaces and
clears temporary nodes. The parent admits ≤8 MiB UTF-8 SVG as a base64 image,
never SVG nodes/styles in its DOM.

Limits: 50,000 UTF-16 source units, 500 error characters. Rendering is serial;
identical requests share work, pending revisions coalesce over 80 ms. Queue:
16 jobs/1 MiB source, 64 subscribers. LRU: 32 source/theme/generation entries,
16 MiB source+encoded images. Superseded/detached cards receive no late repaint.
Card-owned slots preserve shared sanitization and ordinary code. Failures retain
source, last valid same-theme preview and retry. Teardown removes listeners/frame,
cancels decodes and clears queue/cache. The 30-second deadline needs a running
event loop: **not CPU isolation or a hard timeout**. Loaded modules work offline;
reopening the iframe needs the local appliance.

Diagrams reserve full-width `cover` slots: 15.75rem media-only, 8.75rem mixed.
A tool card whose face is one image uses the same standard 15.75rem square as
an operator's lone image, cover-cropped, and carries no card controls: a click
on the image opens it. Like prose, a right-aligned signoff of badges, age, then
lane name closes it; once the card commits, the square stays centred both ways
in the room above that signoff, so it never resizes the committed card. A lane
reading back an operator's steering attachment gets no second image card: that
image already stands with its directive, and repeating it on the leading edge
would push the acknowledgment out of view. Operator messages and quotes keep
their floors.
Source/error stays in-place. All images share an inert modal over the whole
viewport: the interface beneath shows through the shared frosted veil, the image
sits at natural size capped to the viewport, and Escape, Close, or a click on the
frost closes it without reflow. Copy writes PNG; Download keeps originals,
including SVG. Exports need data/same-origin access; unavailable/refused actions
are explained.

Lane/team topology and attribution remain server-owned product invariants.
Reading, motion, scroll, no-launch behavior, and composer-draft restoration bind
the browser client. Drafts survive lane reorder and renewal within the live
client; browser death may discard them.

Authorities: [`wire/src/stream.rs`](../../wire/src/stream.rs),
[`serve/src/stream.rs`](../../serve/src/stream.rs),
[`app/src/browser.rs`](../../app/src/browser.rs),
[`app/src/runtime.rs`](../../app/src/runtime.rs), and
[`app/src/mosaic.rs`](../../app/src/mosaic.rs). Browser-edge behavior is verified
through disposable actual-surface experiments; the retained app checks exercise
the pure Rust client contracts.

## Mosaic implementation boundary

Mosaic's Rust-owned lineage reduction, layout, geometry, history, image
settlement, motion, and performance contracts live in the
[Mosaic Rust implementation boundary](mosaic-rust.md). The app retains one
event-driven implementation path; it does not preserve a predecessor
compatibility mode.

## Steering and warnings

Durable direction and semantic retirement follow the product loop. Filesystem
inboxes are delivery materializations; journal facts own lifecycle history.
Lease warnings are non-authoritative command-stderr decoration produced by the
working-state meter. The meter is read-only with respect to journal, task, and
lease authority, but may durably publish three disposable suppression, owner,
and loss records beneath worktree runtime state: `session-meter.json`,
`session-meter-owner.json`, and `session-meter-loss.json`. The resident never
appends a warning fact or turns a warning into steering. Decodable warning facts
with no producer are retained journal evidence only.

Authorities: [`steer/src/directives.rs`](../../steer/src/directives.rs),
[`spice/src/session_meter.rs`](../../spice/src/session_meter.rs), and
[`spice/src/run_meter.rs`](../../spice/src/run_meter.rs).

## Installation and command authority

Install preview is read-only and apply is bound to its exact digest. The
canonical receipt directly records every reversible prior image, and no
receipt-owned asset or hook effect precedes its acceptance. Assets publish
before the common Git hook path switches. Install owns declared assets, receipt
state, and hook selection, and observes every linked worktree's effective hook
selection as an activation precondition. It never incidentally imports or
deletes foreign process, configuration, or state.

Reversal is strict. If a receipt-owned target no longer has the exact installed
identity, unapply returns a typed drift refusal, preserves the changed target and
current receipt, and publishes no terminal `uninstalled` status. After the
operator restores or deliberately reconciles the named evidence, the same
receipt resumes. An in-place refresh retains each operation's original
pre-first-install image and binds the installed image it supersedes; unapply
therefore restores the filesystem first observed and publishes the current
receipt terminal.

The current `0.0.0` product line uses receipt schema 17 as its sole executable
shape. Schemas 1–16 refuse before receipt-owned mutation and name the retired
release whose executable can unapply them. Installed app history is bounded to
the current generation and two ordered predecessors; older generations retire
inside the refresh transaction.

The compiled CLI exposes the sole Spice command vocabulary. The active harness
owns task, session, and steering state; an installed Spice bootstrap explicitly
authorizes an agent to use the corresponding commands. Native harness authority
remains in force otherwise. Merely finding the binary establishes no task,
lane, session, or claim.

Without `--json`, a reply whose command contract provides human text prints that
text directly. Structured reply bodies print their body JSON by default rather
than gaining an unrelated prose rendering. `--json` always prints the complete
typed outcome, including its reply position; selecting it changes only the
document shape, never the exit code or output stream. Refusals and faults remain
human-readable on standard error unless `--json` is selected.

Repository development commands that may touch Cargo use ambient `spice` and
its `dev cargo` or `dev xtask` surface. In supervised shells, tracked wrapper
policy resolves that word to the exact regular, non-symbolic Git-common
appliance. A missing or invalid asset is a hard stop; never select another
Cargo home.

Authorities: [`spice/src/repository.rs`](../../spice/src/repository.rs),
[`spice/src/install.rs`](../../spice/src/install.rs),
[`spice/src/cargo_isolation.rs`](../../spice/src/cargo_isolation.rs), and
[`spice/CARGO_ISOLATION.md`](../../spice/CARGO_ISOLATION.md).

## Release and proof

Release is a sealed two-target appliance. Each target payload binds source, the
native binary, app generation, and build pins. Proof replays the current
candidate checkout — read-only in the Linux container and through the attested
native runner on Darwin — and does not compare that workspace commit with the
payload source. Only a clean replay publishes a detached receipt. A post-proof
`spice release seal` composes both target payloads and receipts into one release
manifest, publishes its pointer last, and asks the resident to recognize that
exact manifest against the current clean commit and tree. The
publication preview requires the manifest source to name the already-versioned
release commit, then binds that manifest, package, notes, targets, and operations
with the identical source and release commit.
Runtime version surfaces (`spice --version`, `spice version`, and the app
menu footer) report the tracked version, with an installed pre-release extended
by its source commit stamp. The version never carries the executable digest:
the report's native-binary field names the running executable by its complete
domain-separated digest, which needs no Git checkout, generated build input, or
random value, so identical executable bytes keep the same identity. Ordinary
CLI commands do not hash the executable; explicit version reads and resident
identity construction stream it without retaining the whole file. Release
selection and sealing still use the tracked base version from `dist/VERSION`.


Target builds use a persistent release target directory. Package materialization
rechecks staged executable bytes against the payload inventory and must reproduce
the planned package and Git-tree identities. The compiled-host probe reads and
digests its staged source, follows any readable source shape, copies those bytes
to a regular plan-keyed file, applies executable mode to the copy, and accepts a
successful `--json version` process without decoding its output. Source and
package publication have explicit no-effect rehearsals; tag and announcement do
not, and the plan does not label them unrehearsable.

Fresh publish runs the complete canonical strict gate battery over the exact
source commit named by the accepted plan before changing the version or
crossing an external publication boundary. Tag-and-announce recovery instead
resolves preconditions and credentials, awaits registry visibility, creates the
immutable tag, and announces, in that order, without rerunning that gate arm.
The release root is explicit rather than ambient, a
completed red report is a refusal, and only admission followed by an unchanged
clean source can publish the plan-specific correctness receipt.

Authorities: [`dist/src/release.rs`](../../dist/src/release.rs),
[`proof/src/lib.rs`](../../proof/src/lib.rs), and
[`spice/src/release_command.rs`](../../spice/src/release_command.rs),
[`spice/src/release_execution.rs`](../../spice/src/release_execution.rs), and
[`spice/src/gate.rs`](../../spice/src/gate.rs).

## Review rule

Review the product invariant first and this profile second. Resolve every
disagreement in the same change by repairing Rust, refining the portable model,
or changing an explicit implementation choice here together with executable
evidence. Silence is not a disposition.
