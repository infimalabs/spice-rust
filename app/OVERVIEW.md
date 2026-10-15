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
reads **then**. The same lens draws a **then** arrow from each Design to every
Plan it prepared, so the Design's Plans flow from it rather than stacking; a
selected Plan's controller outside the selection joins as an outside neighbor.
That arrow presents the recorded preparation and is never a dependency or an
inherited-control shortcut to work beneath the Plan.
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
typed OOPS filing state reads **Parked**: its public project remains visible,
but the allocator never offers it until the operator files it as ordinary work.
A **Draft** row is prospective work its author is still editing. It counts in
no standing, and nothing offers it until `spice task file` turns it into Filed
work.

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
counts every task on the board only the operator can move, whatever overview
scope is selected: a Design awaiting the operator's selection of its outputs,
whatever its standing or claim; a task whose unsatisfied environmental
requirement (trust grant, journal schema, appliance version) needs the
operator, unless that task is already set aside (closed, Draft, parked,
deferred, or a passive Design); and the task where a held task's dependency
chain stops because a link is deferred, parked, cyclic, closed without
completing, passive, awaiting selection, or held by such a requirement. Each
such task counts once, in board order, however many reasons or held tasks
reach it. A dependency on work an agent can claim or is working is an ordinary
wait and never counts. A waiting board pairs the danger-styled `!` with
**Needs attention**; otherwise a quiet `✓` pairs with **Ready to proceed**,
even while tasks wait on peers. Both states share one geometry, and detail
reads do not own the rail's data; an item leaves on the board update that
records the operator's act.

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

## Drafts and OOPS

A task's filing state is one of three standings. **Filed** work is ordinary
board work. An **OOPS** report asserts unexpected behavior and stays parked
until it is filed through `spice task file` or its task detail's **File**.
A **Draft** is prospective work its author is still shaping. It asserts
nothing, never joins OOPS grooming, and stays outside allocation, claims,
capture, dependency satisfaction, completion, team strips, and execution
summaries until the same `spice task file` transition admits it.

The Spice view's **Drafts** section follows Jobs. It lists each open Draft the
operator can read, then keeps the Task/Plan/Design creator behind one compact
**New Draft** disclosure. Opening it reveals every creation field; closing it
returns focus to the same summary without changing any entered values. A new
Draft needs a title and an admitted public project, and takes an optional Plan
or Design intent, description, and acceptance. Recording one opens it in task
detail, which shows the full editor beside the Draft's contract. **Save Draft**
writes only the fields that changed, including project, intent, and flow.
**File** validates the Draft as a complete ordinary Task, Plan, or Design. A
refusal names the field or filing option to repair and leaves the Draft
unchanged. Success replaces the Draft with its Filed task, which the allocator
can offer at once.

The **OOPS** section follows Drafts and lists each open OOPS report the
operator can read, each opening its task detail. There **File** sits in the
Activity composer's action row beside **Add**, so the operator can read a
report, record observations, and file it in one place. File takes no input: it
files the report under its current project, and the allocator can offer the
result at once. Filing witnesses the statement, workflow, and deferral heads
the control was drawn from, so a report that changed meanwhile refuses. A
refusal is shown beneath the composer and leaves the report parked. Success
replaces the report with its Filed task under that project's handle.
Re-homing a report under another project, filing under a Design's preparation,
or filing and claiming at once stays with `spice task file`.

## Resident topology in the global menu

The Spice view leads with connection state, the inspectable version stack, and
the theme selector. Global TURBO stands beside the Spice button in its header,
starting where the theme selector starts, with the weekly percentage above the
button. The Spice launcher stays fixed
while the sidebar opens and closes; TURBO and the percentage enter and exit
with the fading sidebar surface instead of leading or trailing it. An available
app generation turns the version into the explicit reload control, sized to its
own label rather than stretched across the row.
A superseded app generation keeps running until its session
fails against the resident serving the replacement; that failure reloads the
tab into the served generation once per generation, unless a composer draft or
unsettled submission would be lost, in which case the tab stays and reconnects
behind the reload control.

Resident topology is one interactive roster, and the working-group catalog is
the same roster over designs. Each team lists its agents with presence, name,
worktree, model, pending steering, liveness, held task, kept jobs, and provider
emblem; a teamed agent opens its composer in Chat.

One roster component draws both kinds. Formed groups stand in their shared
order, each a card named for member zero holding its members' tiles; **New
Team** or **New working group** follows, then **Unassigned Agents** or
**Designs**, which lists only members no group holds. While the kind's
configuration and arrangement are admitted, every tile and card drags; a mouse
drags the whole tile or card, a finger or pen drags a tile by its name or a
card by its header, so the rest still scrolls the sheet and a tap still opens
Chat, and holding a lift near the sheet's edge scrolls it toward an off-screen
target. A member released on the New target begins a group holding only it; on
a card, or a tile in another group, joins that group; on a tile in its own
group takes that place; on the ungrouped list leaves its group, and a group
whose last member leaves ends. A card released on another card or its tiles
takes that place among the groups. Reordering restates the whole arrangement
(`team::arrange` or `group::arrange`); every other release moves one member.
A release where nothing would change, a group's only member on New, or a card
on New or the ungrouped list sends nothing. Groups stop at six members: a full
group's card and tiles disable their mouse and touch drop target, while its
members can still reorder or leave, and the authority refuses a stale client's
seventh member. Every change carries the revision the roster was drawn from.
No roster gesture mounts, reorders, closes, or focuses a view.

## Who the app names

