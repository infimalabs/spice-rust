# Case battery — Supporting planes

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### Checks

| Situation | Expected | Ref |
| --- | --- | --- |
| New file exceeds base, under flex | allowed; latched | [Flex is headroom, not a new limit](../invariants/checks.md#checks-flex-is-headroom-not-a-new-limit) |
| Same file edited again, still over base | refused at base | [Flex is headroom, not a new limit](../invariants/checks.md#checks-flex-is-headroom-not-a-new-limit) |
| File shrinks under base in an accepted run | the observed latch heals with the accepted state; unrelated latches survive a partial run | [Accepted reconciliation is scoped](../invariants/checks.md#checks-accepted-reconciliation-is-scoped), [A healed latch retires only with an accepted run](../invariants/checks.md#checks-a-healed-latch-retires-only-with-an-accepted-run) |
| Two agents hit the same hot file | both judge the shared accepted ledger; provenance-seeded jitter and an accepted time-boxed claim coordinate the slice | [Sticky latches and live claims are repository-shared](../invariants/checks.md#checks-sticky-latches-and-live-claims-are-repository-shared), [Ceilings are jittered per path and resolved provenance](../invariants/checks.md#checks-ceilings-are-jittered-per-path-and-resolved-provenance) |
| Pre-existing breach touched incidentally | informs, does not block | [Dirty work renders the same pressure as steering](../invariants/checks.md#checks-dirty-work-renders-the-same-pressure-as-steering) |
| Another actor reads an accepted latch | the same repository latch holds the subject at base; any informational redirect comes from the distinct accepted claim | [Sticky latches and live claims are repository-shared](../invariants/checks.md#checks-sticky-latches-and-live-claims-are-repository-shared), [A finding whose subject is held by a peer informs and passes](../invariants/checks.md#checks-a-finding-whose-subject-is-held-by-a-peer-informs-and-passes) |
| Wide measure unchanged but high | passes | [Wide measures diff against a baseline](../invariants/checks.md#checks-wide-measures-diff-against-a-baseline) |
| Wide measure rises by one | refused | [Wide measures diff against a baseline](../invariants/checks.md#checks-wide-measures-diff-against-a-baseline) |
| Gate-findings dispatch is explicitly requested for one registered study | each reported finding becomes deferred, traceable work | [A requested gate-findings dispatch files deferred work](../invariants/checks.md#checks-a-requested-gate-findings-dispatch-files-deferred-work) |
| A gate blocks a merge | fixed at the source, never by weakening the gate | [A gate is never closed by weakening it](../invariants/checks.md#checks-a-gate-is-never-closed-by-weakening-it) |

### Session and counsel

| Situation | Expected | Ref |
| --- | --- | --- |
| Agent compacts mid-task | briefing restores ask, delivery, working set, pending steering | [The briefing answers four questions mechanically](../invariants/session.md#session-the-briefing-answers-four-questions-mechanically) |
| Agent is renewed | successor gets the same briefing surface | [The briefing answers four questions mechanically](../invariants/session.md#session-the-briefing-answers-four-questions-mechanically) |
| No pending asks | the section is absent, not empty-with-a-heading | [Sections with nothing to say are omitted](../invariants/session.md#session-sections-with-nothing-to-say-are-omitted) |
| A window is unreadable | reported loudly | [Windows the engine could not read are reported loudly](../invariants/session.md#session-windows-the-engine-could-not-read-are-reported-loudly) |
| Recovery text is only assistant prose | not rendered as the user's ask | [Recovery shows the recovered ask](../invariants/session.md#session-recovery-shows-the-recovered-ask) |
| Same command run repeatedly | meter line suppressed after the first | [The working-state line is short, high-signal](../invariants/session.md#session-the-working-state-line-is-short-high-signal) |
| Maxim fires twice on the same content in one epoch | one reminder | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| Compaction occurs, then the same content recurs | eligible again; text unchanged | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| The system quotes its own reminder | not judged | [At most one maxim reminder per content-derived key per compaction epoch](../invariants/session.md#session-at-most-one-maxim-reminder-per-content-derived-key-per-compaction-epoch) |
| Judging is disabled | a configured bag hit publishes directly | [Adjudication is optional](../invariants/session.md#session-adjudication-is-optional) |
| Judging is enabled but the process is absent, times out, errors, or exhausts ambiguous replies | the candidate is abandoned and emits nothing; enabled judging never widens into direct publication | [Adjudication is optional](../invariants/session.md#session-adjudication-is-optional) |
| A maxim is confirmed | returns as ordinary steering | [A confirmed maxim reminder returns as ordinary steering](../invariants/session.md#session-a-confirmed-maxim-reminder-returns-as-ordinary-steering) |

### Metrics

| Situation | Expected | Ref |
| --- | --- | --- |
| Transcript replaced under the same path | ingestion restarts from byte zero | [Resumption is a checkpoint, not an offset](../invariants/session.md#metrics-resumption-is-a-checkpoint-not-an-offset) |
| Ingestion dies mid-pass | the next pass reads the same bytes exactly once | [Counted facts and their checkpoint commit as one write](../invariants/session.md#metrics-counted-facts-and-their-checkpoint-commit-as-one-write) |
| One line has prose, reasoning, and a tool call | three facts counted | [One line carrying prose, reasoning, and a tool call contributes all three](../invariants/session.md#metrics-one-line-carrying-prose-reasoning-and-a-tool-call-contributes-all-three) |
| Operator input appears in the transcript | contributes no activity | [Activity is what the agent produced](../invariants/session.md#metrics-activity-is-what-the-agent-produced) |
| Agent renewed mid-window | the lineage lens shows one continuous worker | [Work follows the agent across renewal and lane changes through durable membership and renewal history](../invariants/session.md#metrics-work-follows-the-agent-across-renewal-and-lane-changes-through-durable-membership-and-renewal-history) |
| Phase window is partial | withheld from sizing | [A partial effort window is not sizing evidence](../invariants/session.md#metrics-a-partial-effort-window-is-not-sizing-evidence) |
| Two phases used different models | not summed into one number | [Model-untagged spend is not rendered as comparable to tagged spend](../invariants/session.md#metrics-model-untagged-spend-is-not-rendered-as-comparable-to-tagged-spend) |

### Task documents

| Situation | Expected | Ref |
| --- | --- | --- |
| Canonical document text is classified and emitted again | byte-identical fixed point | [The task-document properties](../interfaces.md#behavioral-properties) |
| `doc ledger HANDLE` renders a dialect-representable task | emits only its title, current flow, and tags; it is not a family export | [The task-document Rust surface](../interfaces.md#the-rust-surface) |
| `doc ledger HANDLE` sees a task whose flow is outside the dialect vocabulary | reports `doc::ledger-unrepresentable` rather than inventing a document | [The task-document Rust surface](../interfaces.md#the-rust-surface) |
| Apply twice | idempotent | [The task-document properties](../interfaces.md#behavioral-properties) |
| Apply is interrupted before or after commit | no partial family exists; retry commits once or returns the existing receipt | [The task-document properties](../interfaces.md#behavioral-properties) |
| The board changes after an apply invocation captures its board barrier | apply refuses the moved barrier before mutation | [The task-document properties](../interfaces.md#behavioral-properties) |
| A changed document carries a slug already present in `task.document` | edits that task; the new document digest does not create an origin-scoped duplicate | [Identity within a document](../interfaces.md#identity-within-a-document) |
| Document contains a cycle | refused with the cycle named | [The task-document properties](../interfaces.md#behavioral-properties) |
| Hostile input: BOM, CRLF, deep nesting | parses or refuses; never crashes | [The task-document properties](../interfaces.md#behavioral-properties) |
| Acceptance, authored tags, priority, or an explicit dependency is absent from an edited document | prior authored value is removed; apply is replacement, not patch | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
| Flow changes in a document that matches an existing task | existing workflow flow and phase remain unchanged | [The board projection is intentionally narrower than the dialect](../interfaces.md#the-board-projection-is-intentionally-narrower-than-the-dialect) |
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
| The release battery cannot execute every canonical arm over the plan's source commit | the correctness operation refuses; omission cannot become a release receipt | [The release gate runs the complete strict battery over one immutable source](../invariants/distribution.md#distribution-the-release-gate-runs-the-complete-strict-battery-over-one-immutable-source) |
| Range contains an implement-and-revert pair | suppressed from highlights | [Implement-and-revert pairs in one range are suppressed](../invariants/distribution.md#distribution-implement-and-revert-pairs-in-one-range-are-suppressed) |
| Gate check, notes draft, and range preview all run | nothing bumped, committed, tagged, pushed, or published | [Inspection is free of consequence](../invariants/distribution.md#distribution-inspection-is-free-of-consequence) |
| Install unapplied | reverses exactly the canonical receipt and retains that receipt with terminal status | [An install has one canonical status-bearing receipt](../invariants/distribution.md#distribution-an-install-has-one-canonical-status-bearing-receipt) |
| Observation run over foreign sessions | no repo, team, claim, hook, or steering writes | [Observation initializes nothing](../invariants/distribution.md#conduct-observation-initializes-nothing) |
| Observation encounters an unreadable file | reported in the UI and the log; skipped | [Authoritative degradation is loud](../invariants/distribution.md#conduct-authoritative-degradation-is-loud) |
| A user stops at the observation tier | fully usable there | [Each capability tier is independently usable](../invariants/distribution.md#conduct-each-capability-tier-is-independently-usable) |

### Identity and authority

| Situation | Expected | Ref |
| --- | --- | --- |
| A task is re-homed to another project | its rendered handle changes; its stored identity does not | [A task's `born` stamp is its only stored identity](../invariants/identity.md#identity-a-task-s-born-stamp-is-its-only-stored-identity) |
| `born` stamps are sorted as plain strings | the order matches birth order | [The stored stamp orders births without parsing](../invariants/identity.md#identity-the-stamp-is-a-fixed-width-order-preserving-base-encoding-of-birth-time) |
| Many tasks are minted at the same represented instant | each gets a distinct, still-ordered stamp | [Collision resolution preserves stamp order](../invariants/identity.md#identity-the-stamp-alphabet-omits-vowels-in-both-cases) |
| A stream frame is delivered twice at the same sequence | applied once; the duplicate cannot advance the cursor | [Stream sequence is monotone](../invariants/identity.md#identity-stream-sequence-is-monotone) |
| The resident restarts while a client holds a cursor | the changed epoch rejects resume and yields one authoritative snapshot | [A resume cursor belongs to one exact stream](../invariants/identity.md#identity-a-resume-cursor-belongs-to-one-exact-stream) |
| A frame skips a sequence | consumer resumes the exact gap or reconnects for a snapshot; it never guesses | [Stream sequence is monotone](../invariants/identity.md#identity-stream-sequence-is-monotone) |
| A client from another application generation presents a cursor | refused and reconnected against the admitted generation | [The authenticated stream carries generation, epoch, and sequence](../invariants/identity.md#identity-the-authenticated-stream-carries-generation-epoch-and-sequence) |
| Snapshot setup races an accepted journal fact | the fact appears exactly once, in the snapshot head or the next frame | [One epoch-bound stream carries snapshot and resume](../invariants/identity.md#identity-one-epoch-bound-stream-carries-snapshot-and-resume) |
| A projection is deleted | rebuilt from its source; no authority is lost | [Durable authority and rebuildable projection never share a file](../invariants/identity.md#identity-durable-authority-and-rebuildable-projection-never-share-a-file) |
| A projection's source no longer reaches back far enough | it says so rather than answering from nothing | [A projection family must declare its source](../invariants/identity.md#identity-a-projection-family-must-declare-its-source) |
| An authority store several supported shapes behind is opened | each ordered migration applies once through the unbroken chain | [Authority migrations form one ordered forward chain](../invariants/identity.md#identity-authority-migrations-form-one-ordered-forward-chain) |
| An authority store of an unsupported shape is opened | refused without mutation, naming the shape and supported range | [Authority migrations form one ordered forward chain](../invariants/identity.md#identity-authority-migrations-form-one-ordered-forward-chain) |
| Metrics are counted from the transcript | only public typed facts are read | [Counting reads public typed transcript facts](../invariants/session.md#metrics-counting-reads-public-typed-transcript-facts) |
| A consumer needs a fact another plane owns | it asks the owner rather than re-deriving | [Every fact has exactly one authority](../invariants/identity.md#identity-every-fact-has-exactly-one-authority) |

### Work, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| A claim is taken | activity marker and claim metadata are set together | [Claiming is atomic](../invariants/identity.md#work-claiming-is-atomic) |
| A claim is inspected after the fact | its bounded lease, exact opening and closing source boundaries, and verified contiguous effort segments are reconstructible | [Claims carry a bounded lease and an exact source boundary](../invariants/identity.md#work-claims-carry-a-bounded-lease-and-an-exact-source-boundary) |
| Two candidates tie on graph rank | locality breaks the tie, and only then | [Locality is the final tie-break and never overrides the graph](../invariants/identity.md#work-locality-is-the-final-tie-break-and-never-overrides-the-graph) |
| A task is filed through a conventional authored or typed system route | it retains the corresponding `ack:`, `task:`, `gate:`, or `doc:` origin realm | [Every task names an origin realm](../invariants/identity.md#work-every-task-names-an-origin-realm) |
| An authored task supplies another nonempty `class:value` origin | the open wire shape is accepted and preserved; no closed realm enum is imposed | [Every task names an origin realm](../invariants/identity.md#work-every-task-names-an-origin-realm) |
| A phase advance is replayed from history | it is recoverable without a parallel event log | [A phase advance is a publication](../invariants/identity.md#work-a-phase-advance-is-a-publication) |
| A review finds nothing | the phase advances; no merge is produced | [A review that changes nothing still advances the phase](../invariants/identity.md#work-a-review-that-changes-nothing-still-advances-the-phase) |
| An agent files an admission of a mistake | it lands on a hidden channel, not the public board | [Hidden channels keep admissions private](../invariants/identity.md#work-hidden-channels-keep-admissions-private) |
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
| New content arrives | it places by insert; nothing already placed is re-decided | [New content places by insert](../invariants/lifecycle.md#surface-new-content-places-by-insert) |
| A neighbour's content grows | untouched cards keep their column and span | [A card's column and span latch between geometry replays](../invariants/lifecycle.md#surface-a-card-s-column-and-span-are-decided-once) |
| Any motion is observed | it corresponds to a change, not to a re-render | [Movement means change](../invariants/lifecycle.md#surface-movement-means-change) |
| Compensation adjusts the viewport | it is not read as a user gesture | [Programmatic scroll is excluded from anything that reads user intent](../invariants/lifecycle.md#surface-programmatic-scroll-is-excluded-from-anything-that-reads-user-intent) |
| Several changed stream frames arrive in one tick | each frame immediately publishes the resulting surface state | [Each changed stream frame publishes immediately](../invariants/lifecycle.md#surface-each-changed-stream-frame-publishes-immediately) |
| More lane identities exist than palette slots | colours repeat; explicit lane attribution remains unambiguous | [Accent derives from stable lane identity](../invariants/lifecycle.md#surface-accent-derives-from-stable-lane-identity) |
| Two clients render the same fused team | stable lane-derived accents and explicit attribution agree independently of member order | [Member order controls composition, not identity](../invariants/lifecycle.md#topology-member-order-controls-composition-not-identity) |

### Checks, session, extension, continued

| Situation | Expected | Ref |
| --- | --- | --- |
| A study runs from the command surface with flags | the commit gate still runs the defaults | [Direct study commands may take flags](../invariants/checks.md#checks-direct-study-commands-may-take-flags) |
| A check setting is changed | every consumer changes with it | [Check settings have one source](../invariants/checks.md#checks-check-settings-have-one-source) |
| A test-only public symbol is introduced | refused | [Some ceilings are absolute zero](../invariants/checks.md#checks-some-ceilings-are-absolute-zero) |
| A test asserts absence where a present property exists | flagged in favour of the positive assertion | [Tests assert present, observable properties in preference to absence](../invariants/checks.md#checks-tests-assert-present-observable-properties-in-preference-to-absence) |
| Transcript prose and steering disagree about the ask | steering is authoritative; prose is secondary | [Asks are sourced from the steering plane](../invariants/session.md#session-asks-are-sourced-from-the-steering-plane) |
| Rehydration spans many compaction windows | bounded to the recent few | [Rehydration is bounded to a small number of recent compaction windows](../invariants/session.md#session-rehydration-is-bounded-to-a-small-number-of-recent-compaction-windows) |
| Two identical entries would render | collapsed; no class repeats back-to-back | [Identical entries collapse and no class repeats back-to-back](../invariants/session.md#session-identical-entries-collapse-and-no-class-repeats-back-to-back) |
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
| The same applicability rule is needed by two subsystems | expressed once, in one selector grammar | [Applicability is expressed once in one selector grammar reused by policies](../invariants/session.md#extension-applicability-is-expressed-once-in-one-selector-grammar-reused-by-policies) |
| An agent reads any surface addressed to it | the tone is capability, not restriction | [The agent is a user with its own surface](../invariants/distribution.md#conduct-the-agent-is-a-user-with-its-own-surface) |
