# Always-chat team console

Every populated or empty team surface keeps its Chat composer and message plane
mounted continuously. It has no Work/Chat rail, local Work pane, mode-owned
scroll state, or return-to-mode behavior. The toolbar has no hamburger. Its
task strip keeps the full first row. The second row is one full-width status
band: a two-row, three-column matrix of 8px presence pips sits at its physical
left, followed by the ticker in the remaining width. Canonical members fill
bottom-to-top and right-to-left: #1 lower-right, #2 upper-right, #3 lower-middle,
#4 upper-middle, #5 lower-left, #6 upper-left. Partial populations keep these
positions; the app's outer LTR flow is unchanged. The status row uses the same
theme-aware surface as the message stream, with one composer-bottom-strength
top rule throughout;
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
status row. That full-width row owns one continuous top rule; its pip matrix
reserves only the leading segment, not a column above the composer controls.
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
task tabs scroll beneath mirrored alpha screens at both ends; the leading
underlap reaches back beneath the total. The trailing end reserves no pip tab.
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
