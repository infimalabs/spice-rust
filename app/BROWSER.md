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

`spice dev xtask -- app-demo` supplies `appUrl`. It stages the worktree's
current app sources itself before it serves anything, so an app source edit is
visible without a separate `spice dev xtask -- app`; its
`[app-demo] ... sources=... staged_generation=...` line names the source
identity the served generation was compiled from.

An intentional theme-matrix check passes `contextOptions: { colorScheme }`; the
returned `colorScheme` and `colorSchemeSource` report the value and whether it
came from `"system"` or `"explicit"`. A stored `spice:app-theme` preference
still wins over the browser preference.

## Theme selector material

The Spice sidebar keeps Light, System, and Dark as three pressed buttons in one
theme group. Their selected face slides behind the buttons and uses the same
transmitted, lit glass recipe as an active team or working-group slider thumb.
The buttons remain the semantic and keyboard controls; the moving face is
presentation only. Its gradient, edge, and light catches remain legible when
backdrop filtering is unavailable. Reduced-motion presentation moves the face
without travel.

The [glass control studies](design/glass-controls.html) are an interactive
reference gallery for the [team composer
controls](CONSOLE.md#team-composer-controls). They send no commands, and
editing them changes no production style.

## Task graph workflow cues

The task overview overlays each Mermaid task node with the authoritative
availability label. `WAITING` is healthy, inactionable workflow progress: its
muted cool tone and double inline rails appear on both the node frame and its
state tag. `DONE` remains quiet settled provenance with a single neutral frame
and rounded tag. The rail and tag shapes keep the states distinct without
color, including reduced-contrast and forced-color presentation; waiting never
borrows warning, danger, or operator-attention emphasis.

## Browser branding

The tab title and menu use the lowercase brand `spice`, trailing any changing
state: `5 tasks - spice`, `1 task - spice`, `7 tasks (stale) - spice`,
`Loading - spice`, `Disconnected - spice`, and `spice` before the app loads.
Once a board snapshot arrives, the title and the pepper icon's badge carry the
exact remaining public task count; the badge is exact through 99, then `99+`,
and zero is explicit. Reconnect keeps the last count marked stale until current
data returns. Speech never retitles the tab.

The global Spice summary keeps the current connection state and admitted
journal stream cursor on one line. The cursor appears only while the resident
connection is current; startup and reconnect states never present a retained
runtime's cursor as live.

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

Working groups carry shared catalog organization and a shared activity stance.
Opened group consoles, their order and layout, composer drafts, reader and
scroll positions, speech, and focus remain browser preferences. Those
preferences cannot grant task, room, group, team, or lane authority, and a
group edit never mounts, reorders, or closes a browser view.
Hiding a team is likewise browser-local: its membership, claims, drafts,
lanes, and running work remain intact, and activating any member's lane link
reveals the team before focusing that lane's Chat composer.

The Spice sidebar's working-group catalog is the design-side twin of the team
roster and is drawn by the same roster (see [Resident topology in the global
menu](OVERVIEW.md#resident-topology-in-the-global-menu)).
It reads one shared partition: every group in partition order, each holding its
designs in order and one Passive, Explore, or Converge stance, guarded by one
revision. A group exists only while designs belong to it: it is never created
empty, never named, and never removed as an object. Its card is labelled from
its lead design, member zero, as a team is from its host. The whole card mounts
the group's console by click, Enter, or Space; its nested Design controls keep
their own actions. The formed card's frame binds to the same stable base-flavor
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
revision the browser last folded; the console's stance slider sends
`group::stance` under that same revision. A change that partition revision no
longer matches is refused rather than merged. A group whose last design leaves
or which is disbanded ends. Ending a group moves no task: its designs and the
work they produced stay on the board. While room authority loads or
reconnects, an open console keeps its last label and geometry and closes
nothing.

**Designs** lists the open Design tasks no group holds, as Unassigned Agents
lists lanes in no team; each tile there reads open, passive, while a grouped
tile reads the standing its group's stance grants. Passive and no group are
passive; Explore and Converge are active. Dropping a design on **New working
group** begins a Passive group containing only that design, standing where the
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
words lead with the relative age, the speaker in their lane's team accent, an
arrow to any receivers, and one badge: ASK or ANS for a question or answer, the
contribution kind (reason for, reason against, evidence, question), or Unheard
on an operator post that no current or later lane authority proves was heard.
The Unheard pill's tooltip and accessible label explain that no lane listens
yet and that starting or directing one will have the post read.
Its receipts, result, or cited sources follow the words. A lane's first
event-backed transition into listening, and its first such transition after a
leave, is a full-width `joined DESIGN` divider immediately before the exact
contribution or directed packet that established it. The join carries that
event's durable identity and lane attribution; later publications while the
lane remains present and replay after reconnect create none. Current native
work and duties remain standing rather than fabricated timeline events. A
lane's departure from a design's room is likewise one full-width divider, the
group's counterpart of a team compaction card and in its compact shape: two
rules in the design's accent around the leave's linked age, the leaving lane,
the design it left, and its clause, which alone yields width; the label's title
keeps the whole sentence. As on a team toolbar, the status band shows the
newest thing said in any of the group's designs, a contribution named by its
lane and kind, and no group name; a listener transition or coordination never
displaces it.

As on a team's rail, the group's total holds the right edge of its tab rail and
opens the group's work overview. Leftward from it, the rail lists each lane
that holds an active claim on an open task associated with any of the group's
designs: the Design itself, or a task with a direct dependency on that Design.
Removing one Design edge releases the task only from that Design; another
Design edge keeps the task on its other group rail. A lane enters and leaves
the rail exactly as
a task enters and leaves a team's rail as board facts change, appears once in
its canonical team accent, and replaces
the associated task handle. Activating an admitted lane opens its composer in
Chat, whose team plane holds its history; a lane with no Chat seat stays a
plain label.

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

Operator posts and lane MSG, ASK, and ANS publications share one card family:
sender → destinations, verb, and text. A lane's own bytes come from its
transcript echo; absent that, the text its native delivery carried stands in.
An operator-direction route names the principal that direction reaches; while
that recipient read is unavailable it names the route's `#KEY` concern, never
the retired context-free `operator` spelling. Each card separates being
recorded in the room, each route's receipt (pending, queued for a vacant lane
and unheard, vacant, unavailable, acked, declined, expired, or cancelled), and
a question's substantive result (pending against its deadline and the
requester's pending-result capacity, answered, declined, cancelled, or timed
out). ACK settles only a receipt; an answer is its own ANS card whose return
carries a separate receipt. None of these is styled as review evidence or
output authorization, which stay on their exact proposal version and in
immutable preparation authority.

A Design's read view groups its [typed contributions](DESIGNS.md#design-contributions)
by kind above the room history, which keeps them in journal order among the
conversation. Each cited source's availability comes from the
`room::contribution-sources` read, which any authenticated session may issue
and the app issues once per cited contribution of a shown Design, at most
eight in flight, and again after a reconnect; it resolves through the same
resident path as `spice room history`. This read-only surface never creates or
answers the session-profile Design requests delivered through a lane's own
session.

The read view derives its **Design maturity** panel from the bounded per-Design
measure the room projection folds incrementally from streamed requests and
contributions, never by folding rendered room history. It shows contribution
kinds and distinct lanes, requested reach, recent statement-head request
yield, folded older-head and unattributed historical totals, and every missing
substantive dimension. Fresh has no contributions; steeped requires
reason-for, reason-against, evidence, and three contributing lanes. Questions
and broad requested reach cannot satisfy it. Steeped maturity makes the
Design's first pass ready for a lane, which the standing's pass cycle shows;
it is not a danger or workflow state, and maturity never blocks or authorizes
a Design decision.

The same read view lists every immutable proposal version. An operator uses
native radios and text fields to accept or reject every output, optionally
comment on each verdict and the version overall, then preview and submit one
exact decision; Collapse remains the reason-bearing alternative. Output cards
identify declared Plans and Designs by intent, and undeclared work as a task
with its authored flow. Every work output starts visibly **Rejected**, the
conservative default that grants no authorization until the operator selects
**Accept**. The decision and accepted-output authorization are one command,
while every rejected output stays gated. A decided version is read-only. While
a command is pending, the version cannot submit again, and only committed room
and board authority
changes its standing. A version its author withdrew leaves the open list and
the open count for a separate **Withdrawn versions** history that names its
author, age, reason, and withdrawal fact and offers no decision control, so the
remaining open version is the one to decide; the view never picks among open
versions by recency.

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
Then the card shows its publication's full body. A lane's body stays in its
transcript, so the app reads it once, with the concerned direction's text,
through the operator-only `room::conversation-text` read, which projects the
same text `spice://room/inbox` prints; the transcript echo stands in while
that read runs. An ASK item leads with the answer it owes: **Answer** sends the
reply back to the requesting lane as an ANS and settles the receipt, and
**Decline** refuses the question. Its receipt row is separate: ACK only marks
the item seen and NAK declines it. An MSG or ANS item offers **Reply**, ACK,
and NAK: Reply sends waking operator direction to its author lane and settles
the item, while ACK and NAK settle silently. **Recover** returns a lapsed
receipt to pending. Closing a lane party's
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

The repository-owned OOPS Design uses the ordinary Design room and proposal
surface. One typed repository generation authority, not the Design's title or
project spelling, selects the current open Design. Its generated candidate
shows each pending report Plan as one line item with its complete statement
and an Accept or Reject verdict. Every line starts visibly **Reject**, so an
untouched submission explicitly rejects it; Return is not offered because
every submitted line receives a disposition. Accepted lines receive ordinary
execution authorization. Rejected and previously decided lines remain
history, while reports arriving during a decision appear in the next candidate.

## Composer images

The chooser keeps admitted image bytes when the complete draft fits one durable
steering record. A larger decodable camera or gallery image is flattened onto
white, resized, and encoded as JPEG within the record budget the draft's other
attachments leave. The shared attachment limits remain the submission
authority: an attachment that does not fit is refused in the composer, never
after Send.

Retained steering attachments and transcript images carry an image-specific
Quote action beside full-size opening. It wears the same quote glyph,
dimensions, edges, colours, and interaction treatment as an ordinary message's
Quote action. The action drafts exactly that image under its admitted name. Its
target names the durable steering or tape source, source ordinal, and
decoded-byte digest; paths, data URLs, tool intent, and browser-generated
diagrams never become quote identity. A sent image quote resolves from the same
durable source and renders an explicit unavailable state if that source no
longer authenticates the named image.

In the full-size image viewer, A activates **Annotate** when the image is a
draft image the viewer can annotate, and D activates **Download**: each key
does exactly what its button does, including a refusal, and nothing while the
button is absent or disabled. Held-key repeats, modified keys, and keys typed
into a field never trigger them, and inside the annotation editor A keeps its
arrow-tool meaning.

While an image pasted into a composer is open full size, Right Arrow opens the
next image pasted into that same composer and Left Arrow the previous one, in
the order the composer renders them across its main draft strip and each quote
shard's strip. Navigation never leaves that composer, either end holds without
wrapping, and annotating, IME composition, and Alt, Control, or Command keep
the arrows from stepping. Each image opened this way annotates its own draft
attachment.

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
destination. An authored MSG, ASK, or ANS appears as an outgoing quote led by
its card's relative age and lists every addressed occupant in header order.
ASK and ANS carry typed badges; ANS also names the exact result it answers.
Their protocol headers remain hidden.
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

Each authenticated operator direction a lane's transcript recorded is also a
top-level card in that lane's team plane. Its canonical source row is the
earliest recorded delivery, while its placement and relative age use the
direction's operator-authored instant. The card rests in the operator's neutral
accent. Its body is the direction as one quote reading operator → receiving
members, with that quote's Quote and Play controls and its steering quote
target. One send fanned out to several members of a team is one card naming
each of them, so a resend, a nudge, a reload, or an older page adds no second
card. The direction's relative age
links to that card as any card's age does, and a quote of the same direction
inside another card links there too; with no direction card in the team plane,
that age stays unlinked. A peer delivery or a post-compaction replay is no
direction card, and the answering ACK or NAK card still quotes the direction
it answers.

Each quote layer ends with one task target after its speaker's last word,
filled with origin-task badges; it repeats on no other hunk, and Play never
reads it. The badges remain in ordinary inline flow, with no floated placement
or reserved minimum height, so the prose uses its full line and wraps the badge
only when needed. An isolated task-handle chip's hover title is its surrounding
activity prose, or `Open task HANDLE` alone. Directly trailing sentence
punctuation stays on the chip's line without preventing a break before it.
A quote's rail shows its speaker: an agent's or peer lane's quote wears that
lane's accent; a direction card's quote wears its first receiving member's;
other operator, Spice, taste, broadcast, review, and unattributed
quotes take the neutral accent; an
unavailable quote keeps its warning rail.

A card's linked relative age is its permalink. Arriving through it outlines
the card briefly, then fades the outline (cleared at once without motion), and
focuses the card without a visible focus ring.

## Team and group activity traces

Each team or working-group pip is an etched well: a darkened pane with a soft
lip under its top edge and a light catch along its bottom, its status on a
quote-rail-wide physical-left rail rounded by its own width. The pip keeps its
accessible status label and the trace inside is decorative. The trace spans the
ten minutes ending now. Each kind of work with recent activity, reasoning,
tool, and prose, draws one smoothed curve over a fill fading toward the floor,
ending in a dot at now; in the dark theme the curves glow. A curve's height is
that kind's recent rate on one fixed scale shared by every pip, each activity
weighted by its text, through a kernel that slides with time so redraws move
the curve smoothly. Turn, operator direction, provider failure, compaction,
fresh session, and sign-out draw as markers over the curves. Where the lane
was off, from an occupant's end or a vacant seat to the next occupant's start,
every curve leaves a gap with no line or fill, and a lane off now ends its
curves where it went off; an interruption shorter than the spacing between two
of the pane's curve samples is too brief to see and leaves the curves whole.
Quiet is not off: a running lane with no activity still draws its low line.
The trace draws only state the browser already holds, issues no requests, and
redraws on new activity and every few seconds, so old activity leaves without
a new frame.

The app's `style-src 'self'` policy refuses inline style attributes, so the
trace is SVG geometry in the pane's measured CSS pixels and every colour and
glow comes from the stylesheet. A Playwright check of the trace must keep that
policy enforced: a context with `bypassCSP` would hide a trace that draws
nothing in production.

Yew's `html!` lowercases literal element names, and SVG recognises
camel-case elements such as `clipPath` only by their exact case, so the trace
writes those as dynamic tags (`<@{"clipPath"}>`).

A working group's pips are the same traced wells, one per Design, with the
Design's standing on the rail. A Design's trace reads its room's events as a
lane's activity: an agent's words are prose, a lane's contribution is work, a
human's words are direction, a coordination opens a turn, and a lane's
departure divides the room as a compaction divides a lane.
