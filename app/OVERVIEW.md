# App sidebar and task overview

## App sidebar

The app owns one content-agnostic, browser-local sidebar at the right edge:
backdrop, header, Close, scroll, sheet-local overlay slot, Escape, inert/ARIA
wiring, and focus return. Exactly one content type occupies it. Opening another
swaps in place without replaying the transition; Escape, backdrop, or Close
dismisses it and returns focus to a surviving launcher. A local overlay takes
Escape first while the sheet stays mounted and inert. Reduced motion removes
the transition.

## Task overview

Task activation opens the overview for the complete selected task set: an
ordered, stable-deduplicated list derived from the locally admitted board in
its offered order. Every selected root stays visible however large the set.

Work-scope selections are durable overlay locations with one canonical grammar:
`#/work` for all open execution work; `#/work/stem/public/{stem}` and
`#/work/stem/hidden/{stem}` for one visibility-qualified execution stem;
`#/work/planning` for open Designs and every open task transitively downstream
of at least one open Design dependency; `#/work/planning/public/{stem}` and
`#/work/planning/hidden/{stem}` for one planning stem;
`#/work/team/{uuid}` for a team; and `#/work/group/{room-stamp}` for a working
group. A stem is its admitted lowercase `[0-9a-z_]+` spelling and occupies one
literal path segment. Team and group routes retain only stable identity:
hydrated lane topology and working-group authority supply current membership,
and subsequent authority changes refresh the open selection. All-open and
all-planning remain valid when empty; a missing stem, team, or group returns to
Chat once the authority that decides it is available.

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

At 100% zoom a diagram draws at no more than its natural size, shrinking only
to fit and centered in the graph region; only zoom enlarges it. The graph is
unframed, with a margin at every zoom that keeps outer nodes, edges, and
badges off the region's edge. Each node's state badge straddles its top border
so it never covers the label, and same-rank nodes keep room between them for
those badges. A completed task wears a muted **Done** badge, quieter than
**Blocked**; a withdrawn one wears **Deleted**. A Design ready for proposal,
which marks its tab, wears **Ready for proposal** in the same live tone.
Unconnected parts of the graph never stack into one column: whole connected
components fill rows left to right, each row spanning no more ranks than the
node count's square root, rounded up, or the deepest component's ranks when
more, so unconnected roots form a near-square grid. Row placement draws
nothing and is never a relationship.

Open work falls into the board's exclusive standings, in their precedence: a
live claim reads **In flight**, a lapsed one **Stale**, and explicit set-asides
read **Deferred** or **Parked**. The resident's gate then decides **Waiting**,
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
a header strip: the handle, the title, an intent or standing pill, the phase
and flow, the claimant linked to its lane, the review assignment, and the
task's relative age. Its contract, room, and relationships follow, and one
closing **Provenance** section gathers every remaining scalar fact (state,
lease, origin, claim context, and repository boundary) in a single fact grid
with no sub-headings. There an instant reads as a relative age with its full
local time on hover, a lane, occupant, team, or resident generation reads by
name rather than identity, and an absent value reads as a dash. Evidence,
blockers, relationships, provenance, the task's room, and its closure appear
together rather than behind tabs or pagination. A section with nothing to show
(no contract text, no relationships, no room publications) collapses to one
muted line without a heading. Sections head with the sheet's h3 and their
parts with h4, and every sheet action is a compact control or a fact link, so
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
observation folds it back. It records each input as its
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
dependency edge until the operator accepts or rejects each line in its
composed candidate; there is no separate OOPS filing state or filing action.

The Spice view's **Drafts** section follows Jobs. It lists each open Draft the
operator can read, then keeps the Task/Plan/Design creator behind one compact
**New Draft** disclosure. Opening it reveals every creation field; closing it
returns focus to the same summary without changing any entered values. A new
Draft needs a title and an admitted public project, and takes an optional Plan
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

The app names humans from principal profiles, never Git. The connected
principal's own Principals row and inbox heading show their name, initials, and
Gravatar.
Activity, task steps, room lines, and quote bylines use the author's profile;
a quote shows initials with the full name as its accessible label. Historical
`operator` facts name the earliest-born principal.

Before subscribing, the app reads the connected-readable directory and uses
the caller's roles to select operator-only streams and reads. Any `principal::`
fact re-reads it: profile edits publish in place; role edits reconnect so reads
and subjects change together. A profile without a name reads as `human <id>`;
the historical actor reads as Operator.

## Principals in the global menu

