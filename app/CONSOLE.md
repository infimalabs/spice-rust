# Always-chat team console

Every team surface, populated or empty, keeps its Chat composer and message
plane mounted. There is no Work/Chat mode, local Work pane, or mode-owned
scroll state. Chat is the only durable location, `#/chat`, extended by
`/card/{target}` for a retained card; any other location normalizes to Chat
with the unavailable-location notice. Work, task, and direction controls are
browser-local overview actions that create no history entry and claim no
restorable URL.

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
mount, reorder, or close a browser's foreground views. **New Team** therefore
remains lane creation; **New working group** is repository organization.

In the Spice menu, Jobs follows the operator identity before the Working groups
catalog and its New working group and ungrouped Designs targets. The catalog is
an ordinary sheet section: its prominent primary title, secondary label,
inset, and spacing use the same heading row as Jobs, Resident topology, and
Provider quota at every sidebar width.

The browser reads an opened group as a team console with tasks and agents
swapped: one composer per design, posting to that design's room, and one tab
per lane currently claiming an open task associated with any of its designs.
That lane list is derived from board claims on every read; the group stores
and grants none of it.

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
force-frees work or releases child claims and unrelated direction. The
initiating browser hides the room locally. Room history, drafts, groups,
selected outputs, other browsers, and repository authority remain unchanged.

The native command surface is `spice room status HANDLE`,
`spice room start|resume HANDLE ADDRESS --criterion TEXT --contribution TEXT
--stop TEXT [--auto-resume N]`, and `spice room put-aside HANDLE`. An
`--auto-resume` rule lets the resident repeat that Resume up to N times as
[new room input arrives](../docs/product/invariants/identity.md#identity-an-operator-rule-bounds-automated-room-resumes).

## Room standing

Each design's menu opens with the room's standing, derived from the room
snapshot, its live facts, the board, and lane topology on every render:

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
  - a claim-bound stop request.
  Aging removes only activity and invitation; history and native duties stay.
- **Relevance**: the resident's shadow relevance observations, each with its
  classification, progress standing, explanation, exceptions, uncertainty,
  source coverage, and operator verdict.

Agreement, silence, message count, elapsed time, ACK, and ANS never read as
progress or approval. The section only renders inside the explicitly opened
menu: a status, relevance, or automation update changes its text in place and
never mounts a room, moves a console, or wakes a lane.

## Toolbar

The toolbar has two rows: the team's task strip, then one status band holding
presence pips beneath the team total and the live ticker in the rest. Pips
follow canonical member order and distinguish active, queued, waiting, bound,
and unbound states without encoding agent identity. The ticker keeps relative
age, status, prefix, accent, accessible label, and live-region semantics; an
empty fleet reserves no pips and invents no status.

## Task strips

The global strip and each team's strip share one horned-tab construction. Each
leads with a badge-only total: the global total opens every current task, and a
team total opens the overview with that team's complete current task set
selected without changing the durable location. Both totals keep one footprint
for every count, so the totals and the pips beneath stay aligned. Tabs past the
total scroll horizontally under edge fades and never enter the status row.

Task tabs are neutral. Only a handle a lane is working carries that lane's
accent, beside the lane's name in its text; hover and keyboard focus stay
neutral in both strips. Every count badge is one neutral open-task total; see
[Task counts show one open
total](../docs/product/loops.md#the-jitter-contract).

A global stem tab opens its complete visibility-qualified stem. A team strip
clusters by Work's rule: in in-flight, ready, blocked, and deferred offer order,
each stem's first task places one flag and later matches join it in order. The
flag's stem target opens the overview with that team's stem selected; each
following handle opens its exact task. Closing any overview returns focus to the
control that opened it.

A task belongs to the team of the lane actively claiming it (no team when that
lane is unteamed), otherwise its creator team, otherwise its last claimant's
team. Project controls select the exact dotted project, and a relationship
control extends the current selection with that task. Selection always follows
the resident's offer order and may be explicitly empty.

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
even diffuse field across the whole disc that softens into the panel just
beyond the glass, with no drawn ring and no center hotspot. In front, a frosted
glass face transmits that field and collects light in irregular catches along
its curved perimeter: brighter low on the left, a fainter answer high on the
right, as the sliders are lit. Every presentation keeps both layers: idle,
hover, keyboard focus, contact, disabled, waking, recipient, `FULL SEND` and
command states change only the field's intensity, the face's pressure, and
their own typed rims, which stay separate layers above the glass.

SEND's rim is lit in the actual recipients' accents: one continuous rim for one
recipient, separated arcs for several. Its title names the recipient count.
Recipients are the reachable members whose drafts hold text, quotes, or images
a press would send, including a vacant member a press would wake; an empty
vacant member adds nothing. The rim is paint on the one button, never
per-recipient hit targets. A running team with nothing drafted shows no rim; an
idle team with no agents or drafts disables SEND while the sliders stay usable.

A press that wakes a vacant member keeps the panel lit from submission until
the woken lane binds residency, so draft-to-online has no dark interval. Until
then SEND is disabled unless more content is drafted, titles itself `Waking N
agents`, and breathes a rim in the waking members' accents, steady under
reduced motion. A refused, faulted, or unproven wake clears that indication as
it restores the draft. The indication reflects only this browser's own wake
submissions.

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

Each member heading resolves from that lane's newest eligible authored prose.
When a history page has none, header demand continues to a newly exposed older
frontier but never reissues the same frontier without fresh viewport demand.
