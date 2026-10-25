# App sidebar and task overview

## App sidebar

The app owns one content-agnostic, browser-local sidebar at the right edge:
backdrop, header, Close, scroll, sheet-local overlay slot, Escape, inert/ARIA
wiring, and focus return. Exactly one content type occupies it. Opening another
swaps in place without replaying the transition; Escape, backdrop, or Close
dismisses it and returns focus to a surviving launcher. A local overlay takes
Escape first while the sheet stays mounted and inert. It scrolls without
painting a scrollbar. Reduced motion removes the transition.

## Task overview

Task activation opens the overview for the complete selected task set: an
ordered, stable-deduplicated list derived from the locally admitted board in
its offered order. Every selected root stays visible however large the set.

Work-scope selections are durable overlay locations with one canonical grammar:
`#/work` for all open execution work; `#/work/project/{project}` for open
execution work under one exact project; `#/work/planning` for open Designs and
every open task transitively downstream of at least one open Design
dependency; `#/work/planning/{project}` for that planning set within one exact
project; `#/work/team/{uuid}` for a team; and `#/work/group/{room-stamp}` for a
working group. A project is its complete admitted label as one literal path
segment, compared whole: it never merges with another project that shares its
leading text.
Team and group routes retain only stable identity: hydrated lane topology and
working-group authority supply current membership, and subsequent authority
changes refresh the open selection. All-open and all-planning remain valid when
empty; a missing project, team, or group returns to Chat once the authority
that decides it is available.

Opening a Work control replaces the current location with its canonical path.
Closing that route-owned overview replaces it with `#/chat`. Opening a project,
one task, a relationship lead, or another composed task set releases a Work
location before opening the browser-local selection. Reload, bookmark,
Back/Forward, direct hash edits, and `hashchange` all rehydrate the same
route-owned scope without creating a separate Work page or unmounting Chat.

The admitted-board graph renders first, including every selected root, roots
with no edges, and explicit outside-board or outside-selection neighbors. The
same graph offers Dependency, Lineage, Reviews, and Lifecycle lenses. A
Dependency edge points from each prerequisite to every dependent it blocks and
reads **then**. Design control uses that same edge: an unselected Plan flows
from each Design it depends on, and selection by one Design removes only that
Design's edge. No preparation-only edge or inherited-control shortcut reaches
work beneath the Plan.
Direction and depth/node controls bound only optional resident enrichment,
never the local selection; first-load, unavailable, retry, and truncation or
frontier states never replace or silently omit a selected root. A node the
open board lacks, such as a completed task, takes its handle, title, phase, and
status from the same-stamp task in that enrichment answer; admitted rows stay
authoritative, and an identity the answer does not resolve stays Missing.
Lineage preserves exact task, ACK, and origin identities. Review chains keep
author → reviewer → verdict order. Lifecycle facts keep their durable order.
Zoom, pan, reset, and node activation never change the selection.

Releasing a Design's selected outputs refreshes cached dependency enrichment
for both the controller and the released outputs. The open graph replaces the
old gating arrows with the released-work prerequisites without a page reload.

At 100% zoom a diagram draws at no more than its natural size, shrinking only
to fit and centered in the graph region; only zoom enlarges it. The graph is
unframed, with a margin at every zoom that keeps outer nodes, edges, and
badges off the region's edge. Each node's state badge straddles its top border
so it never covers the label, and same-rank nodes keep room between them for
those badges. A completed task wears a muted **Done** badge, quieter than
**Blocked**; a withdrawn one wears **Deleted**. A Design ready for proposal or
awaiting decision, either of which marks its tab, wears **Ready for proposal**
in the tab mark's live tone or **Awaiting decision** in its warning tone.
Unconnected parts of the graph never stack into one column: whole connected
components fill rows left to right, each row spanning no more ranks than the
node count's square root, rounded up, or the deepest component's ranks when
more, so unconnected roots form a near-square grid. Row placement draws
nothing and is never a relationship.

Open work falls into the board's exclusive standings, in their precedence: a
live claim reads **In flight**, a lapsed one **Stale**, and an explicit
set-aside reads **Deferred**. The resident's gate then decides **Waiting**,
**Awaiting decision**, or **Blocked** before unclaimed review,
environment **Held**, and **Ready** work, so each count equals `spice board`'s.
**Waiting** means every unresolved dependency or planning-control edge still
reaches work an agent can advance. **Blocked** is reserved for a missing
authority, missing prerequisite, cycle, or other non-advancing wall that needs
a human to repair; merely waiting its turn is never Blocked. A Design cycle
never hides such a wall beneath it: every dependent is Blocked. The Design's
own row remains **Awaiting decision** when that action is available, is Blocked
in another active cycle, or remains **Passive** outside the partition. A lapsed
claim likewise remains **Stale** on its own row. Awaiting-decision, Passive, and
Stale rows are transparent to a dependency walk, so the underlying gate decides
the dependent's standing.
A **Draft** row is prospective work its author is still editing. It counts in
no standing, and nothing offers it until `spice task file` turns it into Filed
work.

