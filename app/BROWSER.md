# Browser checks

## Browser evidence follows the host system theme

Use the repository-owned `launchAppBrowser` helper for disposable app
browser checks. It detects the host appearance and gives the headless context
the matching `prefers-color-scheme`; callers do not repeat theme setup:

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

An intentional theme-matrix check overrides `contextOptions.colorScheme`.
Ordinary screenshots and visual checks retain the system default. The returned
`colorScheme` and `colorSchemeSource` report the selected value and whether it
came from `"system"` detection or `"explicit"` configuration. This browser
preference never changes an explicit app preference: a stored
`spice:app-theme` still wins.

```js
const lightApp = await launchAppBrowser(chromium, {
  contextOptions: { colorScheme: "light" },
});
```

## Browser branding and remaining work

The tab and menu use the lowercase brand `spice`, always trailing changing
state. Titles are `5 tasks - spice`, `1 task - spice`,
`7 tasks (stale) - spice`, `Loading - spice`, and
`Disconnected - spice`; the static page title is `spice`. Speech never retitles
the tab.

Once a board snapshot arrives, the title carries the exact remaining public
task count and the generated SVG pepper icon carries the same badge. Icon
labels are exact through 99 and then `99+`; zero is explicit. Loading and
disconnected states use ellipsis and exclamation marks. Reconnect retains the
last count in amber and marks the title stale; current data restores green.
Title and icon are rewritten only when this state changes.

## Provider quota rows

Each account-limit row reads from left to right as the limit name, flexible
capacity bar, amount, and reset. Percent amounts describe the remaining quota
as `N% full`; counted limits retain their explicit remaining and total units.
The filled capacity starts at the bar's left edge, so consumption exposes the
track from the right. The amount stays immediately beside the bar it describes
rather than separating the name from that bar.

An account with a shared weekly limit keeps the same capacity-order accent in
its card surface, edge, provider label, and usage bars as its layer in the
Spice-button quota fill. Reports that do not contribute a weekly layer remain
neutral.

The reset-credit summary always states both available and redeemable counts;
when the provider omits the latter, the available count supplies it. Each
redeemable credit with provider detail owns a full-width line. Its
provider-authored title or program leads; otherwise `Reset credit granted`
and the local grant date identify it, with `Reset credit` reserved for an
absent or invalid grant time. A valid expiry stays right-aligned as an `expires`
relative countdown with exact-time hover detail.

## Graph-first task overview

Task activation opens one contextual overview for the complete selected task
set. The selection is an ordered, stable-deduplicated list: every selected
root remains visible even when the set is larger than the relation command's
former root or node limits. Scope and project entry points derive that list
from the locally admitted board in its offered order.

The overview is one continuous modal surface. Its admitted-board graph renders
first, including every selected root, roots with no edges, and explicit
outside-board or outside-selection neighbors. The same graph offers focused
Dependency, Lineage, Reviews, and Lifecycle lenses. Dependency direction and
typed depth/node controls bound optional resident enrichment without bounding
the local selection; loading, refreshing, unavailable, retry, and explicit
truncation/frontier states never replace or silently omit selected roots.
Lineage preserves exact task, ACK, and origin identities. Review chains retain
phase author → reviewer → verdict order. Lifecycle facts retain their durable
source order. Zoom, pan, reset, and task-node activation remain available
without changing the selection.

Every selected task then owns a full detail section below the graph. Current
ownership and claim, task state and contract, project/team location, timestamps
and lifecycle, origin and dependencies answer who, what, where, when, and why
from live operational state. Evidence, blockers, relationships, provenance,
activity, notes, loading, unavailable, and retry states appear simultaneously
rather than behind tabs, pagination, or next/previous controls. Activating a
selected graph node scrolls and moves focus to that task's detail section;
activating a related external task follows that lead in the same overview.
Escape, backdrop, and Close dismiss the overview and restore the prior focus
when it still exists. Reduced-motion mode removes the sheet and graph
transitions.

## Arrange whole teams

Pick up a team from its status-pip stack. After the same six-pixel lift
threshold used by composer movement, the source surface stays reserved and a
bounded ghost follows the pointer. The middle three fifths of another team
exchanges exactly those two team positions. Either outer fifth joins into the
team under the pointer: the receiver keeps its identity, primary composer, and
member order, then receives the source members in their existing order.

Interactive joins stop at six members. A seventh-member edge target is shown
as unavailable and sends no command. Press Enter or Space on the pip stack, or
tap it, for the equivalent **Swap positions** and **Join into** menu. Every
accepted interaction sends the complete topology at the revision currently
drawn by the app; cancellation, a changed revision, or a removed source clears
the lift without sending.

