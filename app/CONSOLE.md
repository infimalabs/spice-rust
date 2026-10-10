# Always-chat team console

Every populated or empty team surface keeps its Chat composer and message plane
mounted continuously. It has no Work/Chat rail, local Work pane, mode-owned
scroll state, or return-to-mode behavior. The toolbar has no hamburger. Its
task strip keeps the full first row. The second row is one full-width status
band: the ticker leads into a fixed 80px pip segment directly above the
physical-right send/slider column. Up to six unified 8px pips occupy that
segment in the same rendered order as the composer columns. Both sequences use
local RTL flow so canonical member zero remains control-adjacent; the app's
outer LTR flow is unchanged. The status row uses the same theme-aware surface
as the message stream and one composer-bottom-strength top rule throughout;
bound and unbound status dims only its text, never that surface. The ticker
retains relative age, lane-status class, agent/tool/command prefix and accent,
accessible label, and live-region semantics; an empty fleet has no pip reserve
and keeps the full surfaced status slot without inventing status.
Adjacent teams share one faint vertical seam on the physical-right edge of the
left team. It starts below the composer block and its painted continuation,
continues to the fleet bottom, and takes no layout space; neither outer fleet
edge is framed, so a lone team has no vertical border.


The toolbar is a fixed-height grid whether or not the team owns work. It has no
outer frame or shadow: the full-width task rail sits above the stream-surfaced
status band, while the team surface frames nothing around them. The 56px
circular send button remains centered in its independent 80px physical-right
composer-control column. That column's opaque field is the stream surface, so
it reads apart from the composers' lighter glass and as one band with the
status row. The status band's matching 80px segment aligns directly above it,
shares the ticker's stream-surface fill and stronger top rule, and centers up
to six 8px pips in the same local right-to-left order as the composer columns.
When a composer is fully collapsed, its bottom edge matches that status rule,
visually bracketing the status row. Pips retain the unified presence palette:
active, queued, waiting, and dim bound/unbound states remain distinguishable
without encoding agent identity.
The global task strip and each team's task strip begin at the same 8px inline
inset, while the Spice button retains its trailing command-bar inset. Global
and team-local work share one horned-tab construction on the composer's dark
`--control` surface rather than the brighter message-stream surface. The
global `.app-global-work-tabs` rail hangs its tabs from the window's top edge
with rounded bottom corners; a team's task band stands its tabs on the status
rule with rounded top corners. Each strip
leads with a badge-only total, identified by `data-work-scope`, one globally
and one per team; its inline-start and inline-end rails take the message card's
`--message-rail` width. Each following stem tab uses the nested quote's
narrower `--quote-rail` width on both sides. Both rail treatments taper into
the message card's own `--app-radius-card` corners, with no edge along the
glued side and a square open side; no
handle carries a rail of its own. Both rails take `--line`, the colour of the
status band's top rule the team tabs stand on, so each team tab and that rule
read as one stroke. A global tab is a stem roll-up: one stem label and its
badge, with no task breakouts. Every `.app-count-badge`, global or
team-local, uses the same muted neutral background regardless of the
lifecycles it counts. The stem label is muted at reduced opacity;
unworked handles are muted, dormant ones weaker. A handle a lane is working
names that lane in its text, the lane accent mixed 55% toward `--ink`, and in a
5% tint of the accent mixed into `--control`; unworked targets tint in
`--muted`. In-flight handles are not dimmed, because text and tint carry
ownership. Every target rises to a 10% tint on hover; keyboard focus uses the
same neutral muted outline in both strips rather than an accent colour. Labels
sit 10px in from either rail: the stem starts and the last handle ends with
10px, and every other join has 8px on each side. An 8px outer inset keeps the
team flags off the browser edge, and adjacent tabs in either strip are
separated by the
message stream's inter-lane `--mosaic-gap`; team flags never join the status
rule below. The team total's sole badge counts every task the band shows, which
is the sum of the stem badges and the number of handles. Activating it opens
the unified overview with the team's complete current task set already
selected. It does not change the durable location.

The team total holds its own column at the rail's inline start, so the first
stem tab sits one `--mosaic-gap` past it whatever its badge width. Stem and
task tabs scroll beneath mirrored alpha screens: a short leading fade reaches
back under the total and through that gap so tabs disappear behind it, while
the trailing fade spans the full 80px composer-control and pip column.
Matching inline and scroll padding keeps the first and last tabs fully opaque
at their respective extents and keeps a no-overflow rail out of both fades.

