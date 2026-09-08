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
equals birth order within that current generation, and the omitted vowels keep
a minted stamp out of prose-shaped words. The 52⁹ space spans about 88.09 years
from the Unix epoch; its most-significant digit advances about every 1.69 years,
the 2026 era begins with `d`, and the first instant outside the space is near
2058-02-02. An out-of-range instant refuses instead of wrapping. A collision
advances exactly one microsecond until the stamp is free; it never appends
randomness or entropy.

Retained eight-character base-52 Unix-millisecond stamps remain readable in
facts, heads, handles, CLI operands, origins, indexes, and Canvas presentation.
They are never minted by current creation. Mixed-width equality, hashing, and
ordered collections remain deterministic, but ordinary string order across
the two codec generations is not chronology; journal sequence and durable queue
origin remain that authority.

Steering uses the same nine-character microsecond codec for current keys and
identity-derived inbox seals. Parsing also admits retained eight-character
alphanumeric keys; an eight-character vowel-free key contributes its decoded
millisecond multiplied by 1,000 to the microsecond high-water, while an older
base-62 key outside the current alphabet contributes no invented time. Schema
14 migrates the rebuildable steering head/index representation once from
`minted_millis` to `minted_micros` without rewriting immutable facts.

Authority: [`wire/src/witness.rs`](../../wire/src/witness.rs). Executable
coverage lives in
[`spice/src/board_command/tests/task_listing.rs`](../../spice/src/board_command/tests/task_listing.rs).
Steering codec coverage lives in
[`steer/src/ack/keys.rs`](../../steer/src/ack/keys.rs),
[`steer/tests/scan.rs`](../../steer/tests/scan.rs), and
[`journal/src/migrate/steering_microseconds.rs`](../../journal/src/migrate/steering_microseconds.rs).

## Session boundary

Session reporting is daemonless and read-only. It briefs from admitted
transcript and repository evidence, reconstructs selected-horizon windows across
compaction, and reports message, phase, command, commit, and token views. It
never distils or persists completion learnings and owns no learning store.
Decodable fields that have no producer are inert input, not product behavior.

Authorities: [`spice/src/session_command.rs`](../../spice/src/session_command.rs),
[`wire/src/intent/task.rs`](../../wire/src/intent/task.rs), and
[`spice/src/repository_tests.rs`](../../spice/src/repository_tests.rs).

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

The journal accepts one schema and an ordered one-way migration chain. Wire and
browser compatibility have independent versions and resident epochs; runtime
behavior never branches by release.

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

## Canvas and transport

Rust/WASM is the sole production browser client. JavaScript is browser-test
code, not a second runtime. `wire` owns the complete protocol vocabulary; the
browser imports its closed DTOs and the resident serves one immutable app
generation. One authenticated, epoch-bound stream socket carries bounded pages
and retention with one correlation identity per request. Explicit reconnect
pacing and a liveness probe turn a half-open connection into the ordinary
reconnect path.

Lane/organization topology and attribution remain server-owned product invariants.
Reading, motion, scroll, no-launch behavior, and composer-draft restoration bind
the browser client. Drafts survive lane reorder and renewal within the live
client; browser death may discard them.

Authorities: [`wire/src/stream.rs`](../../wire/src/stream.rs),
[`canvas/src/stream.rs`](../../canvas/src/stream.rs),
[`app/src/browser.rs`](../../app/src/browser.rs),
[`app/src/runtime.rs`](../../app/src/runtime.rs), and
[`app/src/mosaic.rs`](../../app/src/mosaic.rs). Browser-edge coverage lives in
[`app/browser/canvas.spec.mjs`](../../app/browser/canvas.spec.mjs) and its
focused neighboring modules.

## Mosaic implementation boundary

The Mosaic behavior in the product model is carried forward through one current
path, not a predecessor compatibility mode. Typed `AgentMessage` values enter
the canonical runtime card reducer, which orders them by their chronology key,
deduplicates them by lane and locus, and applies the stream's bounded per-lane
retention. Member-local `MessageHistory` pages merge through that same reducer;
a response that does not name the lane's current cursor changes nothing. The
resulting lineage stream feeds one persistent plane whose pure Rust lattice owns
placement, freezing, liquid settlement, ripple, backfill, geometry replay, and
compaction-epoch replay. JavaScript only drives browser tests.

The predecessor's implementation details differ where the Rust product has a
stronger ownership boundary:

- One lineage plane merges all organization members instead of maintaining a
  JavaScript lane store. Every card retains explicit lane and author identity,
  and each member retains an independent typed history frontier, so a delayed
  member cannot stall or overwrite its siblings.
- Keyed Yew nodes replace imperative node ownership. Measurement is deferred
  until the host has a usable width, cached by card fingerprint, image
  revision, and width, and retained for only the current and immediately prior
  widths. A base measurement admits at most one wider candidate, so ordinary
  placement performs at most two layout reads per card.