## Team status line

The activity age keeps its fixed three-character measure and meets the main
status with only the status line's ordinary inline spacing. It adds no trailing
margin of its own.

## Team composer controls

The 56px Send action occupies the top of an 80px control column, with the
speech and allocation vertical ranges below it. Its label stays “Send”; color
and the adjacent allocation value distinguish Steer, Drive, and Drain. Hover,
press, reduced-motion, ordinary-submit, and broadcast-preview states change
paint only, never geometry.

Member drafts and the control column are one zero-gap field. Drafts share the
overlay glass and gain a 10% lane-accent tint when lit; the control column uses
the matching opaque field plus one faint start rule. Member headings retain
their accent underline, focused or broadcast drafts merge their full-width
highlight, and vacant members mute content without changing the surface.
Measured composer height remains both the overlay collapse distance and the
message-plane spacer; focusing a collapsed draft reveals it without scrolling
the plane.

The range values sit outside their thumbs as one centered phrase. Quiet and
Steer are neutral, Speak and Drive use the active accent, and Narrate and Drain
use warning. Every engine maps Up/Right upward and Down/Left downward. Narrate
voices live card edges; Speak voices resident-classified final answers and
acknowledged steering. Neither reads fenced code or Markdown marks, and loaded
history never auto-plays.

Each member heading resolves from that lane’s newest eligible authored prose.
If one history page has none, header demand continues to a newly exposed older
frontier, but never reissues the same frontier without fresh viewport demand.
An idle team with no draft, quote, or image dims the send frame while leaving
the sliders usable; draft input rerenders that frame only when its active answer
changes.

## Lane menu

Each Chat lane's menu button swaps that lane's composer for its seat panel
instead of opening a floating menu. The panel carries everything one agent
offers. It leads with what the agent needs from the operator now: consent
requests to Allow or Deny in place, held tasks iced, blocked, or in review,
a lease within two minutes of expiry, and direction waiting over five
minutes. Then come the Wake, Drain, Remove, Renew, and next-launch model,
reasoning effort, and fast mode commands and refusals; Renew submits
immediately, while Drain and Remove confirm first. Fast mode is one on/off
toggle right after effort, tinted like the spice menu's global Fast mode when
on. It shows the seat's next-launch value, which follows that global setting
until a press pins the other value; pressing back to the global value clears
the pin. Effort and fast mode each report the running value beside the next
launch, and a running OMP agent takes either change at once, so its composer
icon gains or drops the fast badge once omp confirms. Presence and lease age,
the current task, latest activity, waiting direction, copyable agent channel,
occupant, session, and worktree follow, beside Create new team and Leave all
teams. Wide lanes lay those groups side by side; narrow lanes stack them and
scroll within the lane. The button, Escape, or a click elsewhere in the team
restores the composer.

Each lane menu's neutral hamburger takes that member's accent for hover,
press, keyboard focus, and open states, so neighboring lanes never share an
unrelated highlight color.

## Composer image strip

Draft images fill the composer heading's usable block height as edge-to-edge
cover crops. Thumbnail wrappers add no border, inset shadow, or vertical dead
space; adjacent images share only a 1px seam and use 1px corner radii. Hover
and keyboard focus draw inside the crop without moving it. The overlaid remove
control and touch-only image chooser retain the same compact geometry in both
primary and quoted composer shards.

The chooser preserves admitted image bytes when the complete draft can fit one
durable steering record. A larger decodable camera or gallery image is flattened
onto white, resized, and encoded as a canonical JPEG before it enters the draft,
using only the record budget left by existing attachments. The shared image
count, per-image byte, content-type, and aggregate record limits remain the
submission authority; a concurrent attachment that consumes the reserved space
is refused in the composer rather than failing after Send.

## Composer provider watermark

Each Chat lane paints its provider emblem as a small, faint watermark in the
lower-right corner of every draft composer, both the primary composer and each
message-pinned quote composer. It occupies no layout space, accepts no pointer
events, and sits behind the transparent textarea so selection, typing, pasted
images, and quote controls retain the entire composer. Provider identity remains
available through each emblem's accessible label, while repeated standalone pi
marks use distinct SVG gradients. Dash agent tiles keep their full-size provider
emblem.

## Annotation dismissal

