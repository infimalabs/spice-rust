# Case battery — Orchestration and audit

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### Affinity and anti-affinity

| Situation | Expected | Ref |
| --- | --- | --- |
| Two candidates differ only in placement | both terms are read off the same (project, phase) cell | [Affinity and anti-affinity address one coordinate](../invariants/identity.md#work-affinity-and-anti-affinity-address-one-coordinate) |
| A peer works a cell this actor's route cannot reach | that cell still counts as crowded | [Anti-affinity reads peers across the whole board](../invariants/identity.md#work-anti-affinity-reads-peers-across-the-whole-board) |
| Every ready row sits in a cell a peer occupies | the actor is handed one anyway | [Anti-affinity is a preference, never an exclusion](../invariants/identity.md#work-anti-affinity-is-a-preference-never-an-exclusion) |
| An actor's last three claims were in three areas | only the most recent one attracts | [Affinity is measured from the actor's most recently claimed cell alone](../invariants/identity.md#work-affinity-is-measured-from-the-actor-s-most-recently-claimed-cell-alone) |
| One candidate changes phase and another changes project, each changing only one coordinate | both carry the same movement cost | [Movement counts project and phase separately and sums the two](../invariants/identity.md#work-movement-counts-project-and-phase-separately-and-sums-the-two) |
| Two rows tie above the band but differ widely in urgency | neither locality term is allowed to decide between them | [Locality applies only inside a narrow urgency band](../invariants/identity.md#work-locality-applies-only-inside-a-narrow-urgency-band) |
| Rows fall outside the band | they order by urgency alone and sort after every in-band row | [Locality applies only inside a narrow urgency band](../invariants/identity.md#work-locality-applies-only-inside-a-narrow-urgency-band) |

### The prepended plan phase

| Situation | Expected | Ref |
| --- | --- | --- |
| A task is captured with no acceptance and no named flow | a plan phase is prepended to its flow; the task is not refused | [Tasks without acceptance enter a plan phase](../invariants/identity.md#work-tasks-without-acceptance-enter-a-plan-phase) |
| A submitted flow is empty, repeats a phase, or contains a name that is not one lowercase ASCII word | refused as an invalid flow; no global enumeration or slot ceiling is substituted | [A task flow is nonempty, unique, and self-containing](../invariants/identity.md#work-a-task-flow-is-nonempty-unique-and-self-containing) |
| A durable workflow names a current phase outside its declared flow | authority admission refuses the inconsistent head | [A task flow is nonempty, unique, and self-containing](../invariants/identity.md#work-a-task-flow-is-nonempty-unique-and-self-containing) |
| A pending task has an unsettled dependency | remains pending but is ineligible; no deferral or workflow transition is written | [Dependency blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-dependency-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| The last unsettled dependency completes | ordinary allocation may now admit the still-pending task; dependency completion writes no transition on the dependent | [Dependency blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-dependency-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| The last unsettled dependency is withdrawn | withdrawal settles the edge, so ordinary allocation may admit the still-pending dependent | [Dependency blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-dependency-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| A deferral timestamp passes or a dependency settles | the task remains deferred until an explicit wake | [Only explicit defer and wake change deferral](../invariants/identity.md#work-only-explicit-defer-and-wake-change-deferral) |
| An explicitly woken task still has an unsettled dependency | deferral clears while workflow status stays `pending`; the task remains ineligible under the dependency predicate | [Only explicit defer and wake change deferral](../invariants/identity.md#work-only-explicit-defer-and-wake-change-deferral) |
| A plan phase is advanced with no acceptance anywhere | refused, naming both ways to satisfy it | [A plan phase closes only when acceptance exists](../invariants/identity.md#work-a-plan-phase-closes-only-when-acceptance-exists) |
| A connected pending child carries acceptance | the plan phase advances | [A plan phase closes only when acceptance exists](../invariants/identity.md#work-a-plan-phase-closes-only-when-acceptance-exists) |
| A plan-only flow is completed with no child connected | refused | [A plan-only flow must leave a connected child carrying acceptance](../invariants/identity.md#work-a-plan-only-flow-must-leave-a-connected-child-carrying-acceptance) |
| An operator reads the board | the prepended step appears as a phase, not as a hidden attribute | [The prepended phase is visible as a phase](../invariants/identity.md#work-the-prepended-phase-is-visible-as-a-phase) |

### Steering escalation and the shell boundary

| Situation | Expected | Ref |
| --- | --- | --- |
| A live item is ACKed, declined, canceled, or lapsed after any presentation stage, or a pending item is parked by its lane's qualifying `PendingInbox` automatic-wake failure | the corresponding typed disposition is available without a stage-specific restriction | [Every live presentation stage admits the same dispositions](../invariants/identity.md#steering-every-live-presentation-stage-admits-the-same-dispositions) |
| An item lapses unanswered | recorded as recoverable `lapsed`, distinct from `declined` or `canceled` | [Lapse and terminal dispositions are distinct](../invariants/identity.md#steering-lapse-and-terminal-dispositions-are-distinct) |
| A `lapsed` or `deadlettered` item is recovered | returns live with the same identity, lineage, presentation stage, and history | [Lapsed items remain recoverable](../invariants/identity.md#steering-lapsed-items-remain-recoverable) |
| A disposable inbox readout is observed | a vendor hook may advance its delivery record while a shell read does not; neither changes journal-owned disposition or presentation stage | [The filesystem inbox is a delivery projection](../invariants/identity.md#steering-the-filesystem-inbox-is-a-delivery-projection) |
| An agent completes a non-shell tool call | pending steering is eligible at the vendor post-tool boundary | [Pending steering reaches each admitted agent boundary](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary) |
| The same live item reaches a vendor post-tool boundary and then an active or next top-level shell command | the hook consults its durable suppression record; the shell independently streams changed rendered-inbox bytes and may repeat the same live content | [Pending steering reaches each admitted agent boundary](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary) |
| Two steering keys would occupy the same epoch millisecond | the mint advances the indexed millisecond high-water mark and both keys retain the fixed width | [Steering keys are fixed-width monotone epoch milliseconds](../invariants/identity.md#steering-keys-are-fixed-width-monotone-epoch-milliseconds) |
| An agent runs one shell command | the wrapper runs exactly once for it | [Shell instrumentation is one-shot at the agent's own command boundary](../invariants/identity.md#steering-shell-instrumentation-is-one-shot-at-the-agent-s-own-command-boundary) |
| An environment marker is forged or missing | the stage is still decided structurally, not by that marker | [The exactly-once gate is structural](../invariants/identity.md#steering-the-exactly-once-gate-is-structural) |
| A shell script executed by the agent's command inspects its environment | it sees the user's shell, unmodified | [A descendant process gets the user's shell unmodified](../invariants/identity.md#steering-a-descendant-process-gets-the-user-s-shell-unmodified) |
| A script is sourced rather than executed | it shares the immediate shell's functions and options | [A sourced script is not a descendant and shares the immediate shell's functions](../invariants/identity.md#steering-a-sourced-script-is-not-a-descendant-and-shares-the-immediate-shell-s-functions) |
| A top-level command begins with a rendered inbox | the wrapper writes the current nonempty rendering to wrapper stderr, arms the bounded projection scanner, and only then spawns the command | [Shell delivery spans the immediate child lifetime](../invariants/identity.md#steering-shell-delivery-spans-the-immediate-child-lifetime) |
| Steering becomes live while a top-level command's immediate child is active | the wrapper writes the changed whole rendering to its own stderr without waiting for the child or a vendor hook to finish; identical bytes within that command remain suppressed | [Shell delivery spans the immediate child lifetime](../invariants/identity.md#steering-shell-delivery-spans-the-immediate-child-lifetime) |
| The immediate child forks and waits, or replaces itself | the wrapper's wait and private output relays remain bound to that process identity | [Private output relays end with the immediate child](../invariants/identity.md#steering-private-output-relays-end-with-the-immediate-child) |
| A background descendant outlives the immediate child | inherited relay writers are revoked and committed output drains, so the wrapper still finishes | [Private output relays end with the immediate child](../invariants/identity.md#steering-private-output-relays-end-with-the-immediate-child) |
| A descendant redirects its own error stream | steering is unaffected | [Steering writes to the wrapper's own error stream](../invariants/identity.md#steering-writes-to-the-wrapper-s-own-error-stream) |
| The wrapper's own error stream is redirected | steering follows it | [Steering writes to the wrapper's own error stream](../invariants/identity.md#steering-writes-to-the-wrapper-s-own-error-stream) |
| A compound command reaches the boundary | the whole command string is handed over once, before anything runs | [The complete top-level command string reaches the wrapper exactly once](../invariants/identity.md#steering-the-complete-top-level-command-string-reaches-the-wrapper-exactly-once) |
| An agent works without a shell command but continues non-shell tool calls | each vendor post-tool hook remains a delivery opportunity | [Steering reaches the next admitted agent boundary](../invariants/identity.md#steering-reaches-the-next-admitted-agent-boundary) |
| An agent crosses neither a vendor post-tool hook nor an active or next top-level shell-command delivery span | delivery waits while the durable directive remains live | [Steering reaches the next admitted agent boundary](../invariants/identity.md#steering-reaches-the-next-admitted-agent-boundary) |
| An agent backgrounds long work and ends its turn | the turn exit vacates the lane; that command's later exit does not launch another occupant | [Ending a turn vacates the lane](../invariants/lifecycle.md#lifecycle-ending-a-turn-vacates-the-lane) |
| An agent must wait on already-backgrounded work | it brings the wait into the foreground rather than ending on it | [Long work runs in the foreground](../invariants/lifecycle.md#lifecycle-long-work-runs-in-the-foreground) |

### Maxim bags

| Situation | Expected | Ref |
| --- | --- | --- |
| Prose contains a trigger word inside a longer word | no match; matching is over whole words | [A maxim bag is one opinion with several triggers](../invariants/session.md#session-a-maxim-bag-is-one-opinion-with-several-triggers) |
| A variation of a trigger appears that the bag does not list | no match; the bag is the enumeration | [A maxim bag is one opinion with several triggers](../invariants/session.md#session-a-maxim-bag-is-one-opinion-with-several-triggers) |
| A trigger is configured that is not an alphabetic word or phrase | refused at load | [A trigger must be an alphabetic word or phrase and is refused at load otherwise](../invariants/session.md#session-a-trigger-must-be-an-alphabetic-word-or-phrase-and-is-refused-at-load-otherwise) |
| One statement hits three triggers of one bag | one reminder | [One statement fires a bag at most once however many of its triggers hit](../invariants/session.md#session-one-statement-fires-a-bag-at-most-once-however-many-of-its-triggers-hit) |
| The same prose is scanned twice in one compaction epoch | the first scan emits the deterministic candidate sequence once; the repeated content-derived key is suppressed | [One statement fires a bag at most once however many of its triggers hit](../invariants/session.md#session-one-statement-fires-a-bag-at-most-once-however-many-of-its-triggers-hit) |
| A bag is scoped to one vendor | it does not fire for another | [Bag applicability uses the one shared selector grammar](../invariants/session.md#session-bag-applicability-uses-the-one-shared-selector-grammar) |
| A bag is disabled | the disablement is named, tracked, and identical in every clone | [Bag applicability uses the one shared selector grammar](../invariants/session.md#session-bag-applicability-uses-the-one-shared-selector-grammar) |
| A judge is asked to adjudicate the same evidence twice | equivalent framings appear in the same deterministic order; a different evidence key may produce a different order | [Adjudication states the question in several equivalent framings and shuffles them](../invariants/session.md#session-adjudication-states-the-question-in-several-equivalent-framings-and-shuffles-them) |
| A judge answers with something other than yes or no | retried a bounded number of times, then abandoned rather than interpreted | [Adjudication states the question in several equivalent framings and shuffles them](../invariants/session.md#session-adjudication-states-the-question-in-several-equivalent-framings-and-shuffles-them) |

---

### Audited by review, not settled by a case

The following properties make a claim about an **absence**, and no execution
demonstrates an absence. Where a case exists it demonstrates an instance; what
makes the property true is a reading of the tree. Each therefore carries an
audit as well as whatever cases touch it.

- **[One visible path](../invariants/distribution.md#conduct-one-visible-path)** — no case can
  prove the absence of alternates; that requires reading the tree.
- **[Authoritative degradation is loud](../invariants/distribution.md#conduct-authoritative-degradation-is-loud)** — individual cases demonstrate typed refusals and the two admitted silent shapes;
  establishing that no authoritative observation, durable fact, or visible
  primary operation degrades silently requires enumerating every recovery and
  suppression site in the tree.
- **[Agents never pull and never push](../invariants/lifecycle.md#integration-agents-never-pull-and-never-push)** — the corresponding case states the audit as a row;
  what makes it true is that no such call site exists, not an observation of one
  that did not fire.
- **[Activation is deliberately not a fourth boundary](../invariants/lifecycle.md#integration-activation-is-deliberately-not-a-fourth-boundary)** — activation is not a fourth boundary. The corresponding case demonstrates one
  replayed activation leaving git untouched; the universal claim is proven by
  enumerating the writers, since a fourth boundary is invisible until it fires
  at the wrong moment.

Each is worth a periodic audit precisely because nothing fails when they are
violated — until something does.
