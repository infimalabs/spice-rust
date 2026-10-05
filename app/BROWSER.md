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

The tab and menu use the predecessor's brand: the short lowercase name `spice`,
placed last. The Wasm app carries no separate product name in
user-facing text, which matters in a tab where only a few characters show. The
predecessor Python Spice (`spice.git`, `spice/serve/web.py` and
`spice/serve/static/app.audio.js`) had three branding conventions, and each is
decided here:

- Its title and menu label came from `pyproject.toml` `[project] name`, else
  `[serve] brand` (default `spice`). Adopted: the `spice` default as the brand.
  The per-repository override is not carried over; this repository has no
  brand configuration key.
- It shipped a static raster set: `favicon.ico`, 16/32px PNGs,
  `apple-touch-icon`, and `site.webmanifest`. Rejected: one generated SVG icon
  carries the live count, which a static file cannot. spice is a loopback
  operator tab, not a home-screen install. Until the Wasm app's first branding
  effect runs, the tab shows the browser's default icon.
- During speech playback it retitled the tab with the speaking agent first
  (`main-b - spice`). The shape is adopted: the changing state leads and the
  brand trails after ` - `. The speech trigger is not; the title holds the
  remaining-task count and stays stable instead of flipping with each
  utterance, while the app marks the speaking message card `now-playing`.

Titles read `5 tasks - spice`, `1 task - spice`, `7 tasks (stale) - spice`,
`Loading - spice`, and `Disconnected - spice`; the static page title is
`spice`. These supersede earlier title spellings.

Once the first board snapshot arrives, the title gives the exact number of
remaining public tasks and the icon wears the same count. The pepper keeps the
icon's upper-left corner, and the badge fills the lower right with digits large
enough to read at the 16px tab size. The badge shows exact values through 99
and bounds larger labels at `99+`; the title remains exact. Title and icon are
rewritten only when this state changes. A zero badge is explicit. Loading has
an ellipsis, and disconnection without a retained board has an exclamation
mark. During reconnect, the last known count stays visible in amber and the
title marks it stale; a current snapshot restores the green badge.

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

The full-width team send button sits above the speech and allocation sliders,
whose vertical tracks follow side by side. The two current values occupy the
row immediately below the tracks, leaning on the center divider: speech ends
and allocation starts 3px from it, so they read as one phrase such as "quiet
drain" or "narrate steer". The control field owns one top edge, one seam below
the send button, and one center divider between the sliders; slider state uses
fill without adding nested borders or inset shadows. The frame carries no words
but the button's. Swapping these two fixed units does not change the composer
block, frame width, send row, label row, or slider-track size.
At the 10rem minimum member width, the standard draft block is 170px tall and
reads approximately square without widening the member column.
That shared block size also determines the speech and allocation tracks, their
frame, and the send row; the measured overlay height remains the Chat plane's
collapse distance and top spacer.

The member columns and that control frame are one joined field. Their measured
layout gap is zero, and neither adjacent members nor the member-to-control
junction paint a vertical separator, shadow, or panel-colored gutter. The team
owns the field's left outer edge and the controls own its right outer edge.
Every member heading has a permanent
accent-colored underline and adjacent accents meet without a panel-colored
break. A focused member composer is lit: it gains the same accent underline at
its block-end edge, and the focused draft itself, the primary or a quote draft
pinned to a message, mixes its background 10% toward that accent. Keyboard
broadcast preview lights every occupied recipient through the same single lit
state, filling each recipient's primary draft. Neither treatment changes the
field's geometry, and circular menu
buttons, range thumbs, and the send action retain their own control shapes.

The values sit outside the frame rather than riding the thumbs. A seven-letter
word is more than twice a native thumb's width, a range thumb cannot carry text
in either engine, and an overlay tracking the thumb would cover the track and
its focus ring. Above the frame nothing clips the phrase: "narrate", the longest
speech value at the label's 9px, overhangs the frame's left edge by a few
pixels and stays clear of the draft.