The image annotation editor compares its current marks with the shape set it
opened. Clicking unused canvas space only clears selection; it never dismisses
the editor. Cancel, Escape, and the outer dialog backdrop close immediately
while that shape set is unchanged. Once a mark is drawn, moved, resized,
nudged, or deleted, those exits require confirmation before discarding the
session; undoing back to the opening shape set removes the confirmation.
Declining keeps the complete edit history, while Apply commits without a
discard prompt. Dirty annotation work also participates in the browser's
existing page-unload warning.
The commit controls remain a joined pair; Apply keeps its accent fill and
contrasting label inside their frosted frame.

## Message card control flow

Every attributable prose hunk owns one floating Quote/Play action island at its
top inline-end. Attribution and prose remain in normal flow beside it. The
renderer records exact raw/authenticated quote depth, cumulative block spacing,
and card-leading ancestry as trusted metadata; CSS uses those values to dock the
island against the card edge at arbitrary nesting depth without inline styles
or finite depth selectors. A zero-width two-line prop keeps associated image
rows below the controls.

Quote and Play form one 35px control pair with one internal seam and no outer
right edge. Their fill follows the owning lane or quote accent, with neutral
glyphs and paint-only hover, press, speaking, and reduced-motion states.
Playback belongs to one exact hunk: automatic speech uses direct prose, quoted
sources retain independent Play controls, and keyed portal roots preserve each
hunk’s callback. Narrow cards keep this order rather than adding a controls
header.

Committed cards occupy a fixed 9px lattice perimeter and shared seams carved
from the admitted rectangle. Prose cards center their painted height within
whole-row cells; cards add no private fill, retaining their lane rail, matching
rail-radius corners, final-answer wash, and hover wash. Measurement clones use
the same geometry.

A single-image message uses its card as an edge-to-edge cover. Focus paints one
card-sized boundary, hover and FINAL use whole-cover screens, and attribution
stays in a compact top-left frost while actions remain top-right. Multi-image
or mixed-content hunks instead use one inline-start row with stable preferred
sizes; prose wraps beside it and narrow rows shrink images evenly.

## Attribution placement

Every attributable run between nested quote blocks is one speaker-owned hunk.
The hunk starts with that source's control island, relative age, name, and one
segmented classification pill. The pill orders ACK or NAK first, FINAL second
when present, then TASTE, REVIEW, and BROADCAST roles; each segment keeps its
own authenticated color and interaction. Ordinary prose begins immediately
after that compact attribution sequence on the same line.
When that opening run continues into another paragraph, the later paragraph
keeps the ordinary Markdown paragraph gap; making the first paragraph inline
for attribution does not collapse the boundary between them.
Within each paragraph, ordinary browser whitespace collapsing applies. The
Markdown structure owns authored paragraph, break, list, quote, and code
boundaries; paragraph CSS does not reinterpret source spacing as preformatted
flow.

All images associated with the hunk are consolidated next, floated at the
inline start, and followed by the remaining prose. A nested quote interrupts
the hunk; direct text after it starts a new hunk with the surrounding source's
controls and attribution again. This one rule applies recursively to top-level
agent prose and resolved operator, agent, and peer-lane quotes.

The control island floats at the inline end of the hunk. Its speak action uses
only that hunk's rendered text, and its quote action retains that hunk's exact
authenticated target for every source, operator-authored included, so the
operator can reply against their own earlier words. Authenticated marker nodes
remain in the Markdown DOM for quote semantics but do not duplicate the
classification pill in prose.

Board provenance is independent and terminal. Each complete quote layer keeps
one empty inline task target directly after its speaker's last word, in its
final hunk even when a nested quote follows; Work fills that target with
origin-task badges. Empty, it takes no room; filled, it floats to the inline
end of that final line, set apart from the word, and the browser wraps it
beneath only when the line has no room. A layer with no words of its own keeps
the target on a line after its content. The speak action never reads it.
Every task pill, in prose, quotes, and the unified task overview, is that same
badge: accent type on a 6% accent tint at the quote-tail scale.
Repeating source attribution around a nested quote never repeats or relocates
the quote layer's task provenance.

Quotes at every depth carry no fill of their own; their text keeps the muted
tone and smaller size. A quote's rail shows its speaker the way a top-level card
does: a quote attributed to an agent, or a peer lane's directive context, wears
that lane's accent, the same slot as the lane's own cards. The neutral accent,
the ink tone of the operator's initials, is the accent of operators and of the
system itself, so operator, Spice, and taste, broadcast, and review notice
quotes take the neutral rail and rely on their pills and attribution, as do
unattributed quotes and nested quotes with no speaker of their own. An
unavailable quote keeps its warning rail.

