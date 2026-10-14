# App sidebar and task overview

## App sidebar

The app owns one browser-local sidebar at the right edge. Its frame — backdrop,
header, Close, scroll, sheet-local overlay slot, Escape, inert and ARIA wiring,
and focus return — is content-agnostic.

Exactly one content type occupies the sidebar. Opening different content
while it is present swaps the content in place without replaying the opening
transition; Escape, backdrop, or Close then dismisses the sheet and returns
focus to a surviving launcher. A content-local overlay owns Escape ahead of the
sheet and leaves the sheet mounted but inert behind it. Reduced-motion mode
removes the sheet transition.

## Task overview

Task activation opens the overview for the complete selected task set: an
ordered, stable-deduplicated list derived from the locally admitted board in
its offered order. Every selected root stays visible however large the set.

The admitted-board graph renders first, including every selected root, roots
with no edges, and explicit outside-board or outside-selection neighbors. The
same graph offers Dependency, Lineage, Reviews, and Lifecycle lenses. A
Dependency edge points from each prerequisite to every dependent it blocks and
reads **then**.
Direction and depth/node controls bound only optional resident enrichment,
never the local selection; first-load, unavailable, retry, and truncation or
frontier states never replace or silently omit a selected root. A node the
open board lacks, such as a completed task, takes its handle, title, phase, and
status from the same-stamp task in that enrichment answer; admitted rows stay
authoritative, and an identity the answer does not resolve stays Missing.
Lineage preserves exact task, ACK, and origin identities. Review chains keep
author → reviewer → verdict order. Lifecycle facts keep their durable order.
Zoom, pan, reset, and node activation never change the selection.

At 100% zoom a diagram draws at no more than its natural size, shrinking only
to fit and centered in the graph region; only zoom enlarges it. The graph is
unframed, with a margin at every zoom that keeps outer nodes, edges, and
badges off the region's edge. Each node's state badge straddles its top border
so it never covers the label, and same-rank nodes keep room between them for
those badges. A completed task wears a muted **Done** badge, quieter than
**Blocked**; a withdrawn one wears **Deleted**.

Open work falls into the board's exclusive standings, in their precedence: a
live claim reads **In flight**, a lapsed one **Stale**, a deferral
**Deferred**, then the allocator's gate decides **Blocked**, an unclaimed
review reads **Review**, an unmet environment requirement **Held**, and the
rest **Ready**, so each count equals `spice board`'s. The gate is the
allocator's verdict: the resident's open board names every row it holds back,
judged over terminal evidence the browser never receives, so an unauthorized
controlled output reads Blocked exactly when `task next` skips it. A row in a
hidden project, such as a filed OOPS report, reads **Parked**: the allocator
never offers it until it is promoted.

The graph region holds one fixed footprint whatever the diagram's shape. A
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

Every selected task owns a full detail section below the graph. Ownership and
claim, state and contract, project and team, timestamps and lifecycle, origin
and dependencies answer who, what, where, when, and why from live operational
state. Evidence, blockers, relationships, provenance, activity, and notes
appear together rather than behind tabs or pagination. Activating a selected
graph node scrolls and focuses its detail section; activating a related
external task follows that lead in the same overview. Each detail's Activity
section seats its observation composer directly above the newest-first
observations, so a submitted observation appears right beneath the draft that
wrote it. A selection of any size
hydrates every section: the app keeps a bounded number of detail reads in
flight beside its other reads and actions, asks for the rest as those answer,
and reads a task again after a change or reconnect only while it is selected.
A command or detail read that settles after selection moved creates no hidden
read; opening that task again reads its current detail.

**Running jobs** closes the sheet, after every task's details, in a fixed
footprint that scrolls beyond it.

## Task blocker review

Every task overview reserves one fixed-height status rail under its header. It
counts only walls the operator must move: unsatisfied environmental
requirements (trust grant, journal schema, appliance version) and dependency
chains no agent can advance because a link is deferred, cyclic, closed without
completing, or held by such a requirement. A dependency on work an agent can
claim or is working is an ordinary wait and never counts. A walled selection
pairs the danger-styled `!` with **Needs attention**; otherwise a quiet `✓`
pairs with **Ready to proceed**, even while tasks wait on peers. Each review
card names the task where its chain stops and why only the operator can move
it. Both states share one geometry, and detail reads do not own the rail's
data.

**Review blockers** opens a sheet-local overlay with its own scroll range; the
graph and details stay mounted and inert behind it. Focus moves to **Back to
overview**, Escape closes the review before the overview, and closing the
review returns focus to its launcher.

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
