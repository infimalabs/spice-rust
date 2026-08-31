# Case battery — Surface and checks

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### The rewind guard

| Situation | Expected | Ref |
| --- | --- | --- |
| A reset would drop commits already merged upstream | refused, naming them | [The checked-out branch cannot be rewound past commits already merged upstream](../invariants/lifecycle.md#integration-the-checked-out-branch-cannot-be-rewound-past-commits-already-merged-upstream) |
| An amend would rewrite a commit already merged upstream | refused | [The checked-out branch cannot be rewound past commits already merged upstream](../invariants/lifecycle.md#integration-the-checked-out-branch-cannot-be-rewound-past-commits-already-merged-upstream) |
| The caller declares no old value, so git reports a zero | the tip is still read from the ref; the rewind is judged as a rewind | [The guard reads the branch tip from the ref itself](../invariants/lifecycle.md#integration-the-guard-reads-the-branch-tip-from-the-ref-itself) |
| A process-local shadow aims the branch's alias at itself | the alias is not consulted, so local commits are not read as published | [The upstream is resolved from the branch's own local pairing and nowhere else](../invariants/lifecycle.md#integration-the-upstream-is-resolved-from-the-branch-s-own-local-pairing-and-nowhere-else) |
| The branch tracks nothing at all | the update passes | [Naming an upstream and resolving one are different questions](../invariants/lifecycle.md#integration-naming-an-upstream-and-resolving-one-are-different-questions) |
| The pairing names an upstream that does not resolve, and the update appends | passes — an append gives up no history | [Naming an upstream and resolving one are different questions](../invariants/lifecycle.md#integration-naming-an-upstream-and-resolving-one-are-different-questions) |
| The pairing names an upstream that does not resolve, and the update rewinds | refused as unknown, not passed as cleared | [Naming an upstream and resolving one are different questions](../invariants/lifecycle.md#integration-naming-an-upstream-and-resolving-one-are-different-questions), [Unknown is not the same as clear](../invariants/distribution.md#conduct-unknown-is-not-the-same-as-clear) |
| The branch pairs with a remote or branch name other than the conventional one | that pairing is the upstream; no name is assumed | [The guard assumes no particular remote or branch name](../invariants/lifecycle.md#integration-the-guard-assumes-no-particular-remote-or-branch-name) |

### The composer strip

| Situation | Expected | Ref |
| --- | --- | --- |
| A send receives a typed refusal or fault | the draft returns intact, with its attachments and quotes | [A draft is restored intact on a typed refusal or fault](../invariants/lifecycle.md#surface-a-draft-is-restored-intact-on-a-typed-refusal-or-fault) |
| Several sends are queued and one fails | the queue stops; every undelivered draft is returned | [Sends drain serially per lane](../invariants/lifecycle.md#surface-sends-drain-serially-per-lane) |
| The backend stalls after accepting | the pending count converges on truth and never drifts | [The pending count is a floor over backend truth](../invariants/lifecycle.md#surface-the-pending-count-is-a-floor-over-backend-truth) |
| A predicted item never appears in the backend's keys | the prediction is dropped by identity, not by a timer | [The pending count is a floor over backend truth](../invariants/lifecycle.md#surface-the-pending-count-is-a-floor-over-backend-truth) |
| The operator leaves or reloads with an unsafe draft or submission | the browser requests confirmation first | [Unsubmitted intent resists loss](../invariants/lifecycle.md#surface-unsubmitted-intent-resists-loss) |
| `Close team` is chosen with an unsent draft | the current browser hides the team without erasing the draft or changing topology | [Close team is browser-local presentation](../invariants/lifecycle.md#topology-close-team-is-browser-local-presentation), [Unsubmitted intent resists loss](../invariants/lifecycle.md#surface-unsubmitted-intent-resists-loss) |
| A send completes after the operator has moved on | focus stays where the operator put it | [A late completion does not take focus the operator has since moved elsewhere](../invariants/lifecycle.md#surface-a-late-completion-does-not-take-focus-the-operator-has-since-moved-elsewhere) |
| The operator reads the strip without interacting | age, run state, pending count and claimed work are already there | [The composer strip reports state the operator did not ask for](../invariants/lifecycle.md#surface-the-composer-strip-reports-state-the-operator-did-not-ask-for) |

### Check mechanics

| Situation | Expected | Ref |
| --- | --- | --- |
| A run starts with an explicit ordered gate selection | that immutable selection and order govern the whole run; no registered arm is implicitly added | [The selected gate battery is deterministic, ordered, and immutable](../invariants/checks.md#checks-the-selected-gate-battery-is-deterministic-ordered-and-immutable) |
| A selected step fails early | later selected steps still run where dependencies permit; one report names every selected failure | [Each selected step's failure is collected and the run continues](../invariants/checks.md#checks-each-selected-step-s-failure-is-collected-and-the-run-continues) |
| Every selected arm admits in an installed hook | nothing is printed | [A passing installed hook prints nothing](../invariants/checks.md#checks-a-passing-installed-hook-prints-nothing) |
| A required external tool is absent | typed unavailable evidence names it and the run refuses | [An arm that cannot execute returns a named refusal](../invariants/checks.md#checks-an-arm-that-cannot-execute-returns-a-named-refusal) |
| A measured study passes | the result retains the limits it applied | [A measured pass retains the limits it applied](../invariants/checks.md#checks-a-measured-pass-retains-the-limits-it-applied) |
| No file in scope matches a selected study | typed not-applicable evidence names the reason and declaration provenance; the run admits without claiming a measurement | [Not-applicable is explicit admissible evidence](../invariants/checks.md#checks-not-applicable-is-explicit-admissible-evidence) |
| A study is run directly | passed, not-applicable, excluded, and each refusal state remain distinct in its evidence | [Direct runs preserve typed terminal evidence](../invariants/checks.md#checks-direct-runs-preserve-typed-terminal-evidence) |
| A commit is refused after latch creation, healing, or rename propagation was proposed | every ledger remains byte-for-byte unchanged; the next attempt meets the same accepted state | [Gate state is published only with an accepted run](../invariants/checks.md#checks-gate-state-is-published-only-with-an-accepted-run) |
| Staging fails | configuration-contributed steps do not run | [Steps contributed by configuration are skipped unless the staging step passed](../invariants/checks.md#checks-steps-contributed-by-configuration-are-skipped-unless-the-staging-step-passed) |
| The same elastic policy and base compile on two machines | each produces the same checked integer or the same refusal | [Policy compiles an elastic ceiling to one checked integer](../invariants/checks.md#checks-policy-compiles-an-elastic-ceiling-to-one-checked-integer) |
| A value within flex is refused because it is latched | the diagnosis says held-at-base and names the ledger | [A refusal caused by a latch](../invariants/checks.md#checks-a-refusal-caused-by-a-latch) |
| A latched subject measures exactly at base during a failing run | retirement is proposed but not published; the latch remains | [A healed latch retires only with an accepted run](../invariants/checks.md#checks-a-healed-latch-retires-only-with-an-accepted-run) |
| The last latched subject heals | the ledger file is removed, not emptied | [An emptied ledger is deleted](../invariants/checks.md#checks-an-emptied-ledger-is-deleted) |
| A latched file is renamed | the latch follows to the new name | [Latches follow renames additively](../invariants/checks.md#checks-latches-follow-renames-additively) |
| Jitter is applied | the ceiling stays at or above base for every seed | [Jitter is applied over the headroom](../invariants/checks.md#checks-jitter-is-applied-over-the-headroom) |
| Two runs capture the same path, seed inputs, and policy | both resolve the same ceiling and retain the same seed source | [Jitter seed resolution has one provenance-retaining order](../invariants/checks.md#checks-jitter-seed-resolution-has-one-provenance-retaining-order), [Jitter is deterministic](../invariants/checks.md#checks-jitter-is-deterministic) |
| A claim's lifetime has elapsed when a run starts | treated as absent in that run; an accepted run prunes it, while a refusal leaves durable bytes unchanged | [Live breaches on shared subjects are claimed](../invariants/checks.md#checks-live-breaches-on-shared-subjects-are-claimed) |
| Two agents breach the same subject | the earlier claimant owns it, deterministically | [The earliest claimant owns a contested subject](../invariants/checks.md#checks-the-earliest-claimant-owns-a-contested-subject) |
| A subject is held by a peer | the finding informs and the commit passes, naming holder and expiry | [A finding whose subject is held by a peer informs and passes](../invariants/checks.md#checks-a-finding-whose-subject-is-held-by-a-peer-informs-and-passes) |
| A battery starts without an active actor | refused before measurement | [Every run binds one actor and run-start claims](../invariants/checks.md#checks-every-run-binds-one-actor-and-run-start-claims) |
| A revision-selected run observes live claims | they affect its verdict, but its reconciled proposal is rejected before durable publication | [Every run binds one actor and run-start claims](../invariants/checks.md#checks-every-run-binds-one-actor-and-run-start-claims) |
| A measure reports a limit | it declares which kind of limit it is | [A limit has one declared kind](../invariants/checks.md#checks-a-limit-has-one-declared-kind) |
| A differential measure cannot read its baseline | it does not report clean | [A measure whose baseline cannot be read must not report clean](../invariants/checks.md#checks-a-measure-whose-baseline-cannot-be-read-must-not-report-clean) |
| A file's working-tree bytes change after the staged capture | the run still judges the exact captured staged bytes | [A gate run captures the exact staged selection](../invariants/checks.md#checks-a-gate-run-captures-the-exact-staged-selection) |
| The index moves after the staged capture | refused before state publication; mixed generations are never judged | [The fully-staged rule is the intersection of staged and unstaged names](../invariants/checks.md#checks-the-fully-staged-rule-is-the-intersection-of-staged-and-unstaged-names) |
| Two selectors are given at once | refused rather than resolved by precedence | [Selectors are mutually exclusive](../invariants/checks.md#checks-selectors-are-mutually-exclusive) |
| A study runs over a repository in use | the appliance's runtime state is not scanned | [The appliance's own runtime state is excluded from every selector](../invariants/checks.md#checks-the-appliance-s-own-runtime-state-is-excluded-from-every-selector) |
| A file is staged in part | refused before any measure reads the tree | [The fully-staged rule is the intersection of staged and unstaged names](../invariants/checks.md#checks-the-fully-staged-rule-is-the-intersection-of-staged-and-unstaged-names) |
| An exception is granted | it is recorded in a form that can be listed and attributed | [Every waiver is declared and enumerable](../invariants/checks.md#checks-every-waiver-is-declared-and-enumerable) |
| An allowlist is written | it names the permitted thing, not a tolerated count | [An allowlist names the specific thing it permits](../invariants/checks.md#checks-an-allowlist-names-the-specific-thing-it-permits) |
| A scoped rule is written for a path set | the bound moves; the measure still runs | [A scoped rule retunes a bound over a declared path set](../invariants/checks.md#checks-a-scoped-rule-retunes-a-bound-over-a-declared-path-set) |
| A built-in step is disabled by configuration | the disablement is enumerable and audited | [A step may be disabled by configuration](../invariants/checks.md#checks-a-step-may-be-disabled-by-configuration) |
| A repository is cloned | the same waivers are in force | [Waivers are tracked](../invariants/checks.md#checks-waivers-are-tracked) |
| An unregistered study runs | it files nothing | [Only registered instruments may file work](../invariants/checks.md#checks-only-registered-instruments-may-file-work) |
| An instrument is run twice on an unfixed finding | the open task is reused, not duplicated | [Filing is idempotent against a finding's identity within its project](../invariants/checks.md#checks-filing-is-idempotent-against-a-finding-s-identity-within-its-project) |
| A finding identity passes through board normalization | it still resolves as a key | [A finding's identity survives the board's own normalization](../invariants/checks.md#checks-a-finding-s-identity-survives-the-board-s-own-normalization) |
| A finding recurs after its task was completed | a fresh task is filed, annotated with the resolved one | [A completed predecessor never blocks re-filing](../invariants/checks.md#checks-a-completed-predecessor-never-blocks-re-filing) |
| A study files work | it lands deferred and carries no due date | [Study work is filed deferred and carries no due date](../invariants/checks.md#checks-study-work-is-filed-deferred-and-carries-no-due-date) |
| A study files work with no origin given | the acting claim is inherited as the origin | [A study task carries the same required origin as any other task](../invariants/checks.md#checks-a-study-task-carries-the-same-required-origin-as-any-other-task) |
| An instrument reports findings | it neither pre-seeds nor suppresses its own filing | [Findings and filed work are coextensive](../invariants/checks.md#checks-findings-and-filed-work-are-coextensive) |
| A selected measure needs compiler-specific layout | it consumes the retained selected-source authority through a verified exact sealed mirror with separate outputs | [The retained selected tree is the sole source authority](../invariants/checks.md#checks-the-retained-selected-tree-is-the-sole-source-authority) |
| A measure is running | the caller's checkout is unmodified throughout | [The caller's checkout stays read-only for the whole measurement](../invariants/checks.md#checks-the-caller-s-checkout-stays-read-only-for-the-whole-measurement) |
| A private root remains after its owner exits | a later run acquires its companion lock before reclaiming it | [A companion lock is the liveness authority for a private root](../invariants/checks.md#checks-a-companion-lock-is-the-liveness-authority-for-a-private-root) |
| Cleanup is interrupted after removing a workspace | the retained companion lock lets a later run finish reclamation safely | [Cleanup removes the workspace before its lock](../invariants/checks.md#checks-cleanup-removes-the-workspace-before-its-lock) |
| Configuration carries an unknown key or an invalid bound | refused at load, not defaulted past | [Configuration fails loudly](../invariants/checks.md#checks-configuration-fails-loudly) |