## Always-chat team console

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
left team. It starts at the status row, continues to the fleet bottom, and
takes no layout space; neither outer fleet edge is framed, so a lone team has
no vertical border.

Chat itself is never unmounted. Every other destination leaves the inactive
agents workspace inert, `aria-hidden`, out of flow, and at
`content-visibility: hidden` rather than `display: none`. It stays fixed below
the commandbar with the same padding and frame edge it has when shown. It skips
rendering but keeps that layout, so cards that arrive meanwhile are still
measured, scroll positions hold, and returning to Chat reuses the layout without
any plane, card, or team surface observing a resize. Browsers without
`content-visibility` fall back to `display: none`.

The toolbar is a fixed-height grid whether or not the team owns work. It has no
outer frame or shadow: the full-width task rail sits above the stream-surfaced
status band, while the team surface frames nothing around them. The 56px
circular send button remains centered in its independent 80px physical-right
composer-control column. That column's opaque field matches the composers'
glass at rest and continues their soft top rule above the send button. The status band's
matching 80px segment aligns directly above it, shares the ticker's
stream-surface fill and stronger top rule, and centers up to six 8px pips in
the same local right-to-left order as the composer columns. When a composer is
fully collapsed, its bottom edge matches that status rule, visually bracketing
the status row. Pips retain the unified presence palette: active, queued,
waiting, and dim bound/unbound states remain
distinguishable without encoding agent identity.
The global Spice and Dash-avatar circles share one diameter and equal top,
bottom, and outer-edge insets. Global and team-local work share one horned-tab
construction on the composer's dark `--control` surface rather than the
brighter message-stream surface. The global `task-pill-strip` hangs its tabs
from the window's top edge with rounded bottom corners; a team's task band
stands its tabs on the status rule with rounded top corners. Each strip leads
with a badge-only total, `task-pill--all` globally and `task-pill--total` per
team, whose inline-start and inline-end rails take the message card's
`--message-rail` width. Each following stem tab uses the nested quote's
narrower `--quote-rail` width on both sides. Both rail treatments taper into
1em corners, with no edge along the glued side and a square open side; no
handle carries a rail of its own. Both rails take `--line`, the colour of the
status band's top rule the team tabs stand on, so each team tab and that rule
read as one stroke. A global tab is a stem roll-up: one stem label and its
badge, with no task breakouts. Every `task-pill-count` badge, global or
team-local, uses the same muted neutral background regardless of the
lifecycles it counts. The stem label is muted at reduced opacity;
unworked handles are muted, dormant ones weaker. A handle a lane is working
names that lane in its text, the lane accent mixed 55% toward `--ink`, and in a
5% tint of the accent mixed into `--control`; unworked targets tint in
`--muted`. In-flight handles are not dimmed, because text and tint carry
ownership. Every target rises to a 10% tint on hover. Labels sit 10px in from
either rail: the stem starts and the last handle ends with 10px, and every
other join has 8px on each side. An 8px outer inset keeps the team flags off
the browser edge, and adjacent tabs in either strip are separated by the
message stream's inter-lane `--mosaic-gap`; team flags never join the status
rule below. The team total's sole badge counts every task the band shows, which
is the sum of the stem badges and the number of handles. Activating it opens
the unified overview with the team's complete current task set already
selected. It does not change the durable location.

Flags follow Work's clustering rule: from in-flight, ready, blocked, and
deferred offer order, each visibility-qualified stem's first task places one
`task-pill--project` flag, and later matches join that flag without changing
their relative order. Its `task-pill-stem-target` button displays the raw stem
in the muted blocked tone with a `task-pill-count` badge of the stem's open
tasks, and opens the same overview with that team's matching stem selected.
The following `task-pill-handle` buttons open the exact task. Closing any
overview returns focus to the control that opened it. Flag overflow scrolls
inside its band without entering the status/pip row.

The menu's unchanged Work action and the global total open every current task;
global stem controls open their complete visibility-qualified stems. Team
membership is the team of the lane actively claiming a task, or no team when
that lane is unteamed; otherwise its current creator team; otherwise its last
claimant's team. Project controls select the exact full dotted project, and a
relationship control extends the current selected set with that task. Selection
always follows the resident's admitted offer order and may be explicitly empty.
The overview contains the complete selected graph and task details plus the
resident's running or awaiting-delivery jobs.

Only Dash and Chat are durable routes. Work, task, and direction controls are
browser-local overview actions: they neither create history entries nor claim a
restorable URL. A stale former Work or Directions URL normalizes to Chat under
the ordinary unavailable-location notice.