After the global settings, the Spice view shows **Principals**. The connected
principal appears there once, as their own row marked `(you)`, and that row
seats their personal inbox with its heading, open count, items, and controls.
The row and inbox stand whether or not this connection may list principals,
and while the listing loads or is refused; until the principal directory
arrives the row reads as loading. An operator-role principal also sees every
principal the repository has admitted, oldest first. Each reads with its avatar
and initials, name, email, roles, capability standing (issued, local for the
earliest-born, whose capability each resident generation mints, or revoked),
and admission age with the exact instant on hover. Its controls send the
commands `spice principal` sends: **Edit profile** opens a form whose changed
fields replace the recorded ones, **Grant operator** or **Revoke operator**
changes the one role, and **Revoke capability** asks once more before retiring
an issued capability. **Admit a human** takes a required name, an optional
email, and the operator role; the reply's capability shows once above the list
as the opening link `ORIGIN/#CAPABILITY` with **Copy link** and **Done**, held
only in that panel and dropped with its connection. The listing re-reads on
every `principal::` fact, so another session's change shows at once. The
listing is operator-only: a principal without the operator role sees only their
own row and its inbox, and a listing the resident refuses reads beneath that
row as taking the operator role. One change is in flight at a time; its
progress and any refusal, such as the last operator's revocation, read under
the heading.

## Jobs in the global menu

After Principals, the Spice view shows
**Jobs**, every job every lane keeps, apart from the topology roster. Jobs
stays ahead of Drafts, OOPS, working groups, resident topology, and the **Provider
quota** section that ends the view. The list and its full-width inspection card
share the existing four-row slot. Empty, loading, filtered, and inspection
states retain that footprint; larger lists and evidence scroll inside it rather
than enlarging the sheet. Those bounded viewports retain native wheel and touch
scroll chaining. At either edge, arrow and page keys continue through the
containing Spice menu instead of trapping keyboard navigation in Jobs.

Rows show kind (▶ command, ◆ service, ● watch), agent accent and name, observed
start age, and the complete command on hover; unpublished starts show `?`, and
ended jobs awaiting delivery use hollow glyphs. Search covers agent, command,
and job key. Heading counts filter by kind or unheard endings; **Jobs** clears
filters.

Inspection shows command, working directory, keeper lifecycle, start, deadline,
ending evidence, and bounded output tail with an omission notice. **Summary**
and **Output** share the card; previous/next traverse matching jobs. Copy takes
the complete command or displayed output. Back returns to the list; Escape
first cancels a kill confirmation.

**Kill** requires running/starting evidence and current operator authority.
Confirmation names the exact job key, focuses **Cancel**, and asks the keeper
to stop that process group. Stopping, ended, unavailable, and absent jobs
cannot be killed. A stop is not replayed after reconnect, and neither
inspection nor stopping marks the agent's result delivered; an
operator-stopped ending still receives its ordinary agent notification.

The operator-only `lane::jobs-read`, `lane::job-read`, and `lane::job-stop`
operate on keeper records, not a second store. References carry the native
base52 job key plus encoded topology revision/lane slot. Topology movement
invalidates a reference before any stop marker; return to the list for a
current one. Catalogs refresh while shown, live inspection refreshes while
open, and disconnection or missing records is explicit.

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
same **Swap positions** and **Join into** choices by keyboard or tap, and the
menu's **Disband team** leaves every member as an unassigned lone seat. Escape
returns focus to whichever opened the menu. Joins stop at six members: a target
that would make a seventh is shown unavailable and sends nothing. Every
accepted arrangement, including disbanding, sends the complete topology at the
drawn revision. **Disband team** toasts its typed refusal and refreshes that
topology; cancellation or a removed source sends nothing.

## Lane menu

Each Chat lane's compact menu button shows that lane's provider emblem, or
omp's pi when the model names no recognized provider, and its hover title
names the model, effort, and fast mode the emblem stands for. It swaps that
lane's composer for its seat panel. The panel leads with what the
agent needs from the operator — consent requests to Allow or Deny in place,
held tasks iced, blocked, or in review, Designs awaiting a decision, a lease
near expiry, and direction left waiting — then the Wake, Drain, Remove, Renew,
and next-launch model, effort, and fast-mode commands and their refusals. Renew
submits at once; Drain and Remove confirm first. Effort and fast mode show the
running value beside the next launch, and the fast-mode status names which
level enables it; fast mode follows
[the lane loop](../docs/product/loops.md#l-lane--the-place). Presence,
lease age, current task, latest activity, waiting direction, the lane identity,
occupant, session, and worktree follow, beside **Create new team** and **Leave
all teams**. The button, Escape, or a click elsewhere in the team restores the
composer.
