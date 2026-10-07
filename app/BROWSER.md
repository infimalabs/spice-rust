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
speech and allocation vertical ranges below it. Its label stays “SEND”; color
and the adjacent allocation value distinguish Steer, Drive, and Drain. Hover,
press, reduced-motion, ordinary-submit, and broadcast-preview states change
paint only, never geometry.

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
boundary remains continuous. Equal soft elevation
appears on both inline sides; at a team boundary the following composer carries
the otherwise-clipped right cast, letting either neighbor read beneath the
raised controls without moving either surface. Sibling teams need no hard
separator because their glass, accents, and content already distinguish them.
The screen owns the form glass, one complex theme-derived background, and one
10px frost filter. Its header edge eases from an opaque control line through
stronger control color
into ordinary glass. The form has no physical lower edge or strengthening band;
the ordinary veil simply continues 10px beyond it while a mask fades that
continuation to transparent. No backdrop-filter boundary exists at the form
edge. The static continuation remains after the form fully collapses, preventing
a final visual snap. The shared control edge remains opaque and theme-derived
for other control consumers. Member headings retain
their accent underline, focused or broadcast drafts merge their full-width
highlight, and vacant members mute content without changing the surface.
The composer rides in the Chat plane's own scroll content, so every engine's
scroll moves it in the same frame as the cards. It sticks to the plane's top
edge, so scrolling back never opens a gap above it, and measured composer
height remains both the collapse distance and the message-plane spacer. Between
exposure edges, one fixed absolute rest lets native scroll carry the composer.
At either edge sticky positioning owns continued motion without repeated layout
writes; the first reversed delta reanchors the rest once, so reveal or collapse
begins immediately. Programmatic focus moves
the caret without browser focus scrolling the viewport, team fleet, member
strip, or message plane. The composer's existing focus transition still reveals
a collapsed draft fully, and the plane's scroll padding keeps other
browser-scrolled targets clear of the exposed composer only while focus is
outside it.

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
control keeps the same compact geometry in both primary and quoted composer
shards. The touch-only image chooser is no thumbnail: its plus sits in the lane
menu's transparent circle frame and shares that control's accent states.

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
or finite depth selectors. A top-level island rises half a line into the card
inset, except directly after a nested quote block: there it starts level with
its hunk and only floats aside, so it never reaches the quote's last line or
its task badges. A zero-width two-line prop keeps associated image rows below
the controls.

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