Flags follow Work's clustering rule: from in-flight, ready, blocked, and
deferred offer order, each visibility-qualified stem's first task places one
`.app-horn-tab--group` flag, and later matches join that flag without changing
their relative order. Its `.app-horn-tab__target` button displays the raw stem
in the muted blocked tone with an `.app-count-badge` of the stem's open tasks,
and opens the same overview with that team's matching stem selected. The
following `.app-horn-tab__handle` buttons open the exact task. Closing any
overview returns focus to the control that opened it. Flag overflow retains its
ordinary horizontal scroll inside the screened band without entering the
status/pip row.

The global total opens every current task;
global stem controls open their complete visibility-qualified stems. Team
membership is the team of the lane actively claiming a task, or no team when
that lane is unteamed; otherwise its current creator team; otherwise its last
claimant's team. Project controls select the exact full dotted project, and a
relationship control extends the current selected set with that task. Selection
always follows the resident's admitted offer order and may be explicitly empty.
The overview contains the complete selected graph and task details plus the
resident's running or awaiting-delivery jobs.

Chat is the only durable surface, at `#/chat`; retained cards extend it with
`/card/{target}`. Work, task, and direction controls are browser-local overview
actions: they neither create history entries nor claim a restorable URL. Stale
former Dash, Work, or Directions URLs normalize to Chat under the ordinary
unavailable-location notice.

The composer columns, speech and allocation sliders, and send controls retain
their dimensions. Removing the standalone ticker row gives that block height
to the message stream instead of resizing the controls. Wide, narrow,
short-height, empty, one-member, and multi-member consoles keep a usable header,
composer, and stream without a new nested vertical scroller.

## Team composer controls

The primary panel uses Flush etched glass in a sharp-cornered 80 × 160px frame.
Round 34px TURBO and team-menu buttons share the first row; two pill-shaped
slider flyers follow, and round 62px SEND anchors the panel below them. Faces
transmit the neutral background, with fine etched edges and no opaque
recipient-color fill, domes, or nested ridges. Team and Settings global TURBO
retain the same lettering:
muted when off, brighter ink and a neutral glass wash when on, without colored
active rims. Each remains its own separately admitted control.
Task-observation Add/Close retain their independent appearance.

Slider flyers and their one selected label settle together over 180ms. The
label remains continuously painted inside its flyer during travel; labels do
not crossfade, overlap, or disappear between stops. The native ranges retain
their stops and hit targets, including the 2px endpoint inset. Lettering has a
subtle etched highlight. A speech selection applies to the team's browser-local
speech policy in the same input that paints it, so no narration speaks under a
mode the control no longer shows; message cards do not depend on that mode and
stay unrendered. Reduced motion removes travel without removing state
distinctions.

SEND lights one 2px perimeter rim in actual recipient accents: a continuous rim
for one recipient, equal separated arcs for several. The button title names the
recipient total, which follows sendable typed text, quotes, and images under
current command admission. A
running team without drafted content has no recipient light; an idle team
without agents or drafts dims and disables SEND. A pending allocation-policy
command admits no send and lights no recipients. Existing wake/send and
attachment behavior stays authoritative, not the reference gallery's simulated
readiness.

For teams with multiple members, `FULL SEND` materializes as one compact
lockup exactly when the active submission reaches every member. Its outer box
always stays the width and center of ordinary `SEND`: condensed `FULL` occupies
the original `SE` half, condensed `SEND` occupies the original `ND` half, and
`FULL` uses the focus color. Ordinary submission derives that state from the
reachable members whose composers hold text, quotes, or images, including
wakeable non-running members. While a broadcast preview is active, its actual
broadcast recipients replace that calculation; broadcast mode alone never
implies full coverage. Partial sends and single-member teams show only `SEND`.
The lockup stays inside the existing button geometry and does not change its
accessible name.

Pointer and keyboard contact move SEND down 1.5px, deepen its upper contact
shadow, and dim the rim; release restores its flat face without grid reflow.
Touch-only contact is projected from native pointer events and clears on release,
cancellation, or lost capture without intercepting activation. Reduced motion
removes transitions, not contact feedback. Held ordinary-submit modifiers add a
1px focused-agent command ring outside the 3px recipient rim, so the recipient
bands stay the heavier line; a held broadcast uses its occupied recipients' rim
while retaining the neutral face, and its command ring is their light combined
into the theme's ink signal, near white on dark and near black on light, rather
than any one accent.

Vacant members do not create broadcast recipients, and an empty vacant composer
contributes no ordinary SEND rim arc: when no member has sendable content, the
button removes its recipient-rim paint while retaining the neutral face.
Ordinary arcs and the recipient total in the button title come only from
reachable members holding sendable content; a vacant member joins them only
when it holds a draft that a press would wake it to receive. The rim is paint
on the one SEND button, never a set of per-recipient hover targets, so an empty
vacant member adds no arc, seam, label, or hit region.