The app names each human by their principal's recorded profile, never by Git
configuration. The operator panel's badge and the inbox heading show the
connected principal's own name, initials, and Gravatar, so two humans connected
at once each see themselves. Every authored item names its author's principal:
a task's activity and steps, a room line, and a quote byline, which shows the
author's initials with the full name as its accessible label. Facts the
historical `operator` actor recorded read as the earliest-born principal. The
principal directory rides the global settings read, and any `principal::` fact
makes the app re-read it, so a profile edit reaches every open app without a
reconnect. A human whose profile records no name reads as `human <id>`, and the
historical actor as Operator.

## Jobs in the global menu

Immediately after the operator panel and its inbox, the Spice view shows
**Jobs**, every job every lane keeps, apart from the topology roster. Jobs
stays ahead of Drafts, OOPS, working groups, resident topology, and the **Provider
quota** section that ends the view. The list and its full-width inspection card
share the existing four-row slot. Empty, loading, filtered, and inspection
states retain that footprint; larger lists and evidence scroll inside it rather
than enlarging the sheet. Those bounded viewports retain native wheel and touch
scroll chaining. At either edge, arrow and page keys continue through the
containing Spice menu instead of trapping keyboard navigation in Jobs.

Rows show kind (▶ command, ◆ service, ● watch), agent accent and name, observed
start age, and the complete command in the tooltip. Ended jobs awaiting agent
delivery use hollow glyphs. Search matches agent, command, or job key; the
heading counts also filter by kind or unheard endings. Click **Jobs** to clear
the filters. An unpublished start displays `?`, not an invented age.

Open a row to inspect its command, working directory, keeper lifecycle, start,
deadline, and ending evidence. **Summary** and **Output** share the same card;
previous/next navigate matching jobs without opening another pane. Output is
the keeper's bounded tail, with an explicit notice when earlier output was
omitted. Copy copies the complete command or the displayed output. Back returns
to the list; Escape first cancels a kill confirmation, otherwise returns.

**Kill** requires running/starting evidence and current operator authority.
Confirmation names the exact job key, focuses **Cancel**, and asks the existing
keeper to terminate that command's process group. Stopping, ended, unavailable,
and no-longer-cataloged references cannot be killed from the card. A sent stop
is not replayed after reconnect; refresh reads what the keeper actually did.
Inspection and operator termination never mark the agent's result as delivered.
An operator-stopped ending remains eligible for its ordinary agent notification.

The operator-only `lane::jobs-read`, `lane::job-read`, and `lane::job-stop` reads
and control operate on the existing keeper records, not a second job store.
Their job payloads use the native base52 job key and an encoded topology
revision/lane slot, not a new UUID identity. Changed topology invalidates the
reference before any stop marker is written; return to the list for a current
reference. Existing lane summaries and retained keeper data keep their current
shape. Catalogs refresh while the panel is visible; live inspection refreshes
while its card is visible. Disconnection and missing records are explicit.

## Provider quota rows

**Accounts lacking usage** and **Disabled credentials** appear only when they
have entries. Each account-limit row shows the limit name, a capacity bar
filled with what remains, the amount — `N% left` for a percent limit, `R / L
unit left` for a counted one — and the reset cadence. An account sharing the
weekly limit wears the same accent as its layer in the Spice button's quota
fill; others stay neutral.

The reset-credit summary states available and redeemable counts, the available
count standing in when the provider omits redeemable. Each redeemable credit
shows its provider title, program, and remaining count when present, its grant
age (`Granted at unknown time` when absent or invalid), and any valid expiry as
a countdown, exact times on hover. A provider-reported raw credit balance shows
on its own account as whole credits, truncated so a fraction never reads as
more than can be spent; it is neither a time-window meter nor a reset grant,
accounts never share or aggregate it, and Spice never infers it.

## Arranging teams

A team is named by its lead agent, canonical member zero, with the label its
messages carry: the configured name, else the worktree leaf. Only a team with
no current members keeps its short id.

Each team console takes its whole base from its age among the teams: teams
count up from the palette's start by when each became a team, so the oldest
wears green, the next blue, then gold, and an unteamed agent takes no place. The
lead wears that base and each later member the next slot up. Colours follow only
that age, so swapping two teams, or opening, closing, or reordering consoles in
one browser, changes none; only a team forming or an older one ending moves a
colour.

Pick up a team from its pip matrix: dropping on the middle of another team
swaps the two teams' positions; dropping on either outer edge joins the source
into that team, which keeps its identity, primary composer, and member order and
appends the source members in order. The pip stack and the team menu open the
same **Swap positions** and **Join into** choices by keyboard or tap; Escape
returns focus to whichever opened them. Joins stop at six members: a target
that would make a seventh is shown unavailable and sends nothing. Every
accepted arrangement sends the complete topology at the drawn revision;
cancellation, a changed revision, or a removed source sends nothing.

## Lane menu

Each Chat lane's compact menu button uses a smaller three-bar glyph than the
team menu, matching its smaller control without shrinking the pointer target.
It swaps that lane's composer for its seat panel. The panel leads with what the
agent needs from the operator — consent requests to Allow or Deny in place,
held tasks iced, blocked, or in review, a lease near expiry, and direction left
waiting — then the Wake, Drain, Remove, Renew, and next-launch model, effort,
and fast-mode commands and their refusals. Renew submits at once; Drain and
Remove confirm first. Effort and fast mode show the running value beside the
next launch, and the fast-mode status names which level enables it; fast mode
follows [the lane loop](../docs/product/loops.md#l-lane--the-place). Presence,
lease age, current task, latest activity, waiting direction, the lane identity,
occupant, session, and worktree follow, beside **Create new team** and **Leave
all teams**. The button, Escape, or a click elsewhere in the team restores the
composer.
