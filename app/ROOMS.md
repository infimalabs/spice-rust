# Rooms and working groups

## Room authority and browser preferences

Every stream subscription, first connect and same-scope reconnect alike,
rebuilds one deterministic projection of task-keyed room timelines,
repository working groups, participant reasons, preparation evidence,
decisions, authorizations, and claim-bound stop requests. The `rooms` subject
pages the facts behind current room authority at the subscription horizon:
every room publication, the current head of the working-group partition and
of each proposal and authorization, each coordination's direction and current
steering head, and the repository's first principal admission. That admission
resolves historical operator-authored room authority to the same durable human
after reconnect; later admissions never move it.
Live facts strictly after that horizon continue it, so the operator's
live-start fact stream neither repeats nor misses authority. The projection is
unavailable until its snapshot completes.

The operator's fact stream starts live at the subscription horizon instead of
replaying the journal, so its first `facts` page carries only the newest lane
occupancy facts at or before that horizon (seats, occupant starts and renewals,
and every way an occupancy ends), oldest first, at most one retained page of
rows. The page has no continuation; the stream continues strictly after the
horizon.

Durable task stamps and room publication stamps key cards across reloads and
unavailable source bytes. The current board snapshot resolves an anchor's
displayed handle and terminal standing, an output's control, and whether a
stop's exact claim win is still active. Saved groups retain only their own
settled anchor heads: completed, operator-closed, and deleted rooms remain
named without admitting unrelated task history. A genuinely absent identity
is `missing`; absent board authority is `unavailable`.

Snapshot pages share one horizon, rise strictly, and never move backward. A
moved horizon, regressed page, live fact before the snapshot completes beyond
its horizon, live fact at or before it, or stream overflow invalidates the
projection and reconnects for a fresh snapshot. It is never merged
speculatively.

Working groups carry shared catalog organization and a shared Paused/Running brake.
Opened group consoles, their order and layout, composer drafts, reader and
scroll positions, speech, and focus remain browser preferences. Those
preferences cannot grant task, room, group, team, or lane authority, and a
group edit never mounts, reorders, or closes a browser view.
Hiding a team is likewise browser-local: its membership, claims, drafts,
lanes, and running work remain intact, and activating any member's lane link
reveals the team before focusing that lane's Chat composer.