Both sliders take one stop per arrow press on every engine: Up and Right
raise, Down and Left lower. A native vertical range in WebKit and Firefox
raises on Left, so the app handles Left and Right itself rather than inheriting
each engine's mapping.

When no agent in the team is running and none of its composers holds text, a
quote, or an image, the frame steps back like an inactive lane: the sliders and
send button dim and desaturate, and send is disabled because there is nothing to
send. The sliders stay usable. Drafting anything, or any agent running, lights
them again. Typed text lives in each composer element, so the frame alone
follows it: every composer change, including a value the app writes on mount,
send, or restore, raises a bubbling `input` event at the team form, and only the
frame re-renders, only when the answer flips. A keystroke never renders the
app.

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
primary composer's lower-right corner. It occupies no layout space, accepts no
pointer events, and sits behind the transparent textarea so selection, typing,
and pasted images retain the entire composer. Provider identity remains
available through the emblem's accessible label. Quote shards carry no
watermark, and Dash agent tiles keep their full-size provider emblem.

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

Every attributable hunk owns a shrink-wrapped action island at its top
inline-end. The island floats; attribution and ordinary prose remain in the
same inline flow beside it. Only the hunk's associated image row clears the
island before floating inline-start. Narrow cards preserve the same ordering
instead of adding a separate controls header.

Committed message cards sit in one lattice whose 9px perimeter and row and
column seams equal the card's 9px content inset. The seam is one shared gap
between neighbors, never two adjoining margins. The lattice carves its
perimeter and every seam from the admitted rectangle; it never grows to make
room. Every cell remains a whole number of rows. A
prose card paints only its content height, centered vertically in that cell, so
the cell's slack falls evenly above and below it and the visible space between
cards follows their content while every cell still snaps to the grid. A card
whose content fills its cell leaves the same seam around its neighbors; image
covers still fill the card rectangle. The complete stream field uses one
theme-aware surface halfway between the absolute page and the standard composer
surface. Cards paint no fill of their own, so that field shows through them;
they remain distinguished by their lane's 4px accent rail, square corners, and
the gutters between them.
Nested Markdown blockquotes and resolved or unavailable quote cards are square
at every depth as well. A
FINAL answer keeps only its pill, with no tint or edge of its own. Hover tints
the card 4% toward its lane accent and draws no outline or border, on every
card type. Measurement clones use the same guttered geometry. Compaction and
lifecycle dividers, such as a fresh session, remain semantic full-width rows
drawn as accent rules.

An image-only message with exactly one non-diagram image uses its whole
committed card as an edge-to-edge cover viewport. Measurement clones reserve
that cover from the card width, but the committed lattice remains the placement
authority so no aspect-ratio correction opens a ledger gap. Keyboard focus
paints one square card-sized border above the cover; its inline-start rail and
remaining edge meet as one boundary, never nested outlines, and the image cannot
clip or wash out that edge. Hover draws no border: the same card-sized layer
screens the whole cover 16% toward the lane accent instead, since the image
hides any background wash. The live age and
producer sit in a compact frosted overlay docked flush into the image card's
square top-left corner. The overlay carries no corner curve, so its edges meet
the card without exposing a sliver of image. It pads its text 3px above, 6px
below, and 9px on each side. Actions, when present, remain in their own inset
overlay at the top right; multiple images, mixed prose, quotes, and diagrams
keep their
ordinary in-flow layouts.

Each hunk consolidates its associated images into one inline-start row after
attribution. The row alone clears inline-end controls; prose then wraps beside
the images using ordinary CSS flow-root geometry. Multi-image rows retain the
standard 7px gap and pair sizing at narrow widths.

## Attribution placement

