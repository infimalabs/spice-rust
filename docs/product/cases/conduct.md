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
| The operator closes a pending task from its observation composer | the draft becomes the closure's validation rather than a note; the task completes from its current phase and its dependents may become eligible | [The operator closes a task with a recorded validation](../invariants/identity.md#work-the-operator-closes-a-task-with-a-recorded-validation) |
| The operator closes a task a lane still holds | the closure is refused naming the holder unless forced; forcing frees the claim and records the displaced holder | [The operator closes a task with a recorded validation](../invariants/identity.md#work-the-operator-closes-a-task-with-a-recorded-validation) |
| A pending task requires an exact executable-configuration digest whose trust head is missing, pended, revoked, or lacks producer-worktree provenance | the task remains pending but is excluded from next, capture, steal, recovery, and automatic capacity allocation; dependency-blocked counts remain separate | [Environmental blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-environmental-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| The required digest becomes granted with producer-worktree provenance in any linked repository worktree | ordinary eligibility is recomputed from the refreshed board snapshot; the unchanged task becomes allocatable without free, wake, reschedule, or lane affinity | [Environmental blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-environmental-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| The required grant is revoked after claim selection but before its guarded commit | atomic revalidation refuses the claim and writes no claim head | [Environmental blocking is an eligibility predicate, not a task transition](../invariants/identity.md#work-environmental-blocking-is-an-eligibility-predicate-not-a-task-transition) |
| A current holder encounters an unmet required trust digest | one guarded requirement edit retains the same claim and moves only lease expiry and maximum to the canonical 2099 horizon; no requirement-only or lease-only intermediate commits | [An unmet trust grant ices the current claim without a new state](../invariants/identity.md#work-an-unmet-trust-grant-ices-the-current-claim-without-a-new-state) |
| An iced claim owns a vacant lane with no new messages | held-claim, drain, and available-work starts are suppressed; the lane/worktree affinity remains and no unrelated task is claimed | [An unmet trust grant ices the current claim without a new state](../invariants/identity.md#work-an-unmet-trust-grant-ices-the-current-claim-without-a-new-state) |
| New operator direction arrives for an iced lane | pending-inbox remains the higher-priority message-driven wake and starts the same lane without removing its trust blocker | [An unmet trust grant ices the current claim without a new state](../invariants/identity.md#work-an-unmet-trust-grant-ices-the-current-claim-without-a-new-state) |
| The exact required digest becomes provenance-bound and granted, or its blocker is authorizedly removed | the same-lane held-claim seating path carries/rebinds possession and replaces sentinel lease values with ordinary bounded automatic values | [An unmet trust grant ices the current claim without a new state](../invariants/identity.md#work-an-unmet-trust-grant-ices-the-current-claim-without-a-new-state) |
| A deferral timestamp passes or a dependency settles | the task remains deferred until an explicit wake | [Only explicit defer and wake change deferral](../invariants/identity.md#work-only-explicit-defer-and-wake-change-deferral) |
| An explicitly woken task still has an unsettled dependency | deferral clears while workflow status stays `pending`; the task remains ineligible under the dependency predicate | [Only explicit defer and wake change deferral](../invariants/identity.md#work-only-explicit-defer-and-wake-change-deferral) |
| A plan phase is advanced with no acceptance anywhere | refused, naming both ways to satisfy it | [A plan phase closes only when acceptance exists](../invariants/identity.md#work-a-plan-phase-closes-only-when-acceptance-exists) |
| A connected pending child carries acceptance | the plan phase advances | [A plan phase closes only when acceptance exists](../invariants/identity.md#work-a-plan-phase-closes-only-when-acceptance-exists) |
| A plan-only flow is completed with no child connected | refused | [A plan-only flow must leave a connected child carrying acceptance](../invariants/identity.md#work-a-plan-only-flow-must-leave-a-connected-child-carrying-acceptance) |
| A lane holding a plan claim creates a task without explicit dependencies | child creation and the ambient plan's new dependency edge commit together, or neither commits | [Plan-born children connect atomically](../invariants/identity.md#work-plan-born-children-connect-atomically) |
| That task creation names `--depends` handles | those handles become the new task's prerequisites and the ambient plan is not edited | [Plan-born children connect atomically](../invariants/identity.md#work-plan-born-children-connect-atomically) |
| An operator reads the board | the prepended step appears as a phase, not as a hidden attribute | [The prepended phase is visible as a phase](../invariants/identity.md#work-the-prepended-phase-is-visible-as-a-phase) |
| A task statement matches the current taste policy | a plan phase holds it, and status, the assignment's next action, and the plan refusal all name the hold | [A taste hold is visible before a task is read](../invariants/identity.md#work-a-taste-hold-is-visible-before-a-task-is-read) |
| A held statement is reworded and still matches | the refusal names how many revisions were rechecked, so it reads differently from the first | [A taste hold is visible before a task is read](../invariants/identity.md#work-a-taste-hold-is-visible-before-a-task-is-read) |
| The statement's author reads the hold, on any surface | the matched token, its column, and the taste's message are withheld; the operator who owns the policy reads the complete audit | [The matched taste is withheld from the statement's author](../invariants/identity.md#work-the-matched-taste-is-withheld-from-the-statement-s-author) |

### Steering escalation and the shell boundary

| Situation | Expected | Ref |
| --- | --- | --- |
| A live item is ACKed, declined, canceled, or lapsed after any presentation stage, or a pending item is parked by its lane's qualifying `PendingInbox` automatic-wake failure | the corresponding typed disposition is available without a stage-specific restriction | [Every live presentation stage admits the same dispositions](../invariants/identity.md#steering-every-live-presentation-stage-admits-the-same-dispositions) |
| An item lapses unanswered | recorded as recoverable `lapsed`, distinct from `declined` or `canceled` | [Lapse and terminal dispositions are distinct](../invariants/identity.md#steering-lapse-and-terminal-dispositions-are-distinct) |
| A `lapsed` or `deadlettered` item is recovered | returns live with the same identity, lineage, presentation stage, and history | [Lapsed items remain recoverable](../invariants/identity.md#steering-lapsed-items-remain-recoverable) |
| An authenticated journal inbox readout is observed | the supervisor read changes neither journal-owned disposition nor presentation stage | [The journal inbox is authoritative](../invariants/identity.md#steering-the-journal-inbox-is-authoritative) |
| An agent is waiting on a tool call when steering becomes live | the supervisor interrupts the wait and interjects the pending steering | [Pending steering reaches each admitted agent boundary](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary) |
| Two steering keys would occupy the same Unix microsecond | the mint advances the indexed microsecond high-water by one and both current keys retain nine characters | [Steering keys use the current microsecond codec](../invariants/identity.md#steering-keys-use-the-current-microsecond-codec) |
| An agent runs one shell command | the wrapper runs exactly once for it | [Shell instrumentation is one-shot at the agent's own command boundary](../invariants/identity.md#steering-shell-instrumentation-is-one-shot-at-the-agent-s-own-command-boundary) |
| An environment marker is forged or missing | the stage is still decided structurally, not by that marker | [The exactly-once gate is structural](../invariants/identity.md#steering-the-exactly-once-gate-is-structural) |
| A shell script executed by the agent's command inspects its environment | it sees the user's shell, unmodified | [A descendant process gets the user's shell unmodified](../invariants/identity.md#steering-a-descendant-process-gets-the-user-s-shell-unmodified) |
| A script is sourced rather than executed | it shares the immediate shell's functions and options | [A sourced script is not a descendant and shares the immediate shell's functions](../invariants/identity.md#steering-a-sourced-script-is-not-a-descendant-and-shares-the-immediate-shell-s-functions) |
| An occupant starts while steering is pending | the command-driven supervisor catches up from the authenticated journal inbox before relying on later notifications | [Supervisor delivery reads journal authority](../invariants/identity.md#steering-supervisor-delivery-reads-journal-authority) |
| Steering becomes live while a command is active | the resident kicks the supervisor, which reads current journal authority and interrupts the call before interjecting unheard direction; unchanged renderings remain suppressed | [Supervisor delivery reads journal authority](../invariants/identity.md#steering-supervisor-delivery-reads-journal-authority) |
| The immediate child forks and waits, or replaces itself | the wrapper's wait and private output relays remain bound to that process identity | [Private output relays end with the immediate child](../invariants/identity.md#steering-private-output-relays-end-with-the-immediate-child) |
| A background descendant outlives the immediate child | inherited relay writers are revoked and committed output drains, so the wrapper still finishes | [Private output relays end with the immediate child](../invariants/identity.md#steering-private-output-relays-end-with-the-immediate-child) |
| A compound command reaches the boundary | the whole command string is handed over once, before anything runs | [The complete top-level command string reaches the wrapper exactly once](../invariants/identity.md#steering-the-complete-top-level-command-string-reaches-the-wrapper-exactly-once) |
| An agent works without a shell command but continues non-shell tool calls | the supervisor still interrupts outstanding tool waits to deliver unheard direction | [Steering reaches the next admitted agent boundary](../invariants/identity.md#steering-reaches-the-next-admitted-agent-boundary) |
| No command-driven supervisor is running | delivery waits while the durable directive remains live, and the next occupant's startup catch-up delivers it | [Steering reaches the next admitted agent boundary](../invariants/identity.md#steering-reaches-the-next-admitted-agent-boundary) |
| An agent backgrounds long work and ends its turn | the turn exit vacates the lane while the detached keeper continues; an ending left unheard through the delivery grace requests a lane wake, and the successor receives the completion during startup catch-up | [Ending a turn vacates the lane](../invariants/lifecycle.md#lifecycle-ending-a-turn-vacates-the-lane) |
| An agent has already-backgrounded work | it uses `job wait` when a finite job's result blocks current work; waiting on a service answers at once and leaves it running, readiness is checked through logs or the served surface, `job stop` ends a service no longer needed, and each seat start names services still running | [Background jobs survive turn exit](../invariants/lifecycle.md#lifecycle-background-jobs-survive-turn-exit) |

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