- Yew renders integer layout diagnostics as attributes, then an effect copies
  changed values into CSS custom properties. This is the single CSP-safe
  geometry channel and avoids Firefox's unsupported typed `attr()` arithmetic.
  The copy compares the current property first, so unchanged card placement
  receives no style write.
- Scroll correction retains the nearest visible keyed card at its exact
  viewport offset instead of inferring the reader's position from the plane's
  maximum-row delta. This preserves the same 80px near-top policy while also
  covering nonuniform growth, backfill, and compaction-epoch replacement.
- Image load and error events settle through one card coordinator. A card is
  remeasured only after all of its images are terminal, and repeated terminal
  events do not publish another content revision. This removes the
  predecessor's per-image render cascade without changing placeholder or final
  geometry.
- The predecessor's two-layer positional tween is not transplanted into Yew.
  One effect lands plane properties before card properties, and liquid cards
  expose only a 180ms opacity transition; frozen cards expose no transition.
  Reduced motion removes that fade but never disables same-frame scroll
  correction. This keeps motion tied to real arrivals without exposing stale
  intermediate geometry or scroll overflow.
- The current surface uses the product's merged console hierarchy, card chrome,
  attribution, and responsive controls. The visual parity target is therefore
  the packed chronology, stable card geometry, media containment, compaction
  boundaries, readable loading state, and motion contract; predecessor product
  chrome and palette are not execution contracts.

All Mosaic production work is event-driven by an admitted stream frame, width
change, history-edge scroll, or image terminal event. A no-op reconcile returns
the existing plane, publication shares retained payloads, and keyed mode
switches neither rebuild nor remeasure the plane. The performance profile
exercises 75 and 768 retained cards and requires one cold commit, one read for
each ordinary card, zero card additions or removals on a mode round trip, zero
mode-switch commits, zero extra measurements, and zero transport frames.

Authorities: [`app/src/runtime/message_cards.rs`](../../app/src/runtime/message_cards.rs),
[`app/src/mosaic.rs`](../../app/src/mosaic.rs),
[`app/src/mosaic_plane.rs`](../../app/src/mosaic_plane.rs),
[`app/src/browser/watchband/mosaic_view.rs`](../../app/src/browser/watchband/mosaic_view.rs),
[`app/src/browser/watchband/mosaic_view/history.rs`](../../app/src/browser/watchband/mosaic_view/history.rs),
and
[`app/src/browser/watchband/mosaic_view/image_resolution.rs`](../../app/src/browser/watchband/mosaic_view/image_resolution.rs).
Executable parity and cost evidence lives in
[`app/tests/mosaic_sequences.rs`](../../app/tests/mosaic_sequences.rs),
[`app/tests/mosaic_stability.rs`](../../app/tests/mosaic_stability.rs),
[`app/browser/mosaic-history-pagination.mjs`](../../app/browser/mosaic-history-pagination.mjs),
[`app/browser/mosaic-integrated-proof.mjs`](../../app/browser/mosaic-integrated-proof.mjs),
and [`app/browser/startup-profile.mjs`](../../app/browser/startup-profile.mjs).

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

Install preview is read-only and apply is bound to its exact digest. Immutable
content-addressed rollback material may be staged before receipt acceptance, but
it has no installation authority until the canonical receipt authenticates it;
no receipt-owned asset or hook effect precedes that acceptance. Assets publish
before the common Git hook path switches. Install owns declared assets, receipt
state, and hook selection, and observes every linked worktree's effective hook
selection as an activation precondition. It never incidentally imports or
deletes foreign process, configuration, or state.

Reversal is strict. If a receipt-owned target no longer has the exact installed
identity, unapply returns a typed drift refusal, preserves the changed target and
current receipt, and publishes no terminal `uninstalled` status. After the
operator restores or deliberately reconciles the named evidence, the same
receipt resumes. An ordinary in-place refresh retains each operation's original
pre-first-install image and binds the installed image it supersedes; unapply
therefore restores the filesystem first observed and publishes the current
receipt terminal. Only migration from the admitted version-7 receipt
authenticates a digest-bound rollback snapshot, which may reside in a separately
stored blob. Reversing that migration restores the exact pre-migration assets
and version-7 receipt as one prior state, rather than running the older shape
through the current appliance.

The current `0.0.0` product line uses receipt schema 14. Historical install
support is schema-bounded: complete schemas 7, 8, 9, and 13 migrate directly to
14, while schemas 1–6 and 10–12 must be unapplied by their named retired owner
before the current installer proceeds.

The compiled CLI exposes the sole Spice command vocabulary. The active harness
owns task, session, and steering state; an installed Spice bootstrap explicitly
authorizes an agent to use the corresponding commands. Native harness authority
remains in force otherwise. Merely finding the binary establishes no task,
lane, session, or claim.

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
seal composes both target payloads and receipts into one release manifest; the
publication plan separately admits its source and release commits and the
manifest, package, notes, targets, and operations without equating the plan's
source commit with the manifest source.

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