The Spice sidebar's working-group catalog is the design-side twin of the team
roster and is drawn by the same roster (see [Resident topology in the global
menu](MENU.md#resident-topology-in-the-global-menu)).
It reads one shared partition: every group in partition order, each holding its
designs in order and one Paused or Running brake, guarded by one
revision. A group exists only while designs belong to it: it is never created
empty, never named, and never removed as an object. Its card is labelled from
its lead design, member zero, as a team is from its host. The whole card mounts
the group's console by click, Enter, or Space; its nested Design controls keep
their own actions. Its header carries the group's brake slider, the console's
own control: both show one setting, pending included, and a refusal or owed
shelving reads in the catalog notice. Using it neither mounts the console nor
lifts the card. The formed card's frame binds to the same stable base-flavor
accent index as the corresponding team or group console; member and Design
accent slots remain independent. The console's group menu can move or close
that browser-local console;
its **Disband working group** action sends `group::disband` at the displayed
partition revision, leaves every Design ungrouped, and causes every browser to
close the vanished group's console after authoritative room state arrives.
In that console a design's menu carries **New working group** and **Leave
group**, the twins of an agent's **Create new team** and **Leave all teams**:
the first moves the design alone into a fresh group and is offered only while
its group holds another design; the second takes it out of its group. They and
every drop send one `group::move` or `group::arrange` witnessing the partition
revision the browser last folded; the brake slider, in the console or on the
catalog card, sends `group::stance` under that same revision. A change that
partition revision no longer matches is refused rather than merged. A group
whose last design leaves or which is disbanded ends. Ending a group moves no
task: its designs and the work they produced stay on the board. While room
authority loads or reconnects, an open console keeps its last label and
geometry and closes nothing.

**Designs** lists the open Design tasks no group holds, as Unassigned Agents
lists lanes in no team; each tile there reads open, active, while a grouped
tile reads the standing its group's brake grants. Paused is passive; Running
and no group are active. Dropping a design on **New working
group** begins a Paused group containing only that design, standing where the
design stood in partition order or, for an ungrouped design, last. The catalog
notice reports every outcome; typed refusals such as `group::design-limit`,
`group::design-only`, and `group::partition-moved` leave every group unchanged.

Each opened group is a console in the team fleet, after the teams, read exactly
as a team console with tasks and agents swapped. Its base and durable Design
stepping follow the [console flavor contract](CONSOLE.md#design-standing),
independent of shared partition or browser-console order. A team's composers
target its lanes and its tabs are tasks; a group's composers target its designs
and its tabs are lanes. All
designs' publications interleave newest first in one packed plane, so no room
is paged or opened separately; durable task and publication stamps key the
cards, and linked bytes that are not present stay labeled source
placeholders. Each room card is a single top-level card shaped like a team
message card, with no card nested inside it and no publication key shown. Its
words lead with the relative age, the speaker in their lane's team accent and
linked to that lane's Chat composer when Chat seats it, an
arrow to any receivers, and, where applicable, the contribution's vote
(strongly for, for, evidence, against, or strongly against) or question, or Unheard on an operator post that
no current or later lane authority proves was heard. The Unheard pill's tooltip
and accessible label explain that no lane listens yet and that starting or
directing one will have the post read. Receipts and cited sources follow the
words. Listening, native work, and duties remain standing rather than
fabricated timeline events. A lane's departure from a Design's room is one
full-width divider, the
group's counterpart of a team compaction card and in its compact shape: two
rules in the design's accent around the leave's linked age, the leaving lane,
the design it left, and its clause, which alone yields width; the label's title
keeps the whole sentence. As on a team toolbar, the status band shows the
newest thing said in any of the group's designs, a contribution named by its
lane and vote, and no group name; a coordination never displaces it.

As on a team's rail, the group's total holds the right edge of its tab rail and
opens the group's work overview. Leftward from it, the rail lists each lane
that holds an active claim on an open task associated with any of the group's
Designs: the Design itself, a task with a direct dependency on that Design, or
selected work that retains the Design as its preparation controller. Removing
one Design edge releases an unselected task only from that Design; persistent
controller identity keeps selected work on its group rail. Another current
Design edge can associate the same task with another group. A lane enters and
leaves the rail exactly as
a task enters and leaves a team's rail as board facts change, appears once in
its canonical team accent, and replaces
the associated task handle. Activating an admitted lane opens its composer in
Chat, whose team plane holds its history; a lane with no Chat seat stays a
plain label.

Every Work tab's count badge, the global and stem totals, a team's total, and
a group's total, carries at most one dot seated on its rim at the top
inline-end diagonal, so the round badge cuts into the dot, coloured by the
most actionable state its own partition holds: danger for work only the operator
can unblock (a human-actionable wall or an environment hold), warning for a
Design whose proposal awaits the operator's decision, and the live tone for a
Design no lane holds whose pass cycle is ready for a new proposal. A blocked
state outranks a decision and a decision outranks a ready pass, so a tab never
shows two dots. The tab's title and accessible name lead with the winning
state's count, `2 need you`, `1 awaiting your decision`, or `1 ready for
proposal`, so the state never rests on colour alone; forced-colors mode draws
every dot in the system text colour.

A design's composer posts the guarded native room-post command to exactly that
design's room on **Post** or Control/Command+Enter. Its empty back uses three
plain lines ordered from durable context to live detail, never middot-joined:
`HANDLE: TITLE`; workflow phase, standing, and claimant, comma-separated; then
the directly dependent open-task count and room-post count. Those facts come
from the current board and room projection on every render. Quoting
an operator-authored card drafts a
`spice://quote/v1/room/ROOM/PUBLICATION/DIGEST` shard above that design's
composer, the same shard a team composer drafts, and steering delivery expands
it from the room's journal record. Room posts carry no images. The composer's
accessible label preserves the guarantee that a post is recorded in the room
and asks no lane for anything; no generic prose occupies the composer face. An
admitted reply clears the composer only while it still holds exactly the posted
text and shards and raises no success toast; a refusal keeps everything and
shows in the composer toast.
The design's menu carries its
[standing](CONSOLE.md#design-standing), the
[coordination controls](DESIGNS.md#room-coordination) that ask a seated lane
for a contribution or put the room aside while the Design is active, and its
preparation authority.

Operator posts and lane MSG publications share one card family: sender →
destinations and text. A lane's own bytes come from its transcript echo; absent
that, the text its native delivery carried stands in. An operator-direction
route names the principal that direction reaches; while that recipient read is
unavailable it names the route's `#KEY` concern, never the retired
context-free `operator` spelling. Each card separates being recorded in the
room from each route's receipt (pending, queued for a vacant lane and unheard,
vacant, unavailable, acked, declined, expired, or cancelled). These receipts
are not styled as review evidence or output authorization, which stay on their
exact proposal version and in immutable preparation authority.

Room prose renders as Markdown through the renderer task detail uses, exactly
as written: card words, an inbox item's body and quoted direction, a Design
pass justification, a withdrawal reason, and a decision's return reason and
comments. A card's signoff leads its first paragraph; receipts and controls
stay outside the rendered body.

Each recorded Design pass proposal also joins its room's conversation and its
group's plane at its journal position as one card: the proposing lane, the
recommendation and justification, its decision standing as tab-style badges
(Undecided, Withdrawn, Returned, Accepted, or a resolved proposal's Accepted
and Rejected output counts), and links that focus that version, its statement,
and each output on the Design's read view. The card carries no decision
controls; those stay on the version. There, a work proposal's every output
carries an Accept or Reject verdict the operator must give: no line starts
decided, **Accept all** and **Reject all** above the output list mark every
line at once, and Preview refuses, naming the undecided outputs, until each
has a verdict. The version asks no separate decision: text entered in its
**Return with feedback** field makes the decision a return, which sets the
verdicts aside without clearing them, and Preview names the derived outcome
(the accepted and rejected counts, or the return and its feedback) before
Submit.

The operator's inbox sits in the Spice sidebar's Principals section, inside
their own principal row beneath its avatar, name, and email, above Jobs, and
not in the shared Working groups catalog. Its
heading and open count stay above a viewport-bounded vertical scroller; ordinary
and unbroken card text wrap within the sidebar instead of widening or clipping
the global menu. The heading reads `<initials>'s inbox` (for example, "CA's
inbox") once the connected principal's recorded profile supplies initials, and
"Your inbox" until then; a profile edit retitles the same surface in place. It
catalogs every open operator item each lane addressed through `#KEY` of a
direction the operator sent it; the card names that direction by key and the
text the operator sent. Retained pre-cutover items name the task they
concerned, or show as unnamed when they named none; current packets cannot
create either.
Then the card shows its publication's full body. Its `MSG from LANE` line
links to the message card the lane sent it from, through that card's own
route, whenever that message is loaded; otherwise it stays plain text. A
lane's body stays in its transcript, so the app reads it once, with the
concerned direction's text, through the operator-only `room::conversation-text`
read, which projects the same text `spice://room/inbox` prints; the transcript
echo stands in while that read runs. Every current item offers **Reply**, ACK,
and NAK: Reply sends waking operator direction to its author lane and settles
the item, while ACK and NAK settle silently. **Recover** returns a lapsed
receipt to pending. The catalog keeps operator items in immutable admission
order, and every receipt row keeps one geometry in every state. Acting moves
keyboard focus to the acting card, and its outcome or refusal is reported in
the composer toast for a console card and beside the inbox for an inbox item.
While that open count is nonzero, the closed Spice button wears one small
neutral dot centered just outside its upper-end rim, clear of the pepper, and
its label names how many messages wait; the dot clears with the last open item
and never shows while room authority loads or reconnects.

**Close group** and the group order controls change only this browser.
Opened groups, their order, each design composer's draft, and the last focused
design composer use `spice:room-views:v2` local storage, and every change
applies to the current local state whenever it lands. An explicit open
focuses the named design's composer; otherwise the saved composer focus is
restored only while nothing else holds focus. Incoming cards, receipts,
conclusions, lane changes, and late hydration never mount, close, reorder, or
focus a console.

The repository-owned OOPS Design uses the ordinary Design room and proposal
surface. One typed repository generation authority, not the Design's title or
project spelling, selects the current open Design. The browser composes no
candidate for it: its report Plans wait behind it until the agent holding it
proposes, and that proposal decides like any other, Return included.
