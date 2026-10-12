# Product model — supporting planes — Distribution

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Distribution

Distribution crosses authorities the appliance does not own outright: one
tracked Git release authority and an operator's repository. Immutable release
refs cannot be repaired by pretending an earlier effect did not happen, and
repository installation must preserve state the operator already owns. Exact
plans, durable receipts, convergence, and identity-aware reversal answer those
boundaries.

### External publication is a plan, not a verb

**Invariant —** Every release verb with externally visible publication effects
renders its complete ordered plan and performs none of those effects until
execution supplies that plan's exact digest. An absent or moved digest refuses
before effects; exact-plan execution is the only publication path. Local
mutations remain governed by their own transactional contracts.

**Invariant —** Plan identity covers the repository, action, version, source and
release commits, target matrix, ordered operations, notes content, artifacts,
proof receipts, sealed manifest, and captured preconditions. Step order is part
of it, because the ordering is the safety property. The correctness operation
means the complete canonical strict battery, never a mutable subset outside the
plan; a digest over step names alone would let an input change after approval.

**Invariant —** Preconditions that can be evaluated without effect are evaluated
while the plan is built, so a knowable failure never strands the operator after
an effect.

**Invariant —** Inspection is free of consequence. Checking the gates, drafting
the notes, and previewing the range never bump, commit, tag, push, or publish.

**Invariant —** Release documents are complete only as files. Bare
`spice release notes` prints a bounded summary, and `--output FILE` writes the
complete document. A publish preview retains the complete plan under the Git
common directory before it prints, names that file, and ends with the exact
`--apply=KEY` command; that key is the only accepted authority.

**Invariant —** One executor and one plan and receipt grammar serve preview,
execution, and recovery. The executor records an operation's exact prior state
before crossing its effect boundary and a typed receipt after convergence, and
resumption admits only the same plan and receipt prefix. A parallel
verification or publication body would be a second release authority.

### A versioned commit is sealed before publication

**Invariant —** Version activation is an ordinary task landing. Both release
actions require clean HEAD at the release commit, already integrated in the
tracked source branch, with requested, compiled, and tracked product versions
in agreement; every other shape refuses. Release never edits `dist/VERSION`,
synthesizes a commit, or infers task ownership from the board, and a red
candidate publishes nothing.

**Invariant —** Fresh publish executes the complete canonical strict battery
over the exact source commit the plan names, and every arm must admit. This
host-side battery is distinct from sealed target proof replay. Tag-and-announce
recovery instead resolves preconditions and credentials, awaits registry
visibility, then tags and announces, without rerunning the battery or
republishing the package.

**Invariant —** The artifact gate proves composition and use without rebuilding
after authorization. The package materializes from sealed content-addressed
objects and must equal the previewed package and tree identities. The
compiled-host executable named by the sealed inventory must match the plan's
artifact digest, and a plan-keyed executable copy of it must succeed at
`--json version`.

### Proof is sealed per target and composed last

**Invariant —** A release has exactly two target payloads, in canonical order:
`aarch64-apple-darwin` and `x86_64-unknown-linux-musl`. Each receipt-free payload
binds its target, source identity, app generation, native executable, build
pins, and artifact inventory, and both bind the source digest of the
already-versioned release commit.

**Invariant —** Proof replays the complete current battery against the
candidate checkout: Linux mounts it read-only in the pinned OCI environment,
Darwin passes it to the attested native runner. Replay is bound to payload,
environment, and battery, but does not prove that the checkout equals the
payload's source digest. Unsupported hosts, drift, infrastructure failure, or a
non-clean replay publish no receipt; a refused replay is a typed refusal.

**Invariant —** A proof receipt is detached and canonical. Admission requires
target, payload digest, environment, battery digest, replay digest, and clean
standing to form one identity chain.

**Invariant —** The release manifest is post-proof: both targets with clean,
mutually consistent receipts, or no release. Content-addressed objects publish
before the sole manifest pointer, so a reader sees the prior or the new complete
seal, never a pointer to missing proof.

### Notes are read out of the ledger

**Invariant —** Notes derive from the ledger; there is no maintained changelog.
The window runs from the highest-versioned prior tag reachable from the release
commit, addressed through its fully qualified tag ref so a same-named branch
cannot redefine it. With no reachable tag, a bounded window of recent landings
names that basis. Only first-parent landings count, so one merged unit of work
is one entry.

**Invariant —** Work added and reverted inside one window is suppressed on both
sides. A revert pairs with the landing whose history contains the reverted
commit, not one whose identifier equals it, because the reverted commit usually
arrived on a side branch.

**Invariant —** Landings sharing a task collapse into one entry at the task's
first landing; its latest landing supplies the subject and project. Entries
group by that declared project, never by touched paths, with an empty or
`agent.*` declaration mapped to `general`. Buckets appear in first-seen order and
keep entry order; equal descriptions merge only within a bucket and retain every
reference.

**Invariant —** A trailing handle is stripped from a description only when it
matches that commit's own recorded identifier.

**Invariant —** Curation owns the region above the generated inventory. Plan
building regenerates the inventory against the release commit, so a curated
file never publishes a stale range. Notes refuse when untouched, when they still
carry the placeholder, or when the curated region holds only banner and
headings.

