# App sidebar and task overview

## App sidebar

The app owns one browser-local sidebar at the right edge. Its layer, frosted
backdrop, sticky header, Close action, scroll frame, sheet-local overlay slot,
Escape ownership, close transition, inert and ARIA wiring, and focus return are
content-agnostic. The sheet is 46rem wide, 42rem at or below 1180px, and the
full window at or below 560px.

Exactly one closed content type occupies the sidebar. Opening different content
while it is present swaps the content in place without replaying the opening
transition; Escape, backdrop, or Close then dismisses the shared sheet and
returns focus to a surviving launcher. A content-local overlay owns Escape
ahead of the sheet and leaves the sheet frame mounted but inert behind it.
Reduced-motion mode removes the shared sheet transition.

## Task overview

Task activation opens the task overview content for the complete selected task
set. The selection is an ordered, stable-deduplicated list: every selected root
remains visible even when the set is larger than the relation command's former
root or node limits. Scope and project entry points derive that list from the
locally admitted board in its offered order.

The admitted-board graph renders first, including every selected root, roots
with no edges, and explicit outside-board or outside-selection neighbors. The
same graph offers focused Dependency, Lineage, Reviews, and Lifecycle lenses.
Dependency direction and typed depth/node controls bound optional resident
enrichment without bounding the local selection; first-load, unavailable,
retry, and explicit truncation/frontier states never replace or
silently omit selected roots. A node the open board lacks, such as a completed
task, takes its handle, title, phase, and status from the same-stamp task in
that one enrichment answer, in place; admitted rows stay authoritative, and
an identity the answer does not resolve stays Missing.
Lineage preserves exact task, ACK, and origin identities. Review chains retain
phase author → reviewer → verdict order. Lifecycle facts retain their durable
source order. Zoom, pan, reset, and task-node activation remain available
without changing the selection. At 100% zoom a diagram draws at no more than
its natural size, shrinking only to fit and sitting centered in the graph
frame; zoom alone enlarges it. Each node's state badge straddles its top border
so it never covers the node's label.

The sheet reserves geometry: a region whose read can change holds one fixed
footprint from first paint, so loading, enrichment, other lanes' board
changes, and jobs starting or finishing move nothing below it. The graph frame
is 18–28rem viewport-relative whatever the diagram's shape. The relationship
list and any truncation scroll inside one 14–24rem viewport-relative region.
**Running jobs** always holds five and a half rows (two and a half cards when
narrow) and scrolls beyond them. A first detail read's placeholder holds a full
sheet height, so its answer moves nothing still on screen. Status changes
replace text in place: a trust approval in progress relabels its own button.
Only a shown task's own detail content may grow or shrink in place.

Every selected task then owns a full detail section below the graph. Current
ownership and claim, task state and contract, project/team location, timestamps
and lifecycle, origin and dependencies answer who, what, where, when, and why
from live operational state. Evidence, blockers, relationships, provenance,
activity, notes, first-load, unavailable, and retry states appear simultaneously
rather than behind tabs, pagination, or next/previous controls. Activating a
selected graph node scrolls and moves focus to that task's detail section;
activating a related external task follows that lead in the same overview.
Reduced-motion mode also removes graph transitions.

Replacing a retained relationship or detail read keeps the admitted answer on
screen and shows, toasts, and announces nothing. A first detail read holds a
prose-free placeholder footprint. Unavailable detail or enrichment and a
capability without enrichment toast once on the root toast when they begin;
the only in-place trace is **Retry detail** inside the absent detail's
placeholder or **Retry enrichment** inside the graph frame.

## Task blocker review

Every task overview reserves one fixed-height status rail directly under its
header. It counts only walls the operator must move: unsatisfied typed
environmental requirements (trust grant, journal schema, appliance version)
and dependency chains no agent can advance because a link is deferred, cyclic,
closed without completing, or held by such a requirement. A
dependency on work an agent can claim or is working is an ordinary wait and
never counts. A walled selection pairs the danger-styled `!` with **Needs
attention**; otherwise a quiet `✓` pairs with **Ready to proceed**, even while
tasks wait on peers. Each review card names the task where its chain stops
and why only the operator can move it.
Both states occupy the same geometry, and asynchronous detail reads do not own
the rail's data, so loading and replacing task cards cannot move the graph or
surrounding content.

**Review blockers** opens a sheet-local overlay instead of adding a tab or
expanding the overview. The graph and task details remain mounted and inert
behind it; the overlay has its own scroll range. Focus moves to **Back to
overview**, Escape closes the blocker review before it closes the task
overview, and closing the review returns focus to its launcher.

Trust approval names one exact digest. A grant action exists only when the
current board snapshot carries that digest as pended with nonempty
producer-worktree evidence. Missing, revoked, and legacy-unbound heads remain
visible blockers with capture guidance rather than offering a transition the
resident must refuse. For an approvable digest, the browser reserves the
action, revalidates the originating task stamp, statement revision, pended
standing, and producer evidence immediately before dispatch, then sends the
resident's existing pended-to-granted trust transition witness. The action
stays disabled while pending and while the successful board refresh catches
up. A refusal remains beside the digest with a retry action while its pended
authority remains current. Journal-schema and appliance-version blockers are
framed in the same overlay without controls until real upgrade and reload
operations exist; the app never renders a fake action for them.