Every attributable run between nested quote blocks is one speaker-owned hunk.
The hunk starts with that source's control island, relative age, name, and one
segmented classification pill. The pill orders ACK or NAK first, FINAL second
when present, then TASTE, REVIEW, and BROADCAST roles; each segment keeps its
own authenticated color and interaction. Ordinary prose begins immediately
after that compact attribution sequence on the same line.

All images associated with the hunk are consolidated next, floated at the
inline start, and followed by the remaining prose. A nested quote interrupts
the hunk; direct text after it starts a new hunk with the surrounding source's
controls and attribution again. This one rule applies recursively to top-level
agent prose and resolved operator, agent, and peer-lane quotes.

The control island floats at the inline end of the hunk. Its speak action uses
only that hunk's rendered text, and its quote action retains that hunk's exact
authenticated target. Authenticated marker nodes remain in the Markdown DOM
for quote semantics but do not duplicate the classification pill in prose.

Board provenance remains independent and terminal. Each complete quote layer
keeps one empty task target after all of its hunks and nested quotes; Work fills
that target with origin-task badges. Repeating source attribution around a
nested quote never repeats or relocates the quote layer's task provenance.

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
scroll state, or return-to-mode behavior. The toolbar has no hamburger: its
task strip and complete live ticker fill the width left of the status-pip
stack, which holds the right edge. The ticker keeps its relative age,
lane-status class, agent/tool/command prefix and accent, truncated detail,
accessible label, and live-region semantics in the status band; an empty fleet
reserves that slot without inventing status.

Chat itself is never unmounted. Every other destination leaves the inactive
agents workspace inert, `aria-hidden`, out of flow, and at
`content-visibility: hidden` rather than `display: none`. It stays fixed below
the commandbar with the same padding and frame edge it has when shown. It skips
rendering but keeps that layout, so cards that arrive meanwhile are still
measured, scroll positions hold, and returning to Chat reuses the layout without
any plane, card, or team surface observing a resize. Browsers without
`content-visibility` fall back to `display: none`.

The toolbar is a fixed-height grid whether or not the team owns work, with no
background, border, or shadow; the team surface frames nothing around it, so
only the pane below carries the panel. The pip column spans the task band,
sized to the framed global pill strip, and the status band below it. The status
band has the same height and type size as a draft composer heading, which shows
the same agent activity. The header holds all six member pips in two rows, the
main agent at the lower
right. The global Spice and Dash-avatar circles share one diameter
and equal top, bottom, and outer-edge insets. The team's task strip reuses the
global `task-pill-strip app-control-rail` frame. Its total pill
(`data-work-scope="team"`) carries one open-task count. The remaining rail uses
Work's clustering rule: from in-flight, ready, blocked, and deferred offer
order, each visibility-qualified stem's first task places one
`task-pill--project`, and later matches join that group without changing their
relative order. Its one `task-pill-stem` button displays the raw stem, opens the
team's Work scope narrowed to it, wears the muted blocked tone, and centers its
label in equal inline hover space. The following `task-pill-handle` buttons
open each task's detail and retain their own lifecycle or current-owner tone;
closing detail returns focus to that handle. The ticker centers on the lower
pip row; task overflow scrolls inside its band.

The team's total pill opens the canonical global `#/work/team/{team}` view, and
`#/work/directions/team/{team}` addresses that team's Directions. The team Work
scope admits each task created by the current team or last claimed by one of
its current member lanes, exactly once. Its global task history uses the same
team filter and restarts at the newest page when scope changes. There is no
team-local Settled list or Kept jobs table; Running jobs, open work, and task
history remain the global Work surfaces.

Directions is likewise global. Its team route selects that team's filter while
search text, recipient, lifecycle state, and authored-before filters constrain
the resident read before pagination. Newer and Older replace one bounded,
newest-first page through journal cursors without splitting a fanout. A focused
direction route clears filters and loads the page beginning at that card.
Direction cards use the shared sanitized Markdown, image, task-detail, and Chat
return-link surfaces; there is no team-local direction ledger or
`#/chat/direction/...` route.

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
