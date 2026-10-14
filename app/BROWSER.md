# Browser checks

## Browser evidence follows the host system theme

Use the repository-owned `launchAppBrowser` helper for disposable app browser
checks. It gives the headless context the host's `prefers-color-scheme`, so
callers do not repeat theme setup:

```js
import { chromium } from "playwright";
import launchAppBrowser from "./app/browser/app.mjs";

const app = await launchAppBrowser(chromium);
try {
  await app.page.goto(appUrl);
  // Inspect or capture the actual app surface.
} finally {
  await app.close();
}
```

An intentional theme-matrix check passes `contextOptions: { colorScheme }`; the
returned `colorScheme` and `colorSchemeSource` report the value and whether it
came from `"system"` or `"explicit"`. A stored `spice:app-theme` preference
still wins over the browser preference.

The [glass control studies](design/glass-controls.html) are an interactive
reference gallery for the [team composer
controls](CONSOLE.md#team-composer-controls). They send no commands, and
editing them changes no production style.

## Browser branding

The tab title and menu use the lowercase brand `spice`, trailing any changing
state: `5 tasks - spice`, `1 task - spice`, `7 tasks (stale) - spice`,
`Loading - spice`, `Disconnected - spice`, and `spice` before the app loads.
Once a board snapshot arrives, the title and the pepper icon's badge carry the
exact remaining public task count; the badge is exact through 99, then `99+`,
and zero is explicit. Reconnect keeps the last count marked stale until current
data returns. Speech never retitles the tab.

## Room authority and browser preferences

Every stream subscription, first connect and same-scope reconnect alike,
rebuilds one deterministic projection of task-keyed room timelines,
repository working groups, participant reasons, preparation evidence,
decisions, authorizations, and claim-bound stop requests. The `rooms` subject
pages the facts behind current room authority at the subscription horizon:
every room publication, the current head of the working-group partition and
of each proposal and authorization, and each coordination's direction and
current steering head.
Live facts strictly after that horizon continue it, so the operator's
live-start fact stream neither repeats nor misses authority. The projection is
unavailable until its snapshot completes.

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

Working groups are shared catalog organization only. Opened group consoles,
their order and layout, composer drafts, reader and scroll positions, and focus
remain browser preferences. Those preferences cannot grant task, room, group,
team, or lane authority, and a group edit never mounts, reorders, or closes a
browser view.

The Spice sidebar's working-group catalog is the design-side twin of the team
roster and is drawn by the same roster (see
[Resident topology in the global menu](#resident-topology-in-the-global-menu)).
It reads one shared partition: every group in partition order, each holding its
designs in order, guarded by one revision. A group exists only while designs
belong to it: it is never created empty, never named, and never removed as an
object. Its card is labelled from its lead design, member zero, as a team is
from its host. A group's **Open** mounts its console, as a team's watchband
shows it, and is the only foreground action. In that console a design's menu
carries **New working group** and **Leave group**, the twins of an agent's
**Create new team** and **Leave all teams**: the first moves the design alone
into a fresh group and is offered only while its group holds another design;
the second takes it out of its group. They and every drop send one
`group::move` or `group::arrange` witnessing the partition revision the browser
last folded, and a change that partition revision no longer matches is refused
rather than merged. A group whose last design leaves ends, and any already-open
local console becomes an unavailable placeholder once a completed room snapshot
lacks the group; while room authority loads or reconnects, the console keeps
its last label and geometry.

**Designs** lists the open Design tasks no group holds, as Unassigned Agents
lists lanes in no team; each tile there reads open, passive, and each grouped
tile reads its [standing](CONSOLE.md#design-standing). Dropping a design on
**New working group** begins a
group holding only that design, standing where the design stood in the
partition order or, for an ungrouped design, last. The catalog notice reports
every outcome; the authority's typed refusals, `group::design-limit` from a
stale view of a full group, `group::design-only` for a task without Design
intent and `group::partition-moved` for a partition that changed since the
browser read it, each leave every group unchanged.

Each opened group is a console in the team fleet, after the teams, read exactly
as a team console with tasks and agents swapped, its whole console rebuilt on
the palette's final accent through the
[base-flavor contract](CONSOLE.md#design-standing). A team's composers target
its lanes and its tabs are tasks; a group's composers target its designs and
its tabs are lanes. The group's designs, in their shared order, each take the
accent a lane in that position takes in a team: the design's composer, its
light in the status band, and every one of its room cards carry it. All
designs' publications interleave newest first in one packed plane, so no room
is paged or opened separately; durable task and publication stamps key the
cards, and linked bytes that are not present stay labeled source
placeholders.

As on the command bar's Design rail, the group's total closes its tab rail and
opens the group's work overview. Beside it, toward the start, the rail lists
each lane that holds an active claim on an open task
associated with any of the group's designs: the design itself, or a task
whose preparation parent, controller, or construction is that design. A lane
enters and leaves the rail exactly as a task enters and leaves a team's rail,
as board facts change, in the accent of the design its first such task serves,
with its task handles in its title. Activating a lane opens its composer in Chat, whose team
plane holds its history; a lane with no Chat seat stays a plain label.

A design's composer posts the guarded native room-post command to exactly that
design's room on **Post** or Control/Command+Enter. Quoting an operator-authored
card drafts a `spice://quote/v1/room/ROOM/PUBLICATION/DIGEST` shard above that
design's composer, the same shard a team composer drafts, and steering delivery
expands it from the room's journal record. Room posts carry no images. A post
is recorded in the room and asks no lane for anything, which the composer's cue
states. An admitted reply clears the composer only while it still holds
exactly the posted text and shards; a refusal keeps everything and shows in
the composer toast. The design's menu carries its
[standing](CONSOLE.md#design-standing), the
[coordination controls](CONSOLE.md#room-coordination) that ask a seated lane
for a contribution or put the room aside while the Design is active, its
preparation authority and, for an open OOPS batch, the grooming form below.

Operator posts and lane MSG, ASK, and ANS publications share one card family:
sender → destinations, verb, and text. A lane's own bytes come from its
transcript echo; absent that, the text its native delivery carried stands in.
Each card separates being recorded in the room, each route's receipt
(pending, queued for a vacant lane and unheard, vacant, unavailable, acked,
declined, expired, or cancelled), and a question's substantive result
(pending against its deadline and the requester's pending-result capacity,
answered, declined, cancelled, or timed out). ACK settles only a receipt; an
answer is its own ANS card whose return carries a separate receipt. None of
these is styled as review evidence or output authorization, which stay on
their exact proposal version and in immutable preparation authority.

A Design's read view groups its [typed contributions](CONSOLE.md#design-contributions)
by kind above the room history, which keeps them in journal order among the
conversation. Each cited source's availability comes from the
`room::contribution-sources` read, which any authenticated session may issue
and the app issues once per cited contribution of a shown Design, at most
eight in flight, and again after a reconnect; it resolves through the same
resident path as `spice room history`.
A lane session submits through the guarded `room::contribute` command from the
Design view's form, and its sidebar lists the lane's Design invitations from
the room snapshot's `room::noticed` records.

The same read view lists every immutable proposal version. An operator uses
native radios, output checkboxes, and a reason field to preview then submit one
exact Select, Return, or Collapse decision. Selection and authorization are
one command; the UI has no review-coverage prerequisite or later authorization
stage. While a command is pending the version cannot submit again, and only
committed room and board authority changes its standing.

The operator's inbox sits in the Spice sidebar directly beneath their avatar,
name, and email and above Jobs, not in the shared Working groups catalog. Its
heading reads `<initials>'s inbox` (for example, "CA's inbox") once Git
identity supplies initials, and "Your inbox" until then; an identity refresh
retitles the same surface in place. It catalogs every open operator item each
lane addressed through `#KEY` of a direction the operator sent it; the card
names that direction by key and the text the operator sent. Retained
pre-cutover items name the task they concerned, or show as unnamed when they
named none; current packets cannot create either.
Then the card shows its publication's full body. A lane's body stays in its
transcript, so the app reads it once, with the concerned direction's text,
through the operator-only `room::conversation-text` read, which projects the
same text `spice://room/inbox` prints; the transcript echo stands in while
that read runs. An ASK item leads with the answer it owes: **Answer** sends the
reply back to the requesting lane as an ANS and settles the receipt, and
**Decline** refuses the question. Its receipt row is separate: ACK only marks
the item seen and NAK declines it. An MSG item offers only that receipt row.
**Recover** returns a lapsed receipt to pending. Closing a lane party's
question is an explicit override. The catalog keeps an operator-targeted ASK in
immutable admission order while either its receipt or its answer duty remains
open, so ACK cannot hide the still-pending result. Receipt and result rows keep
one geometry in every state. Acting moves keyboard focus to the acting card,
and its outcome or refusal is reported in the composer toast for a console card
and beside the inbox for an inbox item.

**Close group** and the group order controls change only this browser.
Opened groups, their order, each design composer's draft, and the last focused
design composer use `spice:room-views:v2` local storage, and every change
applies to the current local state whenever it lands. An explicit open
focuses the named design's composer; otherwise the saved composer focus is
restored only while nothing else holds focus. Incoming cards, receipts,
conclusions, lane changes, and late hydration never mount, close, reorder, or
focus a console.

An open, pending Design also exposes bounded OOPS grooming. Each record
names the exact OOPS reports it considered, one of `duplicate`, `no-work`,
`deferred-for-later`, `covered`, or `proposed-work`, its evidenced cause, and
any exceptions or uncertainty. Recording changes no report workflow;
`deferred-for-later` has no wake time. A proposed report is filed through
`spice task file REPORT --preparation DESIGN`, which atomically changes its
filing state and makes it a controlled, unauthorized output. Dispositions are
evidence, not authority: filing neither requires nor follows from a
`proposed-work` record. Filing with `--claim` is refused when a preparation
controller is named, and its card continues to show authorization pending until
an accepted candidate receives separate selected authorization. A concluded
Design accepts no later grooming; start a new Design room for a later batch.

## Drafts and OOPS

A task's filing state is one of three standings. **Filed** work is ordinary
board work. An **OOPS** report asserts unexpected behavior, stays parked for
grooming, and is filed through `spice task file` or its task detail's **File**.
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
operator can read, each opening its task detail. That detail carries **File**
with a **Project** field holding the report's current project; naming another
public project files it there instead. Filing witnesses the statement,
workflow, and deferral heads the control was drawn from, so a report that
changed meanwhile refuses. A refusal, including a refused project, is shown
beside the control and leaves the report parked. Success replaces the report
with its Filed task under that project's handle. Filing under a Design's
preparation, or filing and claiming at once, stays with `spice task file`.

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

## Composer images

The chooser keeps admitted image bytes when the complete draft fits one durable
steering record. A larger decodable camera or gallery image is flattened onto
white, resized, and encoded as JPEG within the record budget the draft's other
attachments leave. The shared attachment limits remain the submission
authority: an attachment that does not fit is refused in the composer, never
after Send.

The image annotation editor closes immediately on Cancel, Escape, or backdrop
while its marks equal the set it opened with; once they differ, those exits
confirm before discarding, and undoing back to the opening set removes the
confirmation. Clicking empty canvas only clears selection. Apply commits
without a prompt, and dirty annotation work joins the page-unload warning.

## Message cards

Every attributable prose run between nested quote blocks is one speaker-owned
hunk. It opens with the source's Quote/Play controls, relative age, name, and
one classification pill: ACK or NAK first, FINAL next, then BROADCAST. Play
speaks only that hunk's rendered text; Quote targets that hunk's exact
authenticated source, operator-authored included. Direct text after a nested
quote starts another hunk with its source's controls and attribution; the
rule applies recursively to agent prose and resolved operator, agent, and peer
quotes.

Acknowledgment is an authenticated message disposition, not source text. Every
direct hunk of the answering speaker wears the same ACK or NAK segment, quoted
hunks never inherit it, and pending settlement revises those segments in place.
TASTE and REVIEW mark the quoted directive the resident sent, not the answer.
Literal `[ACK]`, `[NAK]`, `[TASTE]`, or `[REVIEW]` without an authenticated
owner stays ordinary Markdown.

A quoted peer MSG keeps its original sender, arrow, and exact destination from
the creating journal fact; a fanout reply quotes only its own delivered
destination. An authored MSG, ASK, or ANS appears as an outgoing quote and lists
every addressed occupant in header order. ASK and ANS carry typed badges; ANS
also names the exact result it answers. Their protocol headers remain hidden.
A lane's name and accent belong to the lane, so an occupant that has since left,
whether its lane is vacant or reseated, still reads under them as a historical
attribution. A lane absent from the roster reads as unavailable, or as the
recipient's address, never as raw identifiers.

A directive that an MSG, ASK, or ANS delivered, quoted as the context an
acknowledgment answers, shows the packet body without its header. ASK and ANS
wear their authored-send pills after the source, ANS with the result it
answers; MSG wears none, as an authored MSG does. The opening room publication
proves the kind; any other directive stays literal, whatever its words spell.
An operator packet names the occupant whose acknowledgment the card presents
as its destination. A quote link to such a directive presents the same way:
its card shows the packet body, the same pills, and the lane the directive was
delivered to after the arrow. A quote's head keeps the card head's type scale
and name color.

Each quote layer ends with one task target after its speaker's last word,
filled with origin-task badges; it repeats on no other hunk, and Play never
reads it. An isolated task-handle chip's hover title is its surrounding
activity prose, or `Open task HANDLE` alone. Directly trailing sentence
punctuation stays on the chip's line without preventing a break before it. A
quote's rail shows its speaker: an agent's or peer lane's quote wears that
lane's accent; operator, Spice, taste, broadcast, review, and unattributed
quotes take the neutral accent; an
unavailable quote keeps its warning rail.

A card's linked relative age is its permalink. Arriving through it outlines
the card briefly, then fades the outline (cleared at once without motion), and
focuses the card without a visible focus ring.