### The publication sequence

**Invariant —** The only external release provider is the tracked Git authority:
remote `origin`, source `refs/heads/main`, package
`refs/spice/packages/vVERSION`, tag `refs/tags/vVERSION`, and announcement
`refs/spice/releases/vVERSION`. Runtime configuration cannot redirect a step.

**Invariant —** Publication never moves the source branch. The package ref
converges first through a guarded no-effect rehearsal; tag and announcement
converge to the release commit only after bounded observation sees the exact
package object. Exhausting that observation is a terminal refusal, not
permission to advertise unobservable content.

**Invariant —** A version names exactly one package object and one release
commit. A package, tag, or announcement ref pointing elsewhere refuses rather
than moving; a matching ref is already converged and left untouched. An
interrupted release is completed by re-running it from the first incomplete
boundary, never unwound.

**Invariant —** An operator interruption terminates with a status distinct from a
gate failure.

### Installing into a repository the product does not own

One canonical status-bearing receipt owns the exact plan, fixed-size references
to first-captured prior images, durable progress, and reversal outcomes. The
referenced raw bytes live once in a receipt-owned content-addressed store.

**Invariant —** What the operator reviews is produced purely by observation.
Apply requires that plan's exact digest, re-resolves the plan and executable
repository configuration, and refuses before mutation when either moved. A
process that derives and applies its own plan, such as the development refresh,
may carry it into apply but still proves every bound source, generation,
destination, and receipt image before effect.

**Invariant —** Preview never invokes maintenance. After admission, apply and
`--unapply` may run one bounded pass that reclaims only recognized stale
install-record publications; it is not a plan operation and never imports,
repairs, or deletes foreign state.

**Invariant —** Before the first destination effect, apply atomically accepts
one canonical `applying` receipt with the exact plan, repository identity, zero
progress, and fixed-size prior-image references, then durably publishes every
referenced raw payload before changing a destination. No plan change happens
until the receipt describes it, and each completion is durably published before
the next mutation, so a crash leaves at most the recorded frontier ahead of its
receipt. Terminal `complete` is published only after every stage completes; an
interrupted apply resumes only the same plan.

**Invariant —** The receipt records the exact prior image the installation may
restore: a fixed-size mode/content reference or absence; prior configuration
values or absence; and whether it introduced a parent directory. Referenced
regular-file bytes are stored once outside the repeatedly rewritten receipt,
under their domain-separated content identity. Prior state is captured once, at
first application, and never re-derived from the installation's own output.
Recording only the new state would make absence unrecoverable.

**Invariant —** Installed app history is bounded. A refresh retains the current
generation and the newest predecessors within the bound and retires every other
installed generation in the same transaction, discarding each file only while it
still holds the installed image.

**Invariant —** Generated artifacts live in a directory the product owns and are
activated by redirecting the host tool there, never by writing into the location
the repository already uses. Every generated artifact carries an ownership
digest, so an untouched installation is distinguishable from a locally modified
one and from a newer generated shape.

### Reversal is a claim about identity

An uninstall that reverts by name destroys operator edits. Reversal restores
only what it can still recognize as its own.

**Invariant —** `--unapply` is the explicit reversal direction and has no
preview digest. It advances the exact `complete` receipt generation to
`unapplying`, runs the exact inverse of the canonical application order, and
publishes `uninstalled` only after every operation has a recorded outcome. A
moved generation refuses and preserves the current receipt.

**Invariant —** Identity drift is a typed refusal, not a partial success: it
overwrites nothing, preserves the changed target and the receipt, and retry
resumes the same receipt. A container the installation created is removed only
when still empty.

**Invariant —** An interrupted reversal blocks a new installation until it is
resumed, so two plans never contend for one receipt's first-captured prior
state.

**Invariant —** Both directions are resumable and idempotent: a re-run performs
only operations not yet recorded complete, and a plan already satisfied against
unchanged state performs no receipt-owned effect.

### The canonical receipt

The canonical receipt is the sole installation-status, transaction, and
reversal authority.

**Invariant —** Every receipt generation is canonical and published atomically
through a real non-symbolic file, only while the exact prior generation and
complete image still match; a stale or concurrent writer refuses.

**Invariant —** An unreadable, truncated, noncanonical, linked, or inconsistent
receipt refuses. Reading damaged provenance as "not installed" would strand
active modifications with no owner and no path to removal.

**Invariant —** Every receipt is bound to the repository it describes. A
nonterminal receipt refuses when it names another Git-common owner, as does a
terminal copy while its original owner still exists. A `complete` or
`uninstalled` receipt may rebase only after the old owner is gone, moving
repository, plan, and activation bindings together.

**Invariant —** The receipt remains after clean terminal directions, retaining
its plan, prior-image references, status, and progress. Its referenced raw
payloads remain available for the receipt's next executable direction. Absence
is never synthesized by deleting provenance.

**Invariant —** Install support is bounded by receipt schema, not product
version. The current schema is the sole executable shape. Each retired schema
refuses before receipt-owned mutation and names the release change whose
executable can unapply it, recorded in the landing that retires it. Malformed,
inconsistent, and future receipts are store failures, never retired state.

---