The composer columns, speech and allocation sliders, and send controls retain
their dimensions. Removing the standalone ticker row gives that block height
to the message stream instead of resizing the controls. Wide, narrow,
short-height, empty, one-member, and multi-member consoles keep a usable header,
composer, and stream without a new nested vertical scroller.

## Card endings and relative ages

Invisible renderer portal hosts do not add trailing space. A card whose final
visible Markdown block precedes that host therefore keeps the same visual
bottom inset as its sides.

A final line of text rests on the card's content box, like every other ending.
The body font's descent below the baseline (about 3px at the card's 16px body)
is the extra depth its bottom inset shows, so the line is not moved lower to
hide it. The card's faces clip at its inner bottom edge rather than at the
content box, so that line's descenders are never cut.

A signed prose card keeps its agent attribution in the lane accent, but its
linked top-level relative age is neutral. The age remains an underlined card
permalink and advances on its own visible-unit deadlines. Authored Markdown
links retain the ordinary accent, and quoted-source ages keep their separate
quote presentation. Arriving at a card through its permalink outlines it in the
accent for about a second and a half, then the outline fades out over a little
more than a second; each new target replays it. Without motion the outline
clears at once at the same moment. The route focuses the card without a
visible focus ring, so nothing replaces the faded outline; keyboard navigation
continues from the card, and hover and focus apply as usual.

FINAL is producer metadata rather than an action: it follows the producer name
in the same compact, translucent accent pill used for task origins inside
quoted attributions, which follow their author's name the same way. On a card
without a leading attribution, FINAL sits immediately before the producer name
in the controls island.

## Profile app latency in Firefox

The app exposes opt-in User Timing diagnostics without changing route parsing.
Open the ordinary app URL, run
`sessionStorage.setItem("spice:app-trace", "1"); location.reload()` in its
developer console, and leave that tab open for the capture. A traced page
installs `window.__spiceAppTrace`; an ordinary page does not attach the
native input listeners or create timing entries.

Build the current browser body with `spice dev xtask -- app` and serve that
generation through the development resident. Then capture two short runs of the
same interaction:

1. Open `about:profiling` in Firefox, select the **Web Developer** preset, and
   start the profiler.
2. In the traced app tab, run `window.__spiceAppTrace.clear()` in the
   developer console immediately before the scenario.
3. Perform one fixed scenario: use the same loaded board, focus the same
   composer, enter or dictate the same text, and trigger the same resident
   update. Keep the capture short enough that the entire interaction is easy to
   inspect.
4. For the loaded comparison, start
   `spice dev cargo -- check --locked -p spice` in another terminal and perform
   the scenario while that real compile is active. Stop and save the profile.
5. Wait for the command and all compiler processes to exit. Clear the app
   trace, repeat the identical scenario without compilation, then stop and save
   the settled profile.
6. In each Firefox profile, use the Marker Chart and filter for
   `spice.app.`. Compare those markers on the app content main thread
   against DOM events, task scheduling gaps, style recalculation, layout, and
   paint. Retain the profile URLs or exported profiles with the investigation
   notes.

Marker names carry only bounded structural metadata:

| Marker | Interpretation |
| --- | --- |
| `trace.start` | App source/build identity for the capture. |
| `frame.received`, `frame.decode.*` | Incoming frame size and browser-side decode/admission work. |
| `state.apply.*`, `state.publish` | Runtime mutation and the Yew publication that schedules rendering. |
| `render.app` | Rust/Yew virtual-DOM construction with the retained card count. |
| `layout.measure` | Mosaic DOM measurement, including card count and measured width. |
| `composer.textarea.mount`, `composer.textarea.unmount` | Native textarea instance lifetime, focus, and selection. A changed instance during one interaction is a remount. |
| `composer.input`, `composer.keydown` | Native event kind, composition state, focus, and selection without draft text. |

The browser retains at most 256 app diagnostic records; completed spans use
one measure and two marks, and the oldest record is removed as the bound rolls.
`window.__spiceAppTrace.clear()` removes only entries owned by the current
app trace. Run
`sessionStorage.removeItem("spice:app-trace"); location.reload()` to disable
collection.

Interpret ordering conservatively. `render.app` ends when Rust finishes
building Yew nodes, not when the DOM is committed or painted.
`layout.measure` covers synchronous Mosaic DOM reads and writes, but a later
paint must be established from Firefox's own markers. A DOM `beforeinput` or
`input` marker is not the start of speech recognition. The trace intentionally
has no animation-frame marker: an animation-frame callback would show callback
eligibility, not presentation.
