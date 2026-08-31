# Case battery — Core work

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### Steering

| Situation | Expected | Ref |
| --- | --- | --- |
| Submit while the lane is vacant | the live browser retains the draft in its ephemeral wake queue while lifecycle authority launches an occupant; only the post-wake `SteerSend` and returned live key establish journal durability, so browser death before that boundary may lose the prediction | [Submission lifecycle](../model/states.md#closed-loop-submission-a-draft-becoming-evidence), [An item is durable before delivery is attempted](../invariants/identity.md#steering-an-item-is-durable-before-delivery-is-attempted) |
| Send while an agent is in a non-shell tool span | remains live until the vendor's post-tool hook, then becomes eligible for one suppressed readout | [Pending steering reaches each admitted agent boundary](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary) |
| Send when no post-tool boundary occurs before the next shell command | becomes eligible at that top-level shell-command boundary; descendant shells are not instrumented again | [Pending steering reaches each admitted agent boundary](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary) |
| Send the same text twice | both persist; dedupe is not silent merging | [An item is durable before delivery is attempted](../invariants/identity.md#steering-an-item-is-durable-before-delivery-is-attempted) |
| Agent reads without acknowledging | count does not drop; the reminder ladder starts | [Reads never retire an item](../invariants/identity.md#steering-reads-never-retire-an-item) |
| Agent NACKs with a reason | retires durably as `declined`; the published browser message carries ordinary quote/body Markdown without disposition-specific styling | [Decline is a first-class terminal disposition](../invariants/identity.md#steering-decline-is-a-first-class-terminal-disposition) |
| One message acknowledges two items | both retire | [Reads never retire an item](../invariants/identity.md#steering-reads-never-retire-an-item) |
| Agent acknowledges an unknown key | typed `steer::ack-missed` refusal names the token as unknown and directs the agent to read the current inbox and acknowledge one live key in new prose | [A refusal that can name a way out leads with the exact repair action](../invariants/distribution.md#conduct-a-refusal-that-can-name-a-way-out-leads-with-the-exact-repair-action) |
| Send while the backlog is large | submit latency unchanged | [Submit cost does not grow with backlog depth](../invariants/identity.md#steering-submit-cost-does-not-grow-with-backlog-depth) |
| Item goes unanswered past its lifetime | moves visibly to *lapsed*; recovery preserves the same key and lineage | [Lapsed items remain recoverable](../invariants/identity.md#steering-lapsed-items-remain-recoverable) |
| Agent cannot be started at all | item moves to `deadlettered`; recovery preserves the same key and lineage | [An undeliverable item becomes `deadlettered`](../invariants/identity.md#steering-an-undeliverable-item-becomes-deadlettered) |
| Operator cancels a live item | terminal disposition is `canceled`, distinct from `acked` and `declined` | [Reads never retire an item](../invariants/identity.md#steering-reads-never-retire-an-item) |
| Operator resends | same lineage key; escalated presentation | [Escalation is driven by silence measured from publication](../invariants/identity.md#steering-escalation-is-driven-by-silence-measured-from-publication) |
| Browser dies between typing and sending | draft lost; nothing half-sent | [Submission lifecycle](../model/states.md#closed-loop-submission-a-draft-becoming-evidence) |
| Browser dies after bytes were sent but before a live durable key or covering backend standing returns | the ephemeral prediction may be lost; no durability claim has crossed yet | [Submission lifecycle](../model/states.md#closed-loop-submission-a-draft-becoming-evidence) |
| Browser dies after a live durable key returns | item is durable; authoritative count is correct on reload | [An item is durable before delivery is attempted](../invariants/identity.md#steering-an-item-is-durable-before-delivery-is-attempted) |
| The same readout would repeat | suppressed | [The steering readout is suppressed when it would repeat itself](../invariants/identity.md#steering-the-steering-readout-is-suppressed-when-it-would-repeat-itself) |
| Steering delivered on the hot path | no task-database query | [Submit cost does not grow with backlog depth](../invariants/identity.md#steering-submit-cost-does-not-grow-with-backlog-depth) |
| Item acknowledged | journal history records actor, team-at-send, disposition, and response; deleting the inbox projection loses no fact | [The filesystem inbox is a delivery projection](../invariants/identity.md#steering-the-filesystem-inbox-is-a-delivery-projection) |

### Claims and allocation

| Situation | Expected | Ref |
| --- | --- | --- |
| Two agents ask for work at once | distinct rows; no double claim | [At most one active claim per actor](../invariants/identity.md#work-at-most-one-active-claim-per-actor) |
| An active claim reaches its lease expiry | the timer submits one lapse transaction; accepted disposition becomes `lapsed`, then readiness is derived | [Lapse writes `lapsed` before readiness is derived](../invariants/identity.md#work-a-lapsed-claim-returns-to-ready-transactionally) |
| Explicit steal races a renewal | replaces exactly the named current claim, or refuses cleanly | [An explicit steal replaces exactly the named claim](../invariants/identity.md#work-an-explicit-steal-replaces-exactly-the-named-claim) |
| A live claim is explicitly stolen | no inbox item is fabricated; the displaced claimant's next claim operation receives a typed ownership refusal naming the current owner | [An explicit steal replaces exactly the named claim](../invariants/identity.md#work-an-explicit-steal-replaces-exactly-the-named-claim) |
| A lapsed claim appears after the lapse transaction | not counted as peer-held; counted ready only if every other predicate admits it | [A lapsed claim is not held by a peer](../invariants/identity.md#work-a-lapsed-claim-is-not-held-by-a-peer) |
| Solo agent faces only its own review | handed it, ranked last | [Self-authored review is ranked last, never excluded](../invariants/identity.md#work-self-authored-review-is-ranked-last-never-excluded) |
| Critical task blocked by a trivial one | the blocker inherits the critical priority | [A prerequisite inherits the highest priority it transitively unblocks](../invariants/identity.md#work-a-prerequisite-inherits-the-highest-priority-it-transitively-unblocks) |
| Two ready rows, equal priority | the one releasing more of the graph wins | [Allocation ranks in a fixed ladder](../invariants/identity.md#work-allocation-ranks-in-a-fixed-ladder) |
| Peers already occupy a cell | anti-affinity steers elsewhere | [Allocation ranks in a fixed ladder](../invariants/identity.md#work-allocation-ranks-in-a-fixed-ladder) |
| Agent holds a plan-phase claim and tries to implement | refused by the gate finding | [Implementation work is refused](../invariants/identity.md#work-implementation-work-is-refused) |
| Plan phase carries local commits | refused | [Implementation work is refused](../invariants/identity.md#work-implementation-work-is-refused) |
| Lease renewal happens mid-claim | `claim::renewed`; the same claim remains `active` | [Renew and carry are distinct active-claim facts](../invariants/identity.md#work-renewal-carries-a-claim-to-the-successor-without-restarting-it) |
| Occupant renewal succeeds mid-claim | `claim::carried`; the successor inherits active possession without a fresh win | [Renew and carry are distinct active-claim facts](../invariants/identity.md#work-renewal-carries-a-claim-to-the-successor-without-restarting-it) |
| A row's unrelated fields are edited | queue age unchanged | [Queue age has one durable origin](../invariants/identity.md#work-queue-age-has-one-durable-origin) |
| Allocator is dry right after a request | reports each parked category with its reason | [A dry allocator reports the board](../invariants/identity.md#work-a-dry-allocator-reports-the-board) |
| Board is populated but nothing is available | stated as the expected answer, not an error | [A dry allocator reports the board](../invariants/identity.md#work-a-dry-allocator-reports-the-board) |
| Board merely looks quiet, or pending is low | neither reads as permission to stop | [Permission to stop is bound to the one condition that grants it](../invariants/identity.md#work-permission-to-stop-is-bound-to-the-one-condition-that-grants-it) |
| An agent stops intentionally | a witness distinguishes it from no history | [A witness record distinguishes an intentional end](../invariants/identity.md#work-a-witness-record-distinguishes-an-intentional-end) |

### Capacity and restart

| Situation | Expected | Ref |
| --- | --- | --- |
| Two derived-ready tasks, every lane vacant | exactly one occupant starts | [Capacity follows work](../invariants/lifecycle.md#lifecycle-capacity-follows-work), [The normal wake trigger is two ready tasks](../invariants/lifecycle.md#lifecycle-the-normal-wake-trigger-is-two-ready-tasks) |
| One ready task | nothing starts until the starvation age | [A lone ready task is not stranded](../invariants/lifecycle.md#lifecycle-a-lone-ready-task-is-not-stranded) |
| Many tasks appear at once | occupants start one decision at a time | [Capacity follows work](../invariants/lifecycle.md#lifecycle-capacity-follows-work) |
| A row becomes derived-ready and is claimed immediately by a running occupant | the settle interval prevents a spurious wake | [A settle interval prevents a burst from beating a running agent's own done-then-next cycle](../invariants/lifecycle.md#lifecycle-a-settle-interval-prevents-a-burst-from-beating-a-running-agent-s-own-done-then-next-cycle) |
| A turn exits while its claim remains `active` | the lane is vacated, then a successor is launched onto the already-possessed task | [Vacant lanes resume active claims](../invariants/lifecycle.md#lifecycle-stopped-lanes-resume-held-claims) |
| That restart runs | no new claim reserved; the claim stays put | [The held-claim restart takes no new work and reserves nothing](../invariants/lifecycle.md#lifecycle-the-held-claim-restart-takes-no-new-work-and-reserves-nothing) |
| Successor decides not to continue | it frees the claim; readiness is then derived from the current row | [The held-claim restart takes no new work and reserves nothing](../invariants/lifecycle.md#lifecycle-the-held-claim-restart-takes-no-new-work-and-reserves-nothing) |
| A lane holding nothing declines the held-claim arm | the retry gate is still unspent for the next arm | [One retry-attempt bucket serves every wake arm for a lane](../invariants/lifecycle.md#lifecycle-one-retry-attempt-bucket-serves-every-wake-arm-for-a-lane) |
| Any wake arm refuses | the payload names which arm refused | [A refusal payload names which wake arm produced it](../invariants/lifecycle.md#lifecycle-a-refusal-payload-names-which-wake-arm-produced-it) |
| A lane will not stay up | inherited stop criteria answer, not a per-arm copy | [Stop criteria are inherited by every arm through the shared ensure path](../invariants/lifecycle.md#lifecycle-stop-criteria-are-inherited-by-every-arm-through-the-shared-ensure-path) |

### Integration

| Situation | Expected | Ref |
| --- | --- | --- |
| Clean landing | one merge, baseline as first parent, trailers attached | [The landing merge carries the task's key](../invariants/lifecycle.md#integration-the-landing-merge-carries-the-task-s-key) |
| Baseline already contains the work | fast-forward; no synthetic merge | [Phase-completion integration](../invariants/lifecycle.md#integration-phase-completion-integration) |
| Content conflict | complete recoverable merge state plus a recipe | [A conflict installs as a complete](../invariants/lifecycle.md#integration-a-conflict-installs-as-a-complete) |
| Landing touches paths outside the task footprint | refused, with each drifted path classified | [A landing whose first-parent diff leaves the task's own footprint is refused with a recipe](../invariants/lifecycle.md#integration-a-landing-whose-first-parent-diff-leaves-the-task-s-own-footprint-is-refused-with-a-recipe) |
| A synthesized integrated tree is rejected by `policy.admission` | tree remains checked out at HEAD for repair; nothing publishes until the repair is committed and completion retried | [A synthesized tree must pass the declared cheap admission](../invariants/lifecycle.md#integration-a-synthesized-tree-must-pass-the-declared-cheap-admission) |
| Publish races a peer | re-integrate against the new baseline; peer paths preserved | [A publish race re-integrates against the newly fetched baseline](../invariants/lifecycle.md#integration-a-publish-race-re-integrates-against-the-newly-fetched-baseline) |
| Ref update raced | restore the actual current head without another ref hook | [A publish race re-integrates against the newly fetched baseline](../invariants/lifecycle.md#integration-a-publish-race-re-integrates-against-the-newly-fetched-baseline) |
| Interrupted landing | tree is either pre-merge or a recoverable merge | [A conflict installs as a complete](../invariants/lifecycle.md#integration-a-conflict-installs-as-a-complete) |
| Remote unreachable at launch | lane starts anyway with a skip note | [Agent-launch advancement](../invariants/lifecycle.md#integration-agent-launch-advancement) |
| Lane is dirty at launch | work kept exactly as-is | [Agent-launch advancement](../invariants/lifecycle.md#integration-agent-launch-advancement) |
| Compaction replays activation | no git advance occurs | [Activation is deliberately not a fourth boundary](../invariants/lifecycle.md#integration-activation-is-deliberately-not-a-fourth-boundary) |
| Agent looks for a way to push or pull | none exists | [Agents never pull and never push](../invariants/lifecycle.md#integration-agents-never-pull-and-never-push) |
| Recovery recipe is run as printed | it succeeds in the state that printed it | [A recovery recipe names the resolved commit it applies to](../invariants/lifecycle.md#integration-a-recovery-recipe-names-the-resolved-commit-it-applies-to) |

### Lifecycle

| Situation | Expected | Ref |
| --- | --- | --- |
| Lane opened while vacant | history renders; nothing launches | [Only lifecycle authority launches or renews occupants](../invariants/lifecycle.md#lifecycle-only-the-lifecycle-authority-transitions-an-agent) |
| Page refreshed with many lanes | no launches | [Only lifecycle authority launches or renews occupants](../invariants/lifecycle.md#lifecycle-only-the-lifecycle-authority-transitions-an-agent) |
| Snapshot or metrics query issued | no launches | [Only lifecycle authority launches or renews occupants](../invariants/lifecycle.md#lifecycle-only-the-lifecycle-authority-transitions-an-agent) |
| Renewal mid-draft | lane draft, history, and scroll survive; the unchanged lineage retains its shared task policy | [A lane is an independently steerable worktree seat](../invariants/lifecycle.md#topology-a-lane-is-an-independently-steerable-worktree-seat) |
| Supervisor launch or session proof fails | supervisor phase is `failed`, never `running`; UI lane presentation remains independently derived | [Agent process](../model/states.md#closed-loop-agent-process) |
| Launch inherits an ambient thread id | refused | [Ambient thread ids are refused at launch](../invariants/lifecycle.md#lifecycle-ambient-thread-ids-are-refused-at-launch) |
| Agent starts | prompt is neutral; the ask is recovered from control-plane state | [The launch prompt is neutral](../invariants/lifecycle.md#lifecycle-the-launch-prompt-is-neutral) |
| Worktree deleted underneath a lane | closes on the next authoritative discovery | [A failed enumeration is not evidence of absence](../invariants/lifecycle.md#topology-a-failed-enumeration-is-not-evidence-of-absence) |
| Transient discovery failure | nothing closes | [A failed enumeration is not evidence of absence](../invariants/lifecycle.md#topology-a-failed-enumeration-is-not-evidence-of-absence), [Absence of evidence is not evidence of absence](../invariants/distribution.md#conduct-absence-of-evidence-is-not-evidence-of-absence) |

### Topology

| Situation | Expected | Ref |
| --- | --- | --- |
| Fuse two lanes | teams merge; one merged stream; both remain send addresses | [A fused host renders one merged stream](../invariants/lifecycle.md#topology-a-fused-host-renders-one-merged-stream) |
| Fuse a lane with a team it is already in | refused | [A fused host renders one merged stream](../invariants/lifecycle.md#topology-a-fused-host-renders-one-merged-stream) |
| Split a fused team | each member becomes its own team | [A fused host renders one merged stream](../invariants/lifecycle.md#topology-a-fused-host-renders-one-merged-stream) |
| Move a composer to another team | optimistic, then authoritative; failure re-pulls | [Team mutations use optimistic concurrency on a monotonic revision](../invariants/lifecycle.md#topology-team-mutations-use-optimistic-concurrency-on-a-monotonic-revision) |
| Reorder composers | neither stable accents nor messages move; only composer order changes | [Member order controls composition, not identity](../invariants/lifecycle.md#topology-member-order-controls-composition-not-identity), [Message order is a pure function of the messages](../invariants/lifecycle.md#topology-message-order-is-a-pure-function-of-the-messages) |
| Add a seventh member | admitted; the console keeps its six-share maximum, the finite palette may repeat, and explicit lane attribution stays unambiguous | [Palette size does not bound team membership](../invariants/lifecycle.md#topology-palette-size-does-not-bound-team-membership), [Accent derives from stable lane identity](../invariants/lifecycle.md#surface-accent-derives-from-stable-lane-identity) |
| A member momentarily fails to resolve | keeps its seat and its lane | [A member that momentarily fails to resolve keeps its seat and its lane](../invariants/lifecycle.md#topology-a-member-that-momentarily-fails-to-resolve-keeps-its-seat-and-its-lane) |
| Two browsers open the same board | both converge on authoritative topology; either may independently hide a team as presentation | [Topology is server truth](../invariants/lifecycle.md#topology-topology-is-server-truth), [Close team is browser-local presentation](../invariants/lifecycle.md#topology-close-team-is-browser-local-presentation) |
| Local storage is cleared | topology returns identically from the server | [Topology is server truth](../invariants/lifecycle.md#topology-topology-is-server-truth) |

### Reading surface

| Situation | Expected | Ref |
| --- | --- | --- |
| Message arrives with the reader more than 80px from the top | same-frame compensation protects the viewport | [A reader more than 80px from the top is compensated in the same frame](../invariants/lifecycle.md#surface-a-reader-more-than-80px-from-the-top-is-compensated-in-the-same-frame) |
| Message arrives with the reader 0–80px from the top | near-top behavior shows the push-down | [A reader more than 80px from the top is compensated in the same frame](../invariants/lifecycle.md#surface-a-reader-more-than-80px-from-the-top-is-compensated-in-the-same-frame) |
| Reduced motion is set | no tweens; compensation still lands | [Compensation lands regardless of motion preference](../invariants/lifecycle.md#surface-compensation-lands-regardless-of-motion-preference) |
| Image loads after its placeholder rendered | whole card is remeasured at its latched span; growth ripples, frozen shrink retains rows, and liquid shrink replays smaller | [Pending images are remeasured after resolution](../invariants/lifecycle.md#surface-pending-images-are-remeasured-after-resolution), [Growth ripples down along the original layout](../invariants/lifecycle.md#surface-growth-ripples-down-along-the-original-layout) |
| Image load fails | `error` replaces `pending` as the image state and the whole card is remeasured | [Image failure is distinct from still loading](../invariants/lifecycle.md#surface-image-failure-is-distinct-from-still-loading) |
| Lane hidden then revealed | repaint as a correction, not a rebuild | [Nothing animates until the board first settles](../invariants/lifecycle.md#surface-nothing-animates-until-the-board-first-settles), [A deferred render does not commit its fingerprint](../invariants/lifecycle.md#surface-a-deferred-render-does-not-commit-its-fingerprint) |
| Window resized | one replay at the settled width | [Layout is a pure function of creation order](../invariants/lifecycle.md#surface-layout-is-a-pure-function-of-creation-order) |
| An agent joins a populated team | member count does not directly alter card identity or style; if the new lineage weight changes the settled host width, exactly one full geometry replay repacks the cards | [A card whose position did not change receives no style write](../invariants/lifecycle.md#surface-a-card-whose-position-did-not-change-receives-no-style-write), [Presentation-only changes](../invariants/lifecycle.md#surface-presentation-only-changes), [Layout is a pure function of creation order](../invariants/lifecycle.md#surface-layout-is-a-pure-function-of-creation-order) |
| An age reaches its next visible-unit deadline | its label updates; no reflow | [Deadline-driven live text renders in fixed-width slots](../invariants/lifecycle.md#surface-deadline-driven-live-text-renders-in-fixed-width-slots) |
| A task count gains or loses a digit | the unpadded three-slot string may resize; every semantic slot remains present | [Task-count triples have dynamic width](../invariants/lifecycle.md#surface-task-count-triples-have-dynamic-width) |
| Any menu or picker opens | no layout box changes | [Menus, pickers, overlays, and drag ghosts are positioned out of document flow](../invariants/lifecycle.md#surface-menus-pickers-overlays-and-drag-ghosts-are-positioned-out-of-document-flow) |
| A no-op re-render occurs | zero style writes | [A card whose position did not change receives no style write](../invariants/lifecycle.md#surface-a-card-whose-position-did-not-change-receives-no-style-write) |
| Recorded layout events are replayed | identical positions | [Layout is a pure function of creation order](../invariants/lifecycle.md#surface-layout-is-a-pure-function-of-creation-order) |

### Audio

| Situation | Expected | Ref |
| --- | --- | --- |
| Two messages arrive together | one voice at a time | [At most one audio element sounds at a time](../invariants/lifecycle.md#surface-at-most-one-audio-element-sounds-at-a-time) |
| Operator stops speech | immediate, global, idempotent | [At most one audio element sounds at a time](../invariants/lifecycle.md#surface-at-most-one-audio-element-sounds-at-a-time) |
| Lane opened with old history | nothing older than the open auto-plays | [Nothing older than the moment the operator opened a lane auto-plays](../invariants/lifecycle.md#surface-nothing-older-than-the-moment-the-operator-opened-a-lane-auto-plays) |
| Speech backend is missing or reports failure | `Speech unavailable` is exposed; stream unaffected | [Speech is best-effort](../invariants/lifecycle.md#surface-speech-is-best-effort) |
| External speech rendering hangs | bounded timeout reports failure; stream unaffected | [Speech is best-effort](../invariants/lifecycle.md#surface-speech-is-best-effort) |
| Browser system synthesis emits neither end nor error | active head remains until global stop; stream remains usable | [Speech is best-effort](../invariants/lifecycle.md#surface-speech-is-best-effort), [At most one audio element sounds at a time](../invariants/lifecycle.md#surface-at-most-one-audio-element-sounds-at-a-time) |
| External pause from OS media keys | distinguished from a deliberate pause | [At most one audio element sounds at a time](../invariants/lifecycle.md#surface-at-most-one-audio-element-sounds-at-a-time) |
