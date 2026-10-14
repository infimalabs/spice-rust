# Always-chat team console

Every team surface, populated or empty, keeps its Chat composer and message
plane mounted. There is no Work/Chat mode, local Work pane, or mode-owned
scroll state. Chat is the only durable location, `#/chat`, extended by
`/card/{target}` for a retained card; any other location normalizes to Chat
with the unavailable-location notice. Work, task, and direction controls are
browser-local overview actions that create no history entry and claim no
restorable URL. A Design permalink, `#/chat/design/{stamp}`, is an arrival
rather than a location: it opens [that Design's read view](#reading-a-design)
and replaces itself with `#/chat`.

Wide, narrow, short, empty, one-member, and multi-member consoles keep a usable
header, composer, and stream without a nested vertical scroller.

## Working groups and teams

A team is resident lane topology and policy. A working group is its design-side
twin: a repository-shared identity and ordered list of one to six Design-task
birth stamps, matching the six lanes a team admits. Like a team it exists only
while designs belong to it, carries no operator-given name (its label derives
from its lead design, as a team's does from its host), and is never created
empty, renamed, or removed as an object. A design belongs to at most one group.
It owns no lanes, roster, claims, wake behavior, read permission, task
lifecycle, open view, draft, focus, console order, or geometry.

Every working group lives in one repository partition: groups in order, each
holding its designs in order, under one revision. `spice group list` reads the
whole partition. `spice group move ANCHOR --to GROUP` moves one design into a
saved group, `--new` begins a group holding only that design under a fresh
identity, standing where the design stood, and neither flag takes the design
out of every group. `spice group arrange GROUP=ANCHOR,ANCHOR...` restates the
whole partition in a new group order and design order. Each write witnesses the
revision it read and refuses as `group::partition-moved` once another write
has moved it; a group whose last design moves out ends there. A move naming a
task without Design intent refuses as `group::design-only`, and one naming a
seventh design refuses as `group::design-limit`; an arrangement that drops,
repeats, or invents a design, or names a group without designs, refuses with
its `group::arrangement-*` reason. The journal folds no group past six
designs, and a predecessor journal holding one, a named group, a per-group
head, or a receipt of the retired whole-group put refuses to cross, naming the
last release that admitted it. Anchors resolve by
immutable task birth stamp and display the task's current handle. An anchor
whose task is deleted or no longer declares Design intent remains explicit as
unavailable. Saved edits are visible to other repository clients, but never
mount, reorder, or close a browser's foreground views. A move changes only
organization: it authorizes no Design output. **New Team** therefore remains
lane creation; **New working group** is repository organization.

In the Spice menu, Jobs follows the operator identity before the Working groups
catalog and its New working group and ungrouped Designs targets. The catalog is
an ordinary sheet section: its prominent primary title, secondary label,
inset, and spacing use the same heading row as Jobs, Resident topology, and
Provider quota at every sidebar width.

The browser reads an opened group as a team console with tasks and agents
swapped: one composer per design, posting to that design's room, and the
team toolbar read from the group's designs (see [Group toolbar](#group-toolbar)).
Everything the toolbar shows is derived from the board, room projection, and
lane topology on every read; the group stores and grants none of it, and
reading it changes nothing.

A design composer's cue states that posts are recorded in the room for its
listeners and ask no lane for anything: a post never wakes, obligates, or
directs a lane. Asking a lane goes through the design menu's coordination
controls.

Beside the design drafts, the group console keeps the team control panel's
frame and geometry, mapping each team control to its group analog or to
none:

- **SEND** becomes **POST**. One press posts every design composer holding
  text or a drafted quote, each to its own room. Its rim shows those designs in
  their accents, it becomes the `FULL POST` lockup exactly when every open design
  in a multi-design group is drafted, and it stays disabled while no room takes a
  post. A settled design takes no post and has no place on it. Command or
  Control and Enter posts one composer alone, as it sends one member's draft.
- **Idle** follows the team rule. With no composer holding text, no quote
  drafted, and no post in flight, the panel rests dark with POST as flush glass.
- **Team arrangement menu** becomes the group arrangement menu, in the same
  place and with the same menu, focus, and Escape behavior. It moves this
  browser's console left or right or closes it. Swap and join have no analog:
  membership moves one design at a time through its own menu or the Working
  groups roster, never by merging groups.
- **Team TURBO** has none: fast mode configures the lanes a team runs, and a
  group runs no lane.
- **The speech slider** has none: it voices a team's live agent prose, and
  room publications are records.
- **The allocation slider** has none: Steer, Drive, and Drain set how a team's
  lanes take allocator work, and a working group allocates and authorizes
  nothing.
- **Keyboard broadcast and waking** have none: a post reaches only its own
  room and wakes no lane.

Each design heading resolves, as a member heading does, from the room's newest
publication whose authored words this browser holds. It shows that line, aged
and linked to its card. Until one is held it shows the Design's title, or, for a
design the board does not carry, its handle with that availability.

A post that lands clears its composer only while the composer still holds what
was posted. A refused post leaves only that composer's draft in place. Each
report names its design, and a composer whose post awaits its answer is not
posted again.

## Reading a Design

Any Design, open, completed, or closed, opens for reading in its task
overview: the **Read** action on each design tile in the Spice menu's Working
groups catalog, a Design-intent task's detail, and a Design permalink all open
the same view. Below the task's state, its **Design room** section shows the
room's [standing](#design-standing), the
[conclusion control](#concluding-a-design) while the Design is open, the
[approval control](#approving-a-designs-outputs), its preparation authority
records, and its full room history newest first, resolved through the Design's
birth stamp under its current handle, with the permalink beside the heading.
Room history and retained candidates stay readable in every standing. The view
holds no composer, receipt, answer, or coordination control. Opening or reading
it moves no group, joins, acknowledges, authorizes, and wakes nothing, and
commits no journal fact; acting on a room stays with the group console and the
`spice room` commands.

## Design standing

Every read agrees on one standing for a Design, derived from its workflow and
the working-group partition alone. Board rows, `spice task show`, `spice group
list`, `spice room status`, and the browser all render it:

- **open, passive**: open and in no working group. No lane can claim it or be
  coordinated onto it until the operator moves it into a group.
- **open, active**: open and in a working group, so allocation, capture, and
  room coordination may reach it.
- **concluded done**: the operator concluded it done.
- **concluded collapsed**: the operator collapsed it, with the durable reason
  that collapse recorded.
- **deleted**.

A concluded or deleted Design keeps its standing while a saved group still
holds it: group membership decides activity only for an open Design. The
command bar's Design rail names how many open Designs each stem holds active
and passive, and dashes a stem whose open Designs are all passive. In the work
overview and on dependency cards a passive Design reads **Passive**, waiting on
the operator.

## Concluding a Design

Only the operator concludes a Design. While it is open and the resident offers
conclusion, the read view's **Conclusion** section previews either **done** or
**collapse** with a nonblank reason, stating what the conclusion does: done
keeps outputs already selected released and is refused while an output still
owes the operator's selection; collapse keeps every output it constructed
gated. A blank collapse reason previews nothing and names the refusal. **Submit
conclusion** sends the previewed conclusion witnessing the Design's statement,
workflow, and claim heads the preview was drawn from, the same contract as
`spice task conclude`. The section reports the resident's answer: a refusal,
such as a live claim or heads that moved since the preview, keeps the preview
beside it and changes no standing; the standing moves only when the board
shows the conclusion. Ordinary task and Plan controls are unchanged.

## Approving a Design's outputs

The read view's **Approval** section lists each candidate retained under the
Design with its standing (undecided, returned, accepted by a decision, or
authorized), every review coverage recorded over it, its governed inputs, and
each output's title, handle, status, and acceptance. While the Design is an
open board task, **Accept** and **Return** record the operator's next decision
on that candidate, citing the candidate's retaining fact as its basis. A
returned candidate authorizes nothing.

Authorization requires current coverage from the operator or an independent
lane; the candidate author's self-review certifies nothing alone. Until the
candidate is authorized, **Record my review** records the operator's own
coverage of that exact candidate, citing its retaining fact and covering its
outputs and their acceptance, the same record `spice preparation cover` writes.
Recording again is how stale coverage is renewed after candidate evidence is
answered.

Once a decision accepts the candidate, each of its outputs on the board gets an
unchecked box. **Authorize selected** records one execution authorization of
exactly the checked outputs under that decision, the candidate's governed
inputs, and every coverage recorded over it; it completes the Design, and only
those outputs become eligible. Unchecked outputs stay pending and ineligible.
While the view can already see a refusal, no certifying coverage or a pending
dependency of the Design, the control is disabled and the section states the
reason and the act that clears it. With nothing checked the control submits
nothing. Each command witnesses the Design, input, and output heads the view
was drawn from, and the resident alone judges admissibility: its answer, or a
refusal such as stale coverage, a moved board, an output outside the
candidate, or a controller that is not an open Design, appears under the
candidate and records nothing. This is the same contract as
`spice preparation decide`, `cover`, and `authorize`.

## Preparation authority

A room for a controlling task shows its retained candidate, critique, review
coverage, decision, and execution-authorization records above the timeline in
immutable journal order. Each card preserves the record's exact outputs,
governed inputs, source link, decision outcome, and selected outputs; review
coverage and execution authorization remain distinct. The same authority is
available through bounded `spice preparation show HANDLE` and
`spice://task/HANDLE/preparation` pages. Reading or opening these records joins,
acknowledges, and changes nothing.

## Room coordination

Start and Resume are explicit coordination, not browser-open events. The
operator names a seated occupant's current base-52 address plus the unresolved
criterion, bounded contribution, and stop condition. Spice records one ordinary
durable direction with the room marker; the recipient then acquires the exact
anchor or related preparation through ordinary native task possession. Filing
visibility keeps hidden controller anchors out of automatic allocation; the
room command adds no other eligibility or possession rule.

Put aside cancels only unanswered coordination for that room. If the anchor has
a native holder, it sends that holder a checkpoint/free direction and shows a
stop request until that exact claim is completed, freed, or lapsed; it never
force-frees work or releases child claims and unrelated direction. The answer
asks its initiating consumer to hide the room locally; the team console keeps
the design's composer, since the group partition decides what it shows. Room
history, drafts, groups, selected outputs, other browsers, and repository
authority remain unchanged.

The design menu's **Coordinate** form, beneath the room's standing, offers
these commands while the Design is open and active. A passive Design shows
instead that no lane can claim it or be coordinated onto it until it moves
into a working group, and a concluded or deleted one shows no form. A lane
address field suggests each occupied seat's address with its lane name;
Criterion, Contribution, Stop when, and an optional Auto-resume limit from 1
through 8 complete Start and Resume, and Put aside needs only the room. Each
button stays disabled while the resident does not offer its command. Before
submitting, the browser resolves the address against the board's residency
and addresses with the resident's own rule: an unknown, vacant, or simulated
seat refuses in place with the resident's reason and explanation, and nothing
is written. The submission
witnesses the anchor's statement, workflow, and claim heads and the target
seat's residency, so a moved anchor or seat refuses with the resident's
reason. The composer's report names the request key and target, any remaining
auto-resume budget, or after Put aside the stop request's key, target, and
claim, which stays held until its holder settles it.

The native command surface is `spice room status HANDLE`,
`spice room start|resume HANDLE ADDRESS --criterion TEXT --contribution TEXT
--stop TEXT [--auto-resume N]`, and `spice room put-aside HANDLE`. An
`--auto-resume` rule lets the resident repeat that Resume up to N times as
[new room input arrives](../docs/product/invariants/identity.md#identity-an-operator-rule-bounds-automated-room-resumes).

## Room standing

Each design's menu and read view open with the room's standing, derived from
the room snapshot, its live facts, the board, and lane topology on every render:

- **Next action**: the one next useful action that authority supports.
- **Latest coordination**: its request and that request's disposition. For a
  ruled record, its subject, scope, and stop text. For an automated record,
  the publication that triggered it. Then whether automation is off, armed
  with steps spent of its limit, or stopped by a spent budget, a put-aside, or
  a request retired without acknowledgment.
- **Lanes**: each involved lane with whether it is seated, vacant, or unknown,
  plus one chip per reason, each naming its source:
  - contribution history;
  - activity inside the day-long window;
  - an invitation by a lane-addressed room packet inside that window;
  - native work;
  - a live coordination request;
  - an owed receipt or ASK result;
  - a claim-bound stop request;
  - a leave with the lane's clause, in force until the lane contributes again.
  Aging removes only activity and invitation. A leave ends both, including
  invitations it postdates. History and native duties stay.
- **Relevance**: the resident's shadow relevance observations, each with its
  classification, progress standing, explanation, exceptions, uncertainty,
  source coverage, and operator verdict.

Agreement, silence, message count, elapsed time, ACK, and ANS never read as
progress or approval. The section only renders inside an explicitly opened menu
or read view: a status, relevance, or automation update changes its text in
place and never mounts a room, moves a console, or wakes a lane.

## Design contributions

A lane contributes to an open Design's problem space with one typed record:
a reason for, a reason against, evidence, or a question, with its text and
the transcript, journal, or artifact sources it cites. A contribution is
lane-authored evidence for the operator's judgment, never authority: it
moves no task, claim, working group, candidate, decision, or authorization,
and it wakes nobody. Only a lane session can contribute; the operator reads
contributions and decides through the ordinary Design controls.

The Design read view groups contributions by kind, each with its lane,
time, text, and every cited source, while room history keeps them in journal
order among the ordinary conversation. The resident resolves each cited
source for the shown Design and the card says whether it is available or
why not (missing, hidden, changed, unreadable, or retained in an unsupported
shape), the same states `spice room history` prints.

A lane session's open Design view carries the contribution form: choose a
kind, write nonempty text, cite this room's recent publications or a typed
artifact digest or journal fact code, then **Preview contribution** to see
the non-authorizing effect before **Submit contribution**. The submission
witnesses the Design's statement and workflow heads and each cited source, so
a concluded Design or a changed source refuses beside the kept draft and
leaves every other surface as it was. A pending submission cannot be sent
again, and the contribution appears only when the room fact arrives. A
concluded Design stays readable with no form. The lane's **Design
invitations**, in the Spice sidebar, list the resident's notices of open
Designs matching its claimed work, newest first; following one opens that
Design's view and form without joining its group or acquiring it.

## Toolbar

The toolbar has two rows: the team's task strip, then one status band holding
presence pips beneath the team total and the live ticker in the rest. Pips
follow canonical member order and distinguish active, queued, waiting, bound,
and unbound states without encoding agent identity. The ticker keeps relative
age, status, prefix, accent, accessible label, and live-region semantics; an
empty fleet reserves no pips and invents no status.

## Task strips

The command bar has two mirrored global strips and each team has one local
strip. Every strip uses the same horned-tab construction and leads with a
badge-only total. The global strips keep equal, small browser-edge insets,
including the platform safe area: execution at the left and Design at the
right. The left global total opens all current execution work. The right global
total opens all current Design-owned work: each Design root and each open task
directly controlled by a Design. Global membership is the stable namespace;
active local docking does not remove a task from its global strip. A team total
opens the overview with exactly the tasks its members are currently working,
without changing task authority or accountable ownership. Totals keep one
footprint for every count. Tabs past a total scroll horizontally under edge
fades and never enter the status row.

A team-local task tab appears only while one of that team's lanes actively
claims it, and wears that lane's accent. A group-local tab appears only while
its associated Design work has an active claimant, and wears the accent of the
Design it serves. An accented tab's text is always accent-tinted and its
background carries a 5% accent tint, 10% on hover. Every horn tab's rails,
global or local, keep the neutral line color and undim one neutral step on
hover; no accent paints them. Totals stay neutral, and global strips own their
other interaction feedback independently. Every count badge is one neutral
open-task total; see
[Task counts show one open total](../docs/product/loops.md#the-jitter-contract).

Each global stem tab opens its complete visibility-qualified stem within its
own partition. A local strip has no stem roll-ups: past its total, each actively
worked task is its own tab in the projection's deterministic order. Each tab
opens its exact task, and the local total selects exactly the tabs beside it.
Starting, moving, releasing, lapsing, or completing a claim updates local
docking; it does not change the task's global namespace. Closing any overview
returns focus to the control that opened it.

Typed OOPS rows retain their public project but stay out of allocator and
team-local work until the operator uses `spice task file`; task detail presents
them as Parked. A Draft row is prospective work rather than a report: it shows
its project and editor to the operator and its origin thread, and stays out of
the same work until `spice task file` admits it, so task detail presents it as
Draft. Filed rows are ordinary work. Their filing state, rather than a hidden
stem, controls that placement.

Work keeps immutable creator-team provenance separate from its current
accountable owner. A Design owns itself and tasks it directly controls. All
other work belongs to the team of the lane actively claiming it (no team when
that lane is unteamed), otherwise to a still-live creator team, otherwise to
its last claimant's current team. A task can therefore move from a dissolved
creator team to a live team without losing who created it. Accountable owner
chooses the execution or Design global namespace; it does not by itself dock a
local tab. A Design-produced Plan stays on the right planning strip; tasks that
Plan creates for authorized execution use the ordinary left-global strip.
Ownership is never inferred from dependency reachability, working-group
placement, or a transitive preparation ancestor.

Project controls select the exact dotted project, and a relationship control
extends the current selection with that task. Selection always follows the
resident's offer order and may be explicitly empty.

## Group toolbar

A working group's toolbar keeps team-console geometry and maps each team
element to a design-side analog:

| Team toolbar | Group toolbar |
| --- | --- |
| Team total: opens the overview with exactly the tasks its members are actively working | Group total: opens the overview with exactly the actively claimed work associated with the group's designs |
| One tab per actively worked team task | One tab per actively worked task associated with the group's designs |
| A task handle carries the accent of the lane working it | A task handle carries the accent of the design it serves |
| Presence pips: one per member, by state | Design lights: one per design in its accent, distinguishing open passive, open active, claimed, holding unauthorized outputs, concluded done, concluded collapsed, and deleted, with an accessible label naming the standing; a design whose board row is unavailable invents no standing |
| Ticker: the newest member activity, with age, status, prefix, accent, and live region | Ticker: the newest room publication or coordination event across the group's designs, with the same age, prefix, accessible label, and live region; its accent and status are the design it landed in |

A design light reads its [standing](#design-standing): concluded done,
concluded collapsed, or deleted once terminal, even while a saved group still
holds it; open passive outside every group. An open active Design then reads
holding unauthorized outputs while any output it constructed is pending
without a selection or the group's engagement, then claimed while a lane
actively claims it, and open active otherwise. The group's name closes the
status row; this browser's own move-left, move-right, and close controls live
in the panel's group arrangement menu.

## Team composer controls

Beside the drafts, one control panel holds team TURBO, the team arrangement
menu, the speech and allocation sliders, and SEND, with fixed geometry. The
arrangement menu opens swap and join beside the panel and stays disabled,
naming why, when no other team exists. Team TURBO and Settings global TURBO are
separately admitted controls.

The speech slider reads Quiet / Speak / Narrate and the allocation slider Steer
/ Drive / Drain; Up/Right raise and Down/Left lower on every engine. A speech
selection applies to the team's browser-local speech policy in the same input
that paints it, so nothing speaks under a mode the control does not show.
Quiet refuses every automatic utterance; Speak voices resident-classified final
answers and acknowledged steering; Narrate voices each live card edge. None
reads fenced code or Markdown marks. A pending allocation-policy command admits
no send and lights no recipients. Reduced motion removes slider travel, never a
state distinction.

SEND is one optical system of two layers. Behind it, an emitter spreads an
even diffuse field across the whole disc, ending at the glass's edge: no light
reaches the panel outside SEND, and there is no drawn ring and no center
hotspot. In front, a frosted
glass face transmits that field and collects light in irregular catches along
its curved perimeter: brighter low on the left, a fainter answer high on the
right, as the sliders are lit. Every lit presentation keeps both layers:
hover, keyboard focus, contact, disabled, recipient, `FULL SEND` and command
states change only the field's intensity, the face's pressure, and their own
typed rims, which stay separate layers above the glass.

A team is idle when no member is starting or running and nothing is drafted.
Its controls then carry no light: the emitter goes dark and SEND rests as the
same flush glass as TURBO and the menu, and the sliders keep their stop's accent
but drop the lit thumb's glass catches and halo. Any member starting or running,
or a draft, relights the panel live.

SEND's rim is lit in the actual recipients' accents: one continuous rim for one
recipient, separated arcs for several. Its title names the recipient count.
Recipients are the reachable members whose drafts hold text, quotes, or images
a press would send, including a vacant member a press would wake; an empty
vacant member adds nothing. The rim is paint on the one button, never
per-recipient hit targets. A running team with nothing drafted shows no rim; an
idle team with no agents or drafts disables SEND while the sliders stay usable.

A reachable vacant member uses the same durable steering submission as an
occupied member. Its residency witness names no occupant and carries the
current generation; after the directive commits, the resident launches or
resumes the lane. There is no intermediate browser-owned wake state or second
command. Later submissions for that lane wait for a causally newer lane
snapshot before reusing either a vacant or occupied witness. A refusal restores
the draft through the ordinary submission path.

In a multi-member team, SEND becomes a `FULL SEND` lockup, in the same box and
accessible name, exactly when the submission reaches every member. An active
broadcast preview substitutes its actual recipients; broadcast mode alone never
implies full coverage.

Pointer, keyboard, and touch contact depress SEND without intercepting native
activation. Held submit modifiers add a command ring outside the recipient
rim: the focused agent's for an ordinary submit, the theme's ink for a
broadcast.

## Composer and stream

The composer rides in the Chat plane's own scroll content, and its measured
height is the plane's spacer and scroll padding; exposure follows [Chat scroll
passively controls composer
exposure](../docs/product/invariants/lifecycle.md#surface-chat-scroll-passively-controls-composer-exposure).
Programmatic focus moves the caret without scrolling the viewport, fleet,
member strip, or plane; the composer's own focus transition still reveals a
collapsed draft. The fleet opens at its canonical right edge and holds it
through resizes only while the operator leaves it there; once they scroll to
another team, a keyboard, rotation, or arriving team keeps their place.

A team's member strip paints canonical member zero at its right edge, beside
the controls, so its document order runs right to left. Tab and Shift+Tab
instead read as a person does: across one team's member composers left to
right, each member's own heading and fields in order, then the team's
controls, then the next team the same way. Everywhere else, and inside an
open modal dialog, the browser's own order holds.

Each member heading resolves from that lane's newest eligible authored prose.
When a history page has none, header demand continues to a newly exposed older
frontier but never reissues the same frontier without fresh viewport demand.

## Image runs

A lone tool image renders as a cover card. In the message plane, two to four
such cards in a row from one speaker, each at most two seconds after the
previous, crystallize into one card at the run's first place; a fifth image, a
longer gap, another speaker, or any other card starts a new run. The crystal
keeps one image card's footprint and divides it into one cell per image in
capture order. The cells follow each image's natural aspect, as the browser
decoded it for display, and the card's own box, so wide images take wide
cells, tall images take tall cells, and no image is badly cropped or shrunk to
a sliver; the cells divide again as images load and whenever the card's box
changes shape. Each cell opens its own image full size. Hover and keyboard
focus tint and ring only that actionable cell; the grouped card and its other
images remain at rest. While a lane image is open full size, Right Arrow opens
the next newer rendered image and Left Arrow the previous older one. Every
crystal cell is one stop, navigation never crosses lanes, and either end holds
without wrapping. The effect is the plane's presentation only: narration,
quotes, receipts, and every other surface still see one message per image.
