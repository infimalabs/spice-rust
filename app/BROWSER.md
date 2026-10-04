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
usage bar, amount left, and reset. The amount stays immediately beside the
bar it describes rather than separating the name from that bar.

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

The speech and allocation sliders share a fixed frame above the team send
button, and the frame carries no words but the button's. Its top edge sits one
label row below the composer block's, and the two current values ride above
it, leaning on the center divider: speech ends and allocation starts 3px from
it, so they read as one phrase such as "quiet drain" or "narrate steer". The
label row comes out of the ranges' region; the composer block, the frame width
and the send row keep their size.
At the 10rem minimum member width, the standard draft block is 170px tall and
reads approximately square without widening the member column.
That shared block size also determines the speech and allocation tracks, their
frame, and the send row; the measured overlay height remains the Chat plane's
collapse distance and top spacer.

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
minutes. Then come the Wake, Drain, Remove, Renew, and next-launch-model
commands and refusals; Renew submits immediately, while Drain and Remove
confirm first. Presence and lease age, the current task, latest activity,
waiting direction, copyable agent channel, occupant, session, and worktree
follow, beside Create new team and Leave all teams. Wide lanes lay those groups
side by side; narrow lanes stack them and scroll within the lane. The button,
Escape, or a click elsewhere in the team restores the composer.

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

Card actions form a shrink-wrapped island at the content's top inline-end.
When the first content block is a colored quote, the app portals that island
into the quote before its content. The quote surface itself starts at the card
edge and keeps the full content width; only its text wraps beside the controls,
and image rows clear them from inside the same colored surface. Cards without a
leading quote keep the island in the ordinary message flow. Narrow cards retain
the same quote-local ownership rather than adding an external controls header.

Committed message cards form one gapless ledger: the lattice has no row,
column, or decorative edge gutter. Square card edges meet directly, with one
soft block-start rule between vertical neighbors and the owning card's
attribution-colored inline-end edge as the single horizontal seam. Measurement
clones use the same zero-gap geometry, while compaction and lifecycle dividers
remain semantic full-width rows.

Quoted image pairs retain the standard 7px image-stack gap even when their
leading quote stays flow-root around its control island: flattened stack
wrappers apply that shared gap at the boundary before the later image.

A quote layer that ends in images after other content flows its prose around
them. The images sit at the layer's inline start with their bottom on the
attribution line, and the attribution stays the layer's last line at
inline-end. When the text is shorter than the images, it sits beside their
top and the attribution drops to their bottom. The browser measures that lift
because the wrap depends on where the images sit. A layer that would leave
less than a 14ch column beside the images stacks them under the prose
instead, wrapping in pairs, with the attribution below them. Image-only
quotes and images in the middle of a quote keep the image-row layout.

## Always-chat team console

Every populated or empty team surface keeps its Chat composer and message plane
mounted continuously. It has no Work/Chat rail, local Work pane, mode-owned
scroll state, or return-to-mode behavior. The toolbar orders the status-pip
stack, complete live ticker, and hamburger. The ticker keeps its relative age,
lane-status class, agent/tool/command prefix and accent, truncated detail,
accessible label, and live-region semantics in the middle header slot; an empty
fleet reserves that slot without inventing status.

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
only the pane below carries the panel. Pip and hamburger columns span a task
band sized to the framed global pill strip above the status band. The 44px
hamburger chamber keeps its rounded crown but has square lower corners and no
gap above the pane, fusing it to the team surface. Its outer edge follows the
global Spice circle, and its size can be mirrored at the left while containing
all six member pips. The global Spice and Dash-avatar circles share one diameter
and equal top, bottom, and outer-edge insets. The team's task strip reuses the
global `task-pill-strip app-control-rail` frame. Its total pill
(`data-work-scope="team"`) carries one open-task count; each `task-pill--task`
holds two buttons, `task-pill-project` and `task-pill-handle`, sharing the
pill's tone through `active`, `dormant`, and `system`. The handle opens task
detail through the same path as a Chat task chip, so focus returns to it. The
ticker centers on the lower pip row; task overflow scrolls inside its band.

The hamburger opens canonical global links for the team:
`#/work/team/{team}` and `#/work/directions/team/{team}`. The team Work scope
admits each task created by the current team or last claimed by one of its
current member lanes, exactly once. Its global task history uses the same team
filter and restarts at the newest page when scope changes. There is no
team-local Settled list or Kept jobs table; Running jobs, open work, and task
history remain the global Work surfaces.

Directions is likewise global. Its team link selects that team's filter while
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
bottom inset as its sides, including when its producer and age share the final
prose line.

A final line of text rests on the card's content box, like every other ending.
The body font's descent below the baseline (about 3px at the card's 16px body)
is the extra depth its bottom inset shows, so the line is not moved lower to
hide it. The card's faces clip at its inner bottom edge rather than at the
content box, so that line's descenders and a signoff's underlined age are never
cut.

A signed prose card keeps its agent attribution in the lane accent, but its
linked top-level relative age is neutral. The age remains an underlined card
permalink and advances on its own visible-unit deadlines. Authored Markdown
links retain the ordinary accent, and quoted-source ages keep their separate
quote presentation.

A signoff keeps its relative age at the right edge, immediately after the
producer name; any badge leads that pair. A FINAL classification therefore
precedes the producer name, in the same compact, translucent accent pill used
for task origins inside quoted signoffs, which lead their name and age the same
way. On a card without a signoff, FINAL sits immediately before the producer
name. It remains producer metadata rather than occupying the card-action island.

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
