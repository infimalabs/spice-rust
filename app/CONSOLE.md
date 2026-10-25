# Always-chat team console

Every team surface, populated or empty, keeps its Chat composer and message
plane mounted; there is no Work/Chat mode or mode-owned scroll state. Chat is
the base durable location, `#/chat`, extended by `/card/{target}` for a retained
card. A durable `#/work...` location opens the current scope's task overview
over that mounted Chat surface; closing it returns to `#/chat`. Project,
individual-task, relationship, and caller-composed selections stay
browser-local and release any Work location before opening. A Design
permalink, `#/chat/design/{stamp}`, is an arrival: it opens
[that Design's read view](DESIGNS.md#reading-a-design) and replaces itself
with `#/chat`. Every unrecognized location normalizes to Chat with the
unavailable-location notice.

Consoles of every width, height, and membership keep a usable header,
composer, and stream without a nested vertical scroller. Side-by-side
consoles share one faint dividing edge in the left console's soft edge tone,
painted on that console's end edge from the foot of the task band to the
consoles' bottom, so the earlier console's controls read as in front of it.

Console times never read in UTC unless UTC is the viewer's zone. Live text is
relative, advancing on its own deadline, with full local time on hover;
static renderings such as a task overview's Mermaid labels use compact local
time, so their source and image stay fixed. Canonical UTC
instants appear only in machine-readable attributes and payloads.

Journal positions, such as facts, statement revisions, claim heads and the
journal cursor, read as their condensed base52 sequence codes, never as decimal
integers. The Preparation authority menu follows the same rule inside its
displayed records; wire and journal payloads retain their machine encoding.
Authored numbers, counts, and transcript offsets are not journal positions.

## Working groups and teams

A working group is a team's design-side twin: one to six ordered Designs in the
repository's one
[guarded partition](../docs/product/invariants/identity.md#identity-working-groups-are-one-guarded-partition-of-designs).
It owns no lanes, roster, claims, wake behavior, read permission, task
lifecycle, open view, draft, focus, console order, or geometry. Saved edits
reach other clients but never mount, reorder, or close a browser's views. **New
Team** creates lanes; **New working group** is repository organization.

In the Spice menu, Jobs follows the operator identity before the Working groups
catalog and its New working group and ungrouped Designs targets; the catalog's
heading row matches Jobs, Resident topology, and Provider quota at every width.
A design tile is an agent tile's twin: its handle in its accent, its standing
ending line one, its title beneath, and a click opening its read view. It wears
its console channel's accent, or the Design base while ungrouped. A
working-group card is its Open control: activating it mounts that group's
console.

A group console is a team console with tasks and agents swapped: one composer
per design, posting to that design's room, under the
[group toolbar](#group-toolbar). Everything it shows derives per read from the
board, room projection, and lane topology; the group stores and grants none of
it. A group's console opens when a completed room snapshot first holds a group
this browser has not seen; the first snapshot only records its groups, and a
closed group stays closed. A design composer's cue says posts are room records
that never wake, obligate, or direct a lane; asking a lane goes through the
design menu's coordination controls.

The group control panel keeps the team panel's frame and maps each control:

- **SEND** posts every design composer holding text or a drafted quote, each to
  its own room. Its rim shows those designs' accents; it reads `FULL SEND`
  exactly when every open design of a multi-design group is drafted, and is
  disabled while no room takes a post. A settled design takes no post. Command
  or Control and Enter posts one composer alone. Idle follows the team rule.
- **The group menu** replaces the arrangement menu in place: it moves this
  browser's console left or right, closes it, or **Disband working group**,
  whose `group::disband` result shows in the catalog notice. There is no swap
  or join.
- **Group TURBO** shows and sets the group's durable fast-mode flag through
  `group::fast` at the revision read. While on, every lane claiming work
  associated with the group's Designs runs fast. Fast mode stays additive
  across lane, team, global, and group flags, and changes land as team TURBO's
  do. Its title names the lanes it runs fast.
- **The speech slider** is browser-local per group: each covered lane's prose
  is voiced at the louder of its team's and every covering group's mode, each
  line at most once. Room publications are never voiced.
- **The allocation slider** is the group's shared stance, Passive / Explore /
  Converge, sent as `group::stance` at the revision read. Passive makes every
  open Design passive; Explore asks lanes for input on quiet or unopposed
  questions; Converge asks lanes already charging a contested question to
  re-charge it. Refusal returns the slider to partition authority with its
  reason.

Each design heading resolves, as a member heading does, from the room's newest
authored publication this browser holds, a lane's contribution included;
departures and coordination never displace it. Until one is held it shows the
Design's title, or an unboarded design's handle with that availability. A
landed post clears its composer only while it still holds what was posted; a
refused post leaves that draft in place, and an awaited post is not sent again.

## Design standing

Standing and activity rules live with the
[Design-room contract](DESIGNS.md#design-standing). Design tabs share the
execution tabs' structure, with their count before the stem and their surface
bound to the palette's final accent. The base-flavor contract rebuilds panel,
control, ink, muted, line, ground, focus, and primitive roles from that base
and the page's achromatic anchors; Lane and Design accent slots stay
independent.

Team consoles rank by durable formation instant, oldest first, taking bases up
from the palette's first accent. A team forms at the commit that makes its
identity teamed, **Create new team** included, and keeps that instant until
unteamed. Working-group consoles rank by their identity's minting instant and
take bases down from the last. Arrangement and browser order change neither
rank; member and Design accents step from the base in durable member order. The
task overview sheet wears the base of what it shows: a team or group total opens
in that console's base, Design-only scope in the Design rail's plum, anything
else in the page's base.

## Toolbar

The toolbar has two rows: the task strip, then a status band led by the live
ticker and closed by presence pips directly above the composer's team menu, no
wider than that round control, so the ticker runs on over TURBO. The ticker's
age aligns with the composer ages below at every width. The status band is the
console's top edge, ruled in the soft edge tone. Where two consoles touch, their
bands meet flush and square like the tab strips above, and only the left
console draws the half-width hairline divider, in the open tab row's page
colour, so the head reads as continuing down the seam. A corner at the browser
edge stays square.

Pips show how many agents there are and whether each is working, stalled, or
off: one per member in canonical order, coloured by active, queued, waiting,
bound, or unbound state, never identity. One to six stack in at most two rows
bottom-up from the right, a shorter top row nesting half a pitch in. The ticker
keeps age, status, prefix, accent, accessible label, and live region. A running
command's line names its tool, such as `job:`, then its latest nonblank output
before intent and context, so a narrow ticker cuts context first; hover shows
the bounded line. A new turn names what opened it from the prompt the lane
received: each Spice notice's heading or lead sentence with the facts it lists
(an assignment, a resumed seat's held claim and pending keys, a briefing, room
catch-up, a finished job), each direction key, and each peer note's sender;
the `spice://` guide names nothing, and a turn with no such cause shows the
bare `new turn` label. An empty fleet reserves no pips and invents no status.

## Task strips

The command bar hangs two global totals, execution then Design, from its top
edge before the Spice button, one task-tab gap apart, the Design total off the
button by the button's gap from the window edge. Each team and open
working group has one local strip. Totals and strips use the horned-tab
construction; every total is badge-only with one fixed footprint. The global
totals open all open execution and Design work; the command bar breaks out no
project, which its Work deep link reaches. The Design total opens each Design
past plan or review intake and each open task it directly controls; a Design in
intake is execution work. Accountable ownership, never dependency reachability
or group placement, chooses the global total, so a Design-produced Plan counts
right and the tasks it creates for execution count left. Local docking never
removes a task from its global total.

Local placement follows
[claim-bound placement](../docs/product/invariants/identity.md#work-claim-bound-work-has-two-visible-placements):
a team strip tabs each task its lanes actively claim, Design work included, in
that lane's accent; a group strip tabs each lane claiming work associated with
its Designs. Local tabs run right to left in the composers' order, each over
its own column: a team's by lane, each lane's tabs together, and a group's by
the first Design each lane's work serves. A local total selects exactly what
its tabs stand for, with no project roll-ups. A team or group total spans the
pip column flush to the console's right edge, centring its badge over the pips
and the team menu. Tabs past a total scroll under edge fades. Local tabs meet
each other and the total without gap, border, or interior rounding; only the
strip's outside ends keep the thick rail and corner.

An accented tab's text is accent-tinted over a 5% accent ground, 10% on hover,
under a top rail of the accent mixed into the line color, stronger on hover.
Unclaimed handles and totals keep the neutral rail, undimming one step on
hover. Every count badge is one neutral
[open-task total](../docs/product/loops.md#the-jitter-contract). Gate waits
read the resident's
[one classification](../docs/product/invariants/identity.md#work-gate-waits-have-one-resident-owned-classification);
only a Blocked row is danger-coloured in detail and graphs. A global, team,
or group total whose partition holds work only the
operator can move now (human-actionable blocked or environment-held work, or
for a Design tab an unclaimed, undeferred Design awaiting decision) carries a
small out-of-flow danger dot on its badge's top inline-end corner and leads its
title and accessible name with that count; the overview's attention rail then
reads Needs attention. Local task tabs carry no mark. Closing any overview
returns focus to its opener.

Project controls select the exact project; a relationship control extends the
selection with that task, in the resident's offer order, possibly empty.

## Group toolbar

A working group's toolbar keeps team geometry, total first and lane tabs after:

| Team toolbar | Group toolbar |
| --- | --- |
| Total: tasks its members actively work | Total: all actively claimed work associated with the group's Designs |
| One tab per actively claimed task, in its lane's accent | One tab per lane claiming associated work, in that lane's accent, opening its Chat composer when seated |
| One presence pip per member | One pip per design, in its work-map tone, by [standing](#design-standing) |
| Ticker: newest member activity | Ticker: newest words across the group's rooms; a contribution names its lane and vote |

A design pip reads concluded done, concluded collapsed, or deleted once
terminal, even while grouped, and open passive outside every group; otherwise
claimed while a lane claims it for a pass, else the resident's pass cycle
state. It wears its Design's task-overview graph-card tone, so it agrees with
the tab marks: in flight and ready for proposal live, awaiting decision
warning, blocked danger. A passive Design's pip is a ring in that
tone; a terminal or unavailable one fades. An unavailable row or cycle invents
no standing, and each pip's accessible label names it. The pips close the
status row above the group menu, and each design's tile in the Working groups
catalog leads with the same pip at member size, so readiness reads without
opening the group.

## Team composer controls

Beside the drafts, one fixed-geometry panel holds team TURBO, the arrangement
menu, the speech and allocation sliders, and SEND. The arrangement menu opens
swap and join beside the panel and is disabled, naming why, when no other team
exists. Team and global TURBO are separately admitted; every TURBO reads the
one word TURBO. Off, it rests as flush glass with muted lettering; on, it is
backlit in the sliders' high-stop amber, pooling at its foot and leaking a dull
halo past the edge, with ink lettering.
The word is decorative; pressed state and label are the accessible truth.

The team menu can hide its team surface without changing membership, lanes,
claims, drafts, or work; any of its agent lane links shows it again and focuses
the Chat composer.

The speech slider reads Quiet / Speak / Narrate and the allocation slider Steer
/ Drive / Drain; Up/Right raise and Down/Left lower. A speech selection applies
to the team's browser-local policy in the input that paints it. Quiet voices
nothing automatically. Speak voices resident-classified final answers and
acknowledged steering; Narrate voices every live card. Either mode voices a
steering-key acknowledgment in full without protocol headers, and any other
card by its first and last meaningful paragraphs, where list items are detail
(an opening list speaks its first item; a closing list is silent). None
reads fenced code, quoted lines, or Markdown marks. A pending allocation
command admits no send and lights no recipients. Reduced motion removes slider
travel, never a state.

SEND is two optical layers: an emitter spreads an even diffuse field across the
disc to the glass edge, with no ring or hotspot, under a frosted face
collecting uneven perimeter catches, brighter low left and fainter high right,
as the sliders are lit. States change only field intensity, face pressure, and
its typed rims above the glass. An idle team, with no member starting or
running and nothing drafted, carries no light: the emitter goes dark, SEND rests
as flush glass like TURBO, and sliders keep their stop's accent
without glass catches or halo.

SEND's rim is lit in its actual recipients' accents, continuous for one and
arced for several, with the count in its title; it is paint, never
per-recipient targets. Recipients are reachable members whose drafts hold text,
quotes, or images a press would send, including a vacant member it would wake.
A running team with nothing drafted shows no rim; an idle team with no agents or
drafts disables SEND while the sliders stay usable. In a multi-member team SEND
reads `FULL SEND` exactly when the submission reaches every member; a broadcast
preview substitutes its actual recipients.

A vacant member takes the same durable steering submission as an occupied one,
its witness naming no occupant at the current generation; the resident launches
or resumes the lane after the directive commits, with no browser wake state or
second command. Later submissions wait for a causally newer lane snapshot
before reusing a witness; a refusal restores the draft.

Pointer, keyboard, and touch contact depress SEND without intercepting native
activation. Held submit modifiers add a command ring outside the recipient rim:
the focused agent's for a submit, the theme's ink for a broadcast.

## Speech

Under `spice app` the console speaks through the desktop shell's synthesizer,
because WebKit's `speechSynthesis` offers only built-in voices, not an
operator's downloaded premium voices. The shell injects
`window.spiceShellSpeech` at document start: `getVoices()` lists each voice's
identifier, name, language, quality, and system-default flag; `speak(text,
identifier, rate, handlers)` replaces the current utterance and calls
`onstart`, `onend`, or `onerror` with `{error}`; `cancel()` silences it, after
which its handlers never fire. Requests travel over shell IPC and answers
arrive as evaluated script, so the surface needs no token or content-security
exception. A new document silences what the previous one left speaking. The
speech controller uses the surface, when present, with the same queue,
ownership, voice spread, and stop-all lifecycle; a browser keeps
`speechSynthesis`.

## Composer and stream

The composer rides in the Chat plane's scroll content, and its measured height
is the plane's spacer and scroll padding. Message planes never paint a
scrollbar; wheel and trackpad scrolling stay native. Exposure follows [Chat
scroll passively controls composer
exposure](../docs/product/invariants/lifecycle.md#surface-chat-scroll-passively-controls-composer-exposure).
Programmatic focus moves the caret without scrolling the viewport, fleet,
member strip, or plane; the composer's own focus transition still reveals a
collapsed draft. The fleet opens at its canonical right edge and holds it
through resizes only while the operator leaves it there.

A member strip paints canonical member zero at its right edge, beside the
controls. Tab order instead reads as a person does: one team's member
composers left to right, each heading and its fields in order, then the team's
controls, then the next team. Elsewhere, and inside an open modal dialog, the
browser's order holds.

Each member heading resolves from that lane's newest eligible authored prose.
When a history page has none, header demand continues to a newly exposed older
frontier but never reissues the same frontier without fresh viewport demand.

Message and quote signoffs baseline-align the monospace age with the speaker's
name; the age's glyph-height line box keeps the signoff one prose line high.

## Input history

Each lane keeps one history of text sent from its composers, quote replies
included, in local storage under the lane's logical identity, so it survives
reloads and occupant changes. A send records each nonblank text it carried,
quote replies first; a repeat of the newest entry adds nothing, and images are
never kept. The document keeps the newest 200 entries per lane within 256 KiB,
trimming the least recently used lanes' oldest entries first; a corrupt or
oversized document is replaced by what fits.

An unmodified ArrowUp with a collapsed caret on a composer's first rendered
line recalls the previous entry with the caret at its start; ArrowDown on the
last line moves forward with the caret at its end, finally restoring the unsent
draft. With a selection, a modifier, or a composing input method, the arrows
move the caret. Edits to a recalled entry stay with it
while walking; history changes only on send, which ends every walk and records
a sent edit as a new entry.

## Image runs

A lone tool image renders as a cover card. Two to four such cards in a row from
one speaker, each at most two seconds after the previous, crystallize into one
card at the run's first place; a fifth image, a longer gap, another speaker, or
another card starts a new run. The crystal keeps one card's footprint, one
cell per image in capture order, sized from each image's decoded aspect and the
card's box so none is badly cropped or shrunk to a sliver; cells re-divide as
images load or the box changes. Each cell opens its own image,
and hover and focus mark only that cell. With a lane image open full size,
Right Arrow opens the next newer image and Left Arrow the previous one; each
cell is one stop, navigation never crosses lanes, and the ends do not wrap.
Narration, quotes, receipts, and every other surface still see one message per
image.