The graph region is as tall as the diagram needs at its width-limited fit, up
to a capped footprint beyond which the graph scrolls; zoom never resizes it. A
truncated typed read states how many relationships lie outside it in the map
controls beside a More nodes control that raises the node budget up to the
repository bound; it never lists the omitted edges. The Dependency lens lists
nothing below the graph, so the selected task details follow it directly; the
Lineage, Reviews, and Lifecycle lenses list only the evidence the graph does
not draw as a task node, in one fixed region below the graph. A first detail
read's placeholder holds a full sheet height. A status
change relabels in place: a trust approval in progress relabels its own
button. The general rule is [A changing region holds its largest
footprint](../docs/product/invariants/lifecycle.md#surface-a-changing-region-holds-its-largest-footprint);
retained reads and their feedback follow [Retained task detail does not move
for background
feedback](../docs/product/invariants/lifecycle.md#surface-retained-task-detail-does-not-move-for-background-feedback).
Task save, reconciliation, close, delete, and reconnect feedback shares the
app's lower out-of-flow toast area. Task lifecycle and ordinary action results
own separate stacked slots there, so either may replace or dismiss its own
message without covering or clearing the other at desktop or narrow widths.

Every selected task owns a full detail section below the graph. It opens with
a header strip: the handle, the title, an intent or standing pill, then the
pitch on its own line, then the phase and flow, the claimant linked to its
lane, the review assignment, and the task's relative age. Every task row the
app lists (a blocker row, an unfiled Draft row) likewise shows the title line
and the pitch line and nothing else of the statement. Beneath the strip the
sheet leads with title and pitch and collapses the rest: **Description**,
**Acceptance** (**Questions** for a Design), and **History** are native
`details` blocks closed on every open with no remembered open state, each
summary carrying its count (`Acceptance 5`), so the weight shows before the
prose. History gathers every remaining scalar fact (state, lease, origin,
claim context, and repository boundary) in a single fact grid with no
sub-headings. There an instant reads as a relative age with its full local
time on hover, a lane, occupant, team, or resident generation reads by name
rather than identity, and an absent value reads as a dash. Evidence, blockers,
relationships, history, the task's room, and its closure appear together
rather than behind tabs or pagination. A section with nothing to show (no
relationships, no room publications) collapses to one muted line without a
heading; an empty block says so when opened. Sections head with the sheet's h3
or a block's summary and their parts with h4, and every sheet action is a
compact control or a fact link, so
the sheet reads as one page. Activating a
selected graph node scrolls and focuses its detail section. Every identity the
sheet names navigates: a relationship inside the selection focuses its
section, while one outside it, a proposal output's handle, and a parent-task
origin replace the selection with that task; every lane name, from the
claimant and Standing to each room and contribution card's speaker, opens that
lane's Chat composer when Chat seats it and stays a plain label otherwise.
Journal fact codes and steering origins have no card to open and read as
text; a contribution's cited journal fact also names its fact kind and, while
the task it concerns exists, that task's handle as a link opening its sheet.
Each detail's
**Room** section seats its composer directly above the room's conversation,
newest first, so a submitted observation is published into the task's room
and appears right beneath the draft that wrote it. A Design's room is the same
section with its Design authority added and its permalink as a compact control
beside the heading. The composer is a single **Observe** control until the
operator opens it or it holds a draft, a refusal, or a submission; a landed
observation folds it back. Its actions are **Add**, which publishes the draft
as an observation, one completion beside **Delete** when the socket offers
settlement: **Close** for a task, or **Conclude** for an open Design, which
concludes it done and carries no text, so it refuses while a draft is
unpublished rather than discard it. **Close** and **Delete** publish the draft
as an observation too, and only once the room records it close the task with
that text as its validation or withdraw it with that text as its reason; a
refused observation settles nothing and keeps the draft. Each settlement
confirms first, saying what becomes of the text, witnesses the heads the detail
was drawn from, and names the live claim it takes. Control or Command+Enter is
**Add**; Shift with it is the offered completion, never Delete. A closed or
deleted task's room leads with its settlement card: who closed or deleted it,
when, its text, and any claim it took. The composer records each input as its
task-owned browser draft. A connection loss keeps that composer visible with
its exact text read-only and no publish action until the resident returns;
reconnect makes the same draft editable, and a generation reload hands it to
the replacement document without posting it. A selection of any size
hydrates every section: the app keeps a bounded number of detail reads in
flight beside its other reads and actions, asks for the rest as those answer,
and reads a task again after a change or reconnect only while it is selected.
A command or detail read that settles after selection moved creates no hidden
read; opening that task again reads its current detail.

A pending Filed task's **Deferral** fact changes when it returns in place, on
the fact's own line. **Clear**, shown only while the task is deferred, wakes it
through `task::wake`; **Defer**, labelled **Change deferral** while deferred,
opens the browser's date-time picker, and the chosen local time defers the
task until then through `task::defer`.
Each witnesses the workflow and deferral heads the fact was drawn from, so a
change made against a stale view refuses. A landed change re-reads the board,
so the fact shows the new time, or a dash once cleared, without a reload. The
outcome, or the refusal, stays on the same line, in full on hover. Either
button is disabled when the runtime does not offer its command.

**Running jobs** closes the sheet, after every task's details, in a fixed
footprint that scrolls beyond it. It is global: every job a lane still runs is
listed, whatever the selection. Each row's **Task** names the task the job runs
under, by handle when the board carries it and by stamp otherwise, and opens
that task's sheet: a selected task is focused in place, any other joins the
selection. A job started under no task shows a dash. Jobs under the selected
tasks come first; within each group rows keep job order.

## Task blocker review

Every task overview reserves one fixed-height status rail under its header. It
counts every task on the board that needs an operator act now, whatever
overview scope is selected: an unclaimed, undeferred Design awaiting the
operator's decision on its candidate, read as **Awaiting your decision**; a
task whose unsatisfied environmental requirement (trust grant, journal schema,
appliance version) needs the operator, unless that task is already set aside
(closed, Draft, parked, deferred, or a passive Design); and each
human-actionable Blocked gate or the task where its dependency chain stops. A
deferred task keeps its own explicit standing without entering this danger
queue merely for waiting; it enters only when a Blocked dependency chain
reaches it. Every task behind a blocked or decision tab mark is listed here or
holds behind a listed wall, so the rail reads **Needs attention** whenever any
tab carries either. A Design ready for proposal marks its tab as a notice but
never enters this queue: a lane, not the operator, acts next. Each
queue item appears once, in board order, however many reasons or held tasks
reach it. A dependency on work an agent can claim or is working, or on an open
Design whose underlying gate can advance, is ordinary **Waiting** and never
counts. A Design awaiting your decision counts once as itself, never through
its outputs. Claimable stale prerequisites, passive Designs, and
Awaiting-decision Designs remain transparent: when any has a real wall beneath
it, the queue names that underlying task and reason instead of blaming its
dependent.
A waiting board pairs the danger-styled `!` with **Needs attention**; otherwise a quiet `✓` pairs with
**Ready to proceed**, even while tasks wait on peers. Both states share one
geometry, and detail reads do not own the rail's data; an item leaves on the
board update that records the operator's act.

**Next** opens the next waiting task's own overview in board order, stepping
from the task the overview currently shows and wrapping after the last; it is
disabled while nothing waits. **Review** opens a sheet-local overlay with its
own scroll range; the graph and details stay mounted and inert behind it. Each
review card names one waiting task, why only the operator can move it, and the
held tasks behind it, with **Open task** closing the review on that task's
overview. Focus moves to **Back to overview**, Escape closes the review before
the overview, and closing the review returns focus to its launcher.

Trust approval names one exact digest. A grant action exists only when the
current board snapshot carries that digest as pended with worktree provenance.
A configuration never captured, or recorded without worktree evidence, directs
the operator to run `spice trust pend` from any worktree carrying the exact
configuration, not from the lane or path where the requirement appeared: the
pended path proves where those bytes were captured, not task ownership. A
revoked digest stays a visible blocker rather than offering a transition the
resident must refuse. For an approvable digest the browser revalidates the
task stamp, statement revision, pended standing, and producer evidence
immediately before dispatch, then sends the resident's pended-to-granted trust
transition. The action stays disabled while pending and until the board
refresh catches up; a refusal stays beside the digest with a retry while its
pended authority is current. Journal-schema and appliance-version blockers
carry no controls: the app never renders an action the resident cannot
perform.

## Drafts and OOPS intake

A task's filing state is either **Filed** ordinary board work or a **Draft**
its author is still shaping. A Draft asserts nothing and stays outside
allocation, claims, capture, dependency satisfaction, completion, team strips,
execution summaries, and preparation candidates until `spice task file`
admits it.

`spice task oops` instead records a filed Plan directly under the current
repository-owned OOPS Design. Those Plans stay behind that Design's ordinary
dependency edge until an agent holding it triages them into a proposal the
operator decides; there is no separate OOPS filing state or filing action.

The Spice view's **Drafts** section follows Jobs. It lists each open Draft the
operator can read, then keeps the Task/Plan/Design creator behind one compact
**New Draft** disclosure. Opening it reveals every creation field; closing it
returns focus to the same summary without changing any entered values. A new
Draft leads with its title and pitch, the one plain sentence saying why the
work matters, needs an admitted public project, and takes an optional Plan
or Design intent, description, and acceptance. Recording one opens it in task
detail, which shows the full editor beside the Draft's contract. **Save Draft**
writes only the fields that changed, including project, intent, and flow. The
editor's fields are browser-local: live board, room, connection, and task-detail
refreshes never rewrite text, selection, or focus. A newer admitted revision of
the same Draft updates only the fields the operator has not changed, or still
holds as their landed Save sent them, and the next Save witnesses that revision.
A refused Save keeps every typed field for retry, and opening another Draft
starts from that Draft's latest admitted content.
**File** validates the Draft as a complete ordinary Task, Plan, or Design. A
refusal names the field or filing option to repair and leaves the Draft
unchanged. Success replaces the Draft with its Filed task, which the allocator
can offer at once.
