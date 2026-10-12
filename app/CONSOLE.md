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

SEND floats over a neutral transmitted-light layer using the sliders' opposing
highlights, low shadow, frost and radial fade. That layer stays behind the
button face and responds only in intensity to availability and contact; it
never carries recipient color or replaces the rim.

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
