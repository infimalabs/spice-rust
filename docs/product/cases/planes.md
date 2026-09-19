# Case battery — Supporting planes

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### Checks

| Situation | Expected | Ref |
| --- | --- | --- |
| New file exceeds base but remains at or below its elastic ceiling | admitted and unlatched; accepted publication enters time-boxed claim arbitration | [Flex is headroom, not a new limit](../invariants/checks.md#checks-flex-is-headroom-not-a-new-limit) |
| The same actor measures that file over base but within elastic again | remains unlatched; accepted publication refreshes the time-boxed claim without changing its original creation time | [Flex is headroom, not a new limit](../invariants/checks.md#checks-flex-is-headroom-not-a-new-limit) |
| A file exceeds its elastic ceiling while a peer holds its accepted claim | the finding is an informational redirect; an admitting publication may make the proposed latch durable | [Flex is headroom, not a new limit](../invariants/checks.md#checks-flex-is-headroom-not-a-new-limit), [A finding whose subject is held by a peer informs and passes](../invariants/checks.md#checks-a-finding-whose-subject-is-held-by-a-peer-informs-and-passes) |
| File shrinks under base in an accepted partial run | the observed latch heals with the accepted state; omitted latches that survive whole-ledger run-start rename, liveness, and expiry maintenance remain unchanged | [Accepted reconciliation is scoped](../invariants/checks.md#checks-accepted-reconciliation-is-scoped), [A healed latch retires only with an accepted run](../invariants/checks.md#checks-a-healed-latch-retires-only-with-an-accepted-run) |
| Two agents hit the same hot file | both judge the shared accepted ledger; provenance-seeded jitter and an accepted time-boxed claim coordinate the slice | [Sticky latches and live claims are repository-shared](../invariants/checks.md#checks-sticky-latches-and-live-claims-are-repository-shared), [Ceilings are jittered per path and resolved provenance](../invariants/checks.md#checks-ceilings-are-jittered-per-path-and-resolved-provenance) |
| Changed inherited debt remains inside its elastic ceiling | may pass unlatched and enter accepted claim arbitration | [Inherited debt retains its exact bound](../invariants/checks.md#checks-inherited-debt-retains-its-exact-bound) |
| A staged improvement changes debt already beyond the committed ratchet but remains above base | held to base and refused; improvement alone does not restore elastic headroom | [Inherited debt retains its exact bound](../invariants/checks.md#checks-inherited-debt-retains-its-exact-bound) |
| Another actor reads an accepted latch | the same repository latch holds the subject at base; any informational redirect comes from the distinct accepted claim | [Sticky latches and live claims are repository-shared](../invariants/checks.md#checks-sticky-latches-and-live-claims-are-repository-shared), [A finding whose subject is held by a peer informs and passes](../invariants/checks.md#checks-a-finding-whose-subject-is-held-by-a-peer-informs-and-passes) |
| Wide measure unchanged but high | passes | [Wide measures diff against a baseline](../invariants/checks.md#checks-wide-measures-diff-against-a-baseline) |
| Wide measure rises by one | refused | [Wide measures diff against a baseline](../invariants/checks.md#checks-wide-measures-diff-against-a-baseline) |
| A study reports a finding the operator wants tracked | the study files nothing; the operator captures it with `spice task add` | [A study reports and never files](../invariants/checks.md#checks-a-study-reports-and-never-files) |
| A gate blocks a merge | fixed at the source, never by weakening the gate | [A gate is never closed by weakening it](../invariants/checks.md#checks-a-gate-is-never-closed-by-weakening-it) |

### Session and counsel

| Situation | Expected | Ref |
| --- | --- | --- |
| Agent compacts mid-task | briefing restores ask, delivery, working set, pending steering | [The briefing answers four questions mechanically](../invariants/session.md#session-the-briefing-answers-four-questions-mechanically) |
| Agent is renewed | successor gets the same briefing surface | [The briefing answers four questions mechanically](../invariants/session.md#session-the-briefing-answers-four-questions-mechanically) |
| No pending asks | `authorities.asks` is absent rather than an empty array | [Only the two optional authority arrays omit when empty](../invariants/session.md#session-only-the-two-optional-authority-arrays-omit-when-empty) |
| A window is unreadable | reported loudly | [Windows the engine could not read are reported loudly](../invariants/session.md#session-windows-the-engine-could-not-read-are-reported-loudly) |
| Recovery text is only assistant prose | not rendered as the user's ask | [Recovery shows the recovered ask](../invariants/session.md#session-recovery-shows-the-recovered-ask) |
| The same command observes an identical state key repeatedly | meter line stays suppressed until the state key changes, including at the next claim-lease boundary | [The meter announces a durable state-key change](../invariants/session.md#session-the-meter-announces-a-durable-state-key-change) |
| Maxim fires twice on the same content in one epoch | one reminder | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| Compaction occurs, then the same content recurs | eligible again; text unchanged | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| The system quotes its own reminder | not judged | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| Judging is disabled | a configured bag hit publishes directly | [Adjudication is optional](../invariants/session.md#session-adjudication-is-optional) |
| Judging is enabled but the process is absent, times out, errors, or exhausts ambiguous replies | the candidate is abandoned and emits nothing; enabled judging never widens into direct publication | [Adjudication is optional](../invariants/session.md#session-adjudication-is-optional) |
| A maxim is confirmed | returns as ordinary steering | [A confirmed maxim reminder returns as ordinary steering](../invariants/session.md#session-a-confirmed-maxim-reminder-returns-as-ordinary-steering) |

### Metrics

| Situation | Expected | Ref |
| --- | --- | --- |
| One decoded record carries several usage events | every normalized sample is retained with its exact spans and ordinal; the record is not also counted as non-usage | [Transcript accounting retains typed usage and record counts](../invariants/session.md#metrics-transcript-accounting-retains-typed-usage-and-record-counts) |
| A complete decoded record carries no usage | its batch increments `non_usage_records` | [Transcript accounting retains typed usage and record counts](../invariants/session.md#metrics-transcript-accounting-retains-typed-usage-and-record-counts) |
| A decoded record is residue | its batch increments `residue_records` | [Transcript accounting retains typed usage and record counts](../invariants/session.md#metrics-transcript-accounting-retains-typed-usage-and-record-counts) |
| A transcript is replaced under the same path | ingestion restarts at member 1, offset 0 and names the superseded source generation | [A source head is generation and position, not an offset](../invariants/session.md#metrics-a-source-head-is-generation-and-position-not-an-offset) |
| Ingestion fails before its journal transaction commits | neither the batch nor the advanced head crosses; the same continuation remains eligible on retry | [An accounted batch and its advanced source head commit together](../invariants/session.md#metrics-an-accounted-batch-and-its-advanced-source-head-commit-together) |
| A live pass reads records under the accounting bound | the records are presented and held; no batch and no head movement cross yet | [A held span costs a re-read, never a record](../invariants/session.md#metrics-a-held-span-costs-a-re-read-never-a-record) |
| A pass reads a record carrying usage, or the seat closes | the held span is accounted at once as one batch spanning every held record | [A held span costs a re-read, never a record](../invariants/session.md#metrics-a-held-span-costs-a-re-read-never-a-record) |
| A resident stops while a span is held | the next start resumes from the stored head and reads the held records again | [A held span costs a re-read, never a record](../invariants/session.md#metrics-a-held-span-costs-a-re-read-never-a-record) |

### Task documents

| Situation | Expected | Ref |
| --- | --- | --- |
| Canonical document text is classified and emitted again | byte-identical fixed point | [The task-document properties](../interfaces.md#behavioral-properties) |
| `doc ledger HANDLE` renders a dialect-representable task | emits its current task prose, flow, authored tags, priority, and append-only notes without exporting a family or dependency graph | [The task-document Rust surface](../interfaces.md#the-rust-surface) |
| `doc ledger HANDLE` sees a task whose flow is outside the dialect vocabulary | reports `doc::ledger-unrepresentable` rather than inventing a document | [The task-document Rust surface](../interfaces.md#the-rust-surface) |
| Apply twice | idempotent | [The task-document properties](../interfaces.md#behavioral-properties) |
| Apply is interrupted before or after commit | no partial family exists; retry commits once or returns the existing receipt | [The task-document properties](../interfaces.md#behavioral-properties) |
| The board changes after an apply invocation captures its board barrier | apply refuses the moved barrier before mutation | [The task-document properties](../interfaces.md#behavioral-properties) |
| A changed document carries a slug already present in `task.document` | edits that task; the new document digest does not create an origin-scoped duplicate | [Identity within a document](../interfaces.md#identity-within-a-document) |
| Document contains a cycle | refused with the cycle named | [The task-document properties](../interfaces.md#behavioral-properties) |
| Hostile input: BOM, CRLF, deep nesting | parses or refuses; never crashes | [The task-document properties](../interfaces.md#behavioral-properties) |
| Acceptance, authored tags, priority, or an explicit dependency is absent from an edited document | prior authored value is removed; apply is replacement, not patch | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
| Flow changes in a document that matches an existing task | existing workflow flow and phase remain unchanged | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
| Description or annotations are imported from a document | description follows replacement semantics; annotations append as deduplicated task notes and remain recoverable with later notes through `doc ledger` | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
| A task document names a flow phase outside `design`, `plan`, `todo`, `verify`, and `review` | classification refuses it; this closed dialect grammar does not constrain ordinary board flows | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
| A refusal fires | nothing was written; when a repair exists, its exact action leads the diagnostic | [A refusal that can name a way out leads with the exact repair action](../invariants/distribution.md#conduct-a-refusal-that-can-name-a-way-out-leads-with-the-exact-repair-action) |

### Extension and wire

| Situation | Expected | Ref |
| --- | --- | --- |
| A resident producer and application consumer disagree on a wire type | compilation or the selected compatibility gate fails | [One wire vocabulary is compiled through every producer and consumer](../invariants/session.md#extension-one-wire-vocabulary-is-compiled-through-every-producer-and-consumer) |
| An emitter produces a payload violating the schema | caught at the emitter | [Payloads are validated at their emitters](../invariants/session.md#extension-payloads-are-validated-at-their-emitters) |
| A reader tries to take a field off the wrong union arm | a type error, before narrowing | [An answer with two shapes is two types](../invariants/session.md#extension-an-answer-with-two-shapes-is-two-types) |
| A mount collides with a builtin at any depth | refused loudly | [Built-in verbs and registered actions win at every depth](../invariants/session.md#extension-built-in-verbs-and-registered-actions-win-at-every-depth) |
| An extension declares a new vendor | available without editing a dispatcher | [A new vendor, wrapper, or gate is a declared value](../invariants/session.md#extension-a-new-vendor-wrapper-or-gate-is-a-declared-value) |
| Configuration value queried | names its source | [Every effective configuration value carries its source provenance](../invariants/session.md#extension-every-effective-configuration-value-carries-its-source-provenance) |
| Command optimizer unavailable or malformed | native command runs; health reported non-blocking | [The command optimizer is optional](../invariants/session.md#extension-the-command-optimizer-is-optional) |

### Distribution and observation

| Situation | Expected | Ref |
| --- | --- | --- |
| A fresh publish's release battery cannot execute every canonical arm over the plan's source commit | the correctness operation refuses; omission cannot become a release receipt | [The release gate runs the complete strict battery over one immutable source](../invariants/distribution.md#distribution-the-release-gate-runs-the-complete-strict-battery-over-one-immutable-source) |
| Range contains an implement-and-revert pair | suppressed from highlights | [Implement-and-revert pairs in one range are suppressed](../invariants/distribution.md#distribution-implement-and-revert-pairs-in-one-range-are-suppressed) |
| Gate check, notes draft, and range preview all run | nothing bumped, committed, tagged, pushed, or published | [Inspection is free of consequence](../invariants/distribution.md#distribution-inspection-is-free-of-consequence) |
| Install unapplied | reverses exactly the canonical receipt and retains that receipt with terminal status | [An install has one canonical status-bearing receipt](../invariants/distribution.md#distribution-an-install-has-one-canonical-status-bearing-receipt) |
| Observation run over foreign sessions | no repo, team, claim, hook, or steering writes | [Observation initializes nothing](../invariants/distribution.md#conduct-observation-initializes-nothing) |
| A user stops at the observation tier | fully usable there | [Each capability tier is independently usable](../invariants/distribution.md#conduct-each-capability-tier-is-independently-usable) |

### Identity and authority

| Situation | Expected | Ref |
| --- | --- | --- |
| A task is re-homed to another project | its rendered handle changes; its stored identity does not | [A task's `born` stamp is its only stored identity](../invariants/identity.md#identity-a-task-s-born-stamp-is-its-only-stored-identity) |
| Current-generation `born` stamps are sorted as plain strings | the order matches current-generation birth order; journal sequence remains cross-generation chronology | [The current stored stamp orders current births without parsing](../invariants/identity.md#identity-the-stored-stamp-orders-births-without-parsing) |
| Many tasks are minted at the same represented instant | each gets a distinct, still-ordered stamp | [Collision resolution preserves stamp order](../invariants/identity.md#identity-collision-resolution-preserves-stamp-order) |
| A stream frame is delivered twice at the same sequence | applied once; the duplicate cannot advance the cursor | [Stream sequence is monotone](../invariants/identity.md#identity-stream-sequence-is-monotone) |
| The resident restarts while a client holds a cursor | the changed epoch rejects resume and yields one authoritative snapshot | [A resume cursor belongs to one exact stream](../invariants/identity.md#identity-a-resume-cursor-belongs-to-one-exact-stream) |
| A frame skips a sequence | consumer resumes the exact gap or reconnects for a snapshot; it never guesses | [Stream sequence is monotone](../invariants/identity.md#identity-stream-sequence-is-monotone) |
| A client from another application generation presents a cursor | refused and reconnected against the admitted generation | [The authenticated stream carries generation, epoch, and sequence](../invariants/identity.md#identity-the-authenticated-stream-carries-generation-epoch-and-sequence) |
| Snapshot setup races an accepted journal fact | the fact appears exactly once, in the snapshot head or the next frame | [One epoch-bound stream carries snapshot and resume](../invariants/identity.md#identity-one-epoch-bound-stream-carries-snapshot-and-resume) |
| Rebuildable index rows are deleted | rebuilt from immutable facts without mutating those facts or their heads | [Durable facts and heads remain distinct from rebuildable index rows](../invariants/identity.md#identity-durable-facts-and-heads-remain-distinct-from-rebuildable-index-rows) |
| A projection's source no longer reaches back far enough | it says so rather than answering from nothing | [A projection family must declare its source](../invariants/identity.md#identity-a-projection-family-must-declare-its-source) |
| An authority store of an admitted historical shape is opened | its sole declared direct or sequential route applies once to reach the current shape; no historical runtime remains active | [Authority migrations have one unambiguous forward route to current](../invariants/identity.md#identity-authority-migrations-have-one-unambiguous-forward-route-to-current) |
| An authority store of an unknown or inconsistent shape is opened | refused without migrating or rewriting that authority; independent explicitly admitted pre-read maintenance may still mutate only the stale temporary state it owns | [Authority migrations have one unambiguous forward route to current](../invariants/identity.md#identity-authority-migrations-have-one-unambiguous-forward-route-to-current) |
| Transcript usage is accounted | typed decoder events supply normalized samples with exact source loci; non-usage and residue remain counts | [Transcript accounting retains typed usage and record counts](../invariants/session.md#metrics-transcript-accounting-retains-typed-usage-and-record-counts) |
| A consumer needs a fact another plane owns | it asks the owner rather than re-deriving | [Every fact has exactly one authority](../invariants/identity.md#identity-every-fact-has-exactly-one-authority) |

### Work, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| A claim is taken | activity marker and claim metadata are set together | [Claiming is atomic](../invariants/identity.md#work-claiming-is-atomic) |
| A claim is inspected after the fact | its bounded lease and exact opening and closing source boundaries are reconstructible | [Claims carry a bounded lease and an exact source boundary](../invariants/identity.md#work-claims-carry-a-bounded-lease-and-an-exact-source-boundary) |
| Two candidates tie on graph rank | locality breaks the tie, and only then | [Locality is the final tie-break and never overrides the graph](../invariants/identity.md#work-locality-is-the-final-tie-break-and-never-overrides-the-graph) |
| A task is filed through a conventional authored or typed system route | it retains the corresponding `ack:`, `task:`, `gate:`, or `doc:` origin realm | [Every task names an origin realm](../invariants/identity.md#work-every-task-names-an-origin-realm) |
| An authored task supplies another nonempty `class:value` origin | the open wire shape is accepted and preserved; no closed realm enum is imposed | [Every task names an origin realm](../invariants/identity.md#work-every-task-names-an-origin-realm) |
| A phase advance is replayed from history | it is recoverable without a parallel event log | [A phase advance is a publication](../invariants/identity.md#work-a-phase-advance-is-a-publication) |
| A review finds nothing | the phase advances; no merge is produced | [A review that changes nothing still advances the phase](../invariants/identity.md#work-a-review-that-changes-nothing-still-advances-the-phase) |
| An agent files an admission of a mistake | it lands on a hidden channel, off the default allocation queue but fully readable | [Hidden channels keep admissions off the queue](../invariants/identity.md#work-hidden-channels-keep-admissions-off-the-queue) |
| A claim begins | the tree is fast-forwarded to the baseline first | [Claim synchronization](../invariants/lifecycle.md#integration-claim-synchronization) |
| An agent hits an integration conflict | it is described as an overlap with the baseline | [Content conflicts are framed as baseline overlap](../invariants/lifecycle.md#integration-content-conflicts-are-framed-as-baseline-overlap) |

### Lifecycle, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| Two capacity scans overlap | selection, claim, and start remain one serialized decision | [Capacity, selection, claim, and start are one serialized decision](../invariants/lifecycle.md#lifecycle-capacity-selection-claim-and-start-are-one-serialized-decision) |
| One worktree is deciding while another needs to | siblings proceed independently | [Lifecycle decisions for one worktree are serialized](../invariants/lifecycle.md#lifecycle-lifecycle-decisions-for-one-worktree-are-serialized) |
| An agent is renewed | the request travels as ordinary steering | [Renewal is ordinary steering](../invariants/lifecycle.md#lifecycle-renewal-is-ordinary-steering) |
| The reconciler restarts | no lane snapshot was persisted; state is re-read | [The lifecycle reconciler is ephemeral](../invariants/lifecycle.md#lifecycle-the-lifecycle-reconciler-is-ephemeral) |
| A lane not permitted to expand holds a claim | the held-claim arm still applies to it | [The held-claim arm applies to every driven lane](../invariants/lifecycle.md#lifecycle-the-held-claim-arm-applies-to-every-driven-lane) |
| A lane is restarted onto held work | it recovers the task, and is not treated as a wake | [The restart recovers the task, not the session](../invariants/lifecycle.md#lifecycle-the-restart-recovers-the-task-not-the-session) |

### Topology and surface, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| A team holds several worktrees | each worktree remains its own lane and send address; the team groups them in one console | [A team groups one or more lanes](../invariants/lifecycle.md#topology-a-team-groups-one-or-more-lanes) |
| One browser chooses `Close team` | that mounted browser hides the console; topology and other browsers are unchanged | [Close team is browser-local presentation](../invariants/lifecycle.md#topology-close-team-is-browser-local-presentation) |
| That browser chooses **Agents → Open team** for a closed grouped team | the console returns during the same mount; no authority command or topology change occurs | [Close team is browser-local presentation](../invariants/lifecycle.md#topology-close-team-is-browser-local-presentation) |
| New content arrives | it places by insert; nothing already placed is re-decided | [New content places by insert](../invariants/lifecycle.md#surface-new-content-places-by-insert) |
| A neighbour's content grows | untouched cards keep their column and span | [A card's column and span latch between geometry replays](../invariants/lifecycle.md#surface-a-card-s-column-and-span-latch-between-geometry-replays) |
| Any motion is observed | it corresponds to a change, not to a re-render | [Movement means change](../invariants/lifecycle.md#surface-movement-means-change) |
| Compensation adjusts the viewport | it is not read as a user gesture | [Programmatic scroll is excluded from anything that reads user intent](../invariants/lifecycle.md#surface-programmatic-scroll-is-excluded-from-anything-that-reads-user-intent) |
| Several changed stream frames arrive in one tick | each frame immediately publishes the resulting surface state | [Each changed stream frame publishes immediately](../invariants/lifecycle.md#surface-each-changed-stream-frame-publishes-immediately) |
| More lane identities exist than palette slots | colours repeat; explicit lane attribution remains unambiguous | [Accent derives from stable lane identity](../invariants/lifecycle.md#surface-accent-derives-from-stable-lane-identity) |
| Two clients render the same fused team | stable lane-derived accents and explicit attribution agree independently of member order | [Member order controls composition, not identity](../invariants/lifecycle.md#topology-member-order-controls-composition-not-identity) |

### Checks, session, extension, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| A study runs from the command surface with flags | the commit gate still runs the defaults | [Direct study commands may take flags](../invariants/checks.md#checks-direct-study-commands-may-take-flags) |
| Tracked policy input attempts to name a packaged-arm program or revive a retired packaged arm | it has no packaged executable authority; compiled declarations and the packaged battery manifest remain authoritative, while tracked `policy.pre_commit` command argv stays a separate captured capability | [Check settings have partitioned authority](../invariants/checks.md#checks-check-settings-have-partitioned-authority) |
| A test-only public symbol is introduced | refused | [Some ceilings are absolute zero](../invariants/checks.md#checks-some-ceilings-are-absolute-zero) |
| A test asserts absence where a present property exists | flagged in favour of the positive assertion | [Tests assert present, observable properties in preference to absence](../invariants/checks.md#checks-tests-assert-present-observable-properties-in-preference-to-absence) |
| Transcript prose and steering disagree about the ask | steering remains authoritative; transcript prose never supplies or backfills the ask | [Asks are sourced from steering and recovered compaction intent](../invariants/session.md#session-asks-are-sourced-from-the-steering-plane) |
| A briefing spans many compaction windows | the briefing requests the newest three boundaries; a syntactically valid nonpositive sweep count normalizes to one and larger process-representable counts have no Rust-side ceiling | [A briefing requests three boundaries and a sweep normalizes its caller count](../invariants/session.md#session-a-briefing-requests-three-boundaries-and-a-sweep-normalizes-its-caller-count) |
| The transcript decoder admits two dialogue entries | the report emits both in canonical turn and admitted dialogue order without a second report-layer fold | [The report preserves admitted turn and dialogue order](../invariants/session.md#session-the-report-preserves-admitted-turn-and-dialogue-order) |
| Generated text resembles authored prose | the classifier separates them structurally | [Message-shape classification is deterministic and structural](../invariants/session.md#session-message-shape-classification-is-deterministic-and-structural) |
| A diff or tool output contains maxim trigger words | excluded from judging | [Judge and maxim scanning exclude generated diff](../invariants/session.md#session-judge-and-maxim-scanning-exclude-generated-diff) |
| A correction recurs past the threshold | a maxim proposal is filed for the operator to take up | [New taste is mined from recurring corrections and filed as a proposal](../invariants/session.md#session-new-taste-is-mined-from-recurring-corrections-and-filed-as-a-proposal) |
| A union arm never carries a field | the field is declared absent so a reader may narrow | [A field one arm never carries is declared absent](../invariants/session.md#extension-a-field-one-arm-never-carries-is-declared-absent) |
| A value is absent on the wire | one spelling is used, enforced mechanically | [Absence has one spelling, enforced mechanically](../invariants/session.md#extension-absence-has-one-spelling-enforced-mechanically) |
| A worktree has no configured vendor | resolution falls to the declared default | [Vendor resolution is explicit](../invariants/session.md#extension-vendor-resolution-is-explicit) |
| Configuration is malformed | refused loudly rather than defaulted past | [Bad configuration fails loudly](../invariants/session.md#extension-bad-configuration-fails-loudly) |

### Conduct

| Situation | Expected | Ref |
| --- | --- | --- |
| A refusal has nothing to run | it stays a bare diagnostic rather than inventing a repair | [Repair-first refusal exemptions](../invariants/distribution.md#conduct-repair-first-refusal-exemptions) |
| A gate reports findings | the board is rendered as evidence, then scored | [Gate reports are a third shape](../invariants/distribution.md#conduct-gate-reports-are-a-third-shape) |
| An optional capability is unavailable | the work proceeds without it | [Prefer the good property, never block on it](../invariants/distribution.md#conduct-prefer-the-good-property-never-block-on-it) |
| Release notes are generated for a range | derived from history, not from a maintained file | [Release notes derive from history](../invariants/distribution.md#distribution-release-notes-derive-from-history) |
| The same applicability rule is needed by a gate study and a maxim | expressed once in the shared path, extension, vendor, model, and phase selector grammar | [Gate policy/study and taste/maxim applicability share one selector grammar](../invariants/session.md#extension-gate-policy-study-and-taste-maxim-applicability-share-one-selector-grammar) |
| An agent reads any surface addressed to it | the tone is capability, not restriction | [The agent is a user with its own surface](../invariants/distribution.md#conduct-the-agent-is-a-user-with-its-own-surface) |