Member drafts and the control column are one zero-gap field. Drafts gain a 10%
lane-accent tint when lit. One standard 31% glass thickness is the shared
primitive; the dark veil uses it while the light theme retains its softer base.
Composer headings use the derived 62% theme-control paint as two thicknesses
and retain their full-width accent underlines. Behind the entire form, one
pointer-inert composited screen continues 10px over the message stream. The
control frame uses one derived 93% theme-control paint as three thicknesses and
holds it through the full height of the neighboring text areas. That paint
fades smoothly across the 10px continuation below instead of exposing the
stream inside the frame. The continuation itself paints its opaque 1px top
inset rule above the control paint, using the shared backdrop edge color so the
boundary remains continuous. Equal soft elevation appears on both inline
sides; at a team boundary the following composer carries the otherwise-clipped
right cast, letting either neighbor read beneath the raised controls without
moving either surface. The faint sibling separator continues that internal
boundary through the message plane without painting either browser edge.
The screen owns the form glass, one complex theme-derived background, and one
10px frost filter. Its header edge eases from an opaque control line through
stronger control color
into ordinary glass. The form has no physical lower edge or strengthening band;
the ordinary veil simply continues 10px beyond it while a mask fades that
continuation to transparent. No backdrop-filter boundary exists at the form
edge. The continuation depth is declared once in CSS and counts as composer:
collapse travels the measured height plus that depth, so the feather tapers out
of view before the form is fully collapsed and tapers back in first on reveal.
Only then does the message stream's top rule appear, fading in rather than
switching on, and without motion under reduced motion. The shared control edge
remains opaque and theme-derived
for other control consumers. Member headings retain
their accent underline, focused or broadcast drafts merge their full-width
highlight, and vacant members mute content without changing the surface.
An empty message plane reserves that same continuation depth below the composer
before the pending-direction notice, so its overflow clip retains the full
feather even before an agent has emitted prose.
The composer rides in the Chat plane's own scroll content, so every engine's
scroll moves it in the same frame as the cards. It sticks to the plane's top
edge, so scrolling back never opens a gap above it, and measured composer
height remains the message-plane spacer and scroll padding. Between
exposure edges, one fixed absolute rest lets native scroll carry the composer.
At either edge sticky positioning owns continued motion without repeated layout
writes; the first reversed delta reanchors the rest once, so reveal or collapse
begins immediately. Programmatic focus moves
the caret without browser focus scrolling the viewport, team fleet, member
strip, or message plane. The composer's existing focus transition still reveals
a collapsed draft fully, and the plane's scroll padding keeps other
browser-scrolled targets clear of the exposed composer only while focus is
outside it. The team fleet opens at its canonical right edge and holds that
edge through resizes only while the operator leaves it there; once they scroll
to another team, a keyboard opening or closing, a rotation, or a team arriving
keeps their place.

The range words ride inside their pill flyers. Each 22px-high thumb travels
inside a 26px-high groove, inset 2px at both ends; label travel follows that
same rail at every stop. Both rows use one light map: Quiet and Steer keep the
neutral groove, Speak and Drive illuminate its complete backplane green, and
Narrate and Drain illuminate it amber. Each colored stop is one uniform
full-groove wash, without a dark center band or visible point emitter. The
flyer's near-clear center transmits that wash while opposing inset bevels,
curved edge caustics, and low-radius backdrop diffusion gather the color into
a thick lens perimeter rather than painting a stripe across its face.
Allocation reads STEER / DRIVE / DRAIN. Every engine maps Up/Right upward and
Down/Left downward.
Narrate voices live card edges; Speak voices resident-classified final answers and
acknowledged steering. Neither reads fenced code or Markdown marks, and loaded
history never auto-plays.

Each member heading resolves from that lane’s newest eligible authored prose.
If one history page has none, header demand continues to a newly exposed older
frontier, but never reissues the same frontier without fresh viewport demand.
An idle team with no draft, quote, or image dims the send frame while leaving
the sliders usable; draft input rerenders that frame only when its active answer
changes.

The panel begins with a three-pixel top inset and has four rows separated by
three three-pixel gaps: a 34-pixel two-slot button row, two equal 26-pixel
range rows, then the 62-pixel SEND row, because
$(160 - 3 - 34 - 62 - 3 \times 3) / 2 = 26$. TURBO occupies the upper-left slot
at the global Spice control's diameter, with equal three-pixel top and left
clearance; the team-menu button in the upper-right slot opens the swap/join
arrangement menu beside the control column and closes it again. Each range runs
left to right, minimum to maximum, as a 46 × 22 labeled thumb in a 74 × 26
groove with three pixels of panel inset at each end. Every current-value word
fills the same 32 × 9 label box, centered with seven pixels of horizontal and
six-and-a-half pixels of vertical thumb clearance; the box clips overscan before
it reaches the rounded edge.
