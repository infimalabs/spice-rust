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

## Glass control-panel design gallery

The standalone [glass control studies](design/glass-controls.html) are a
reference gallery for the 80 × 160 composer control panel, against dark and
light backgrounds. They are interactive local mockups, not resident controls,
and editing them changes no production style. Browser screenshots of the
gallery can be retained as task artifacts without adding generated images to
the application sources.

Flush etched glass is the shipped primary panel described under [Team composer
controls](CONSOLE.md#team-composer-controls); the gallery reproduces its
geometry, 3px recipient rim, and contact depression. Smoked pearl and Frosted
instrument are reference-only finishes on the same geometry, not the current
selection. Every finish keeps sharp panel corners, circular BASIC/TURBO, menu,
and SEND controls, and two pill-shaped slider flyers labeled Quiet / Speak /
Narrate and Steer / Drive / Drain.

SEND uses one perimeter rim, without nested optical ridges. Its face rests
flush and moves down with an inset upper contact shadow during native pointer
or keyboard activation; release restores the resting surface. A released/held
comparison freezes both views for each finish.
The rim-light selector previews neutral, mint, amber, or lilac agent accents
without filling the glass faces with the accent color.
The readiness selector applies off, running-without-text, one-recipient, or
multiple-recipient states to the material previews. A separate complete-panel
matrix compares all four states, defaulting to the shipped finish, with
selectable finish and background. SEND is disabled in the first two states;
ready states carry explicit recipient counts and single or segmented accent
rims.

Serve `app/design` with a local static server, then open `glass-controls.html`.
The actual-size toggle shows the existing footprint; the detail toggle doubles
it for inspection. Both sliders, BASIC/TURBO, menu preview, and SEND preview
operate locally without sending commands.

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

## Resident topology in the global menu

The Spice view opens with one heading-free control row: current connection state
and the inspectable product version stack at the left, and global TURBO remains
at the right. An available app generation turns the version into the explicit
reload control. Epoch and stream cursor stay out of this primary row. The
read-only Git identity section follows.

Resident topology is one interactive roster. Each team contains compact agent
cards with presence, pending steering, liveness, held task, kept jobs, model,
and provider emblem. A teamed agent's card opens its composer in Chat; the same
card is draggable when team configuration is admitted.

Drop a card onto another team to mix members, onto **New Team** to create a
team, or onto **Unassigned Agents** to separate it from its team. Unassigned
agents use the same cards without a Chat link. Every drop uses the topology
revision currently shown, and unavailable configuration disables dragging
without disabling existing Chat links. Cards use two columns where the menu
has room and one column in narrow layouts.

## Provider quota rows

The **Accounts lacking usage** and **Disabled credentials** sections appear
only when their respective collections contain entries. Empty collections
omit the entire section, including its heading; account limits and quota
status remain visible independently.

Each account-limit row reads from left to right as the limit name, flexible
capacity bar, amount, and compact reset cadence. Percent amounts describe the
remaining quota as `N% full`; counted limits retain their explicit remaining
and total units. The filled capacity starts at the bar's left edge, so
consumption exposes the track from the right. The amount and reset stay beside
the bar they describe; only the limit name may wrap when the sidebar is narrow.

An account with a shared weekly limit keeps the same capacity-order accent in
its card surface, edge, provider label, and usage bars as its layer in the
Spice-button quota fill. Reports that do not contribute a weekly layer remain
neutral.

The reset-credit summary always states both available and redeemable counts;
when the provider omits the latter, the available count supplies it. Each
redeemable credit owns a full-width line. Its provider-authored title, program,
and remaining count lead when present; `Granted` and a live relative age follow,
with the exact grant timestamp on hover. An absent or invalid grant time reads
`Granted at unknown time`. A valid expiry stays right-aligned as an `expires`
relative countdown with exact-time hover detail.

A provider-reported raw balance appears on its account card as whole credits,
truncated so a fraction never reads as more than can be spent and grouped by
the browser's locale. It sits at the right end of the **Reset credits** line,
aligned with the reset expiries, or on its own **Credits** line when the
account has no reset credits. It comes from the OMP usage limit whose scope
identifies the `balance` window, has no window, and reports only a remaining
`credits` amount, so it is neither a time-window meter nor a reset grant, and
accounts never share or aggregate it. An account without that limit shows no
balance; Spice does not infer zero from limits or read provider credentials to
fill it.

## Arrange whole teams

Pick up a team from the pip matrix at the physical left of its status bar.
After the same six-pixel lift threshold used by composer movement, the source
surface stays reserved and a bounded ghost follows the pointer. The middle
three fifths of another team exchanges exactly those two team positions. Either
outer fifth joins into the team under the pointer: the receiver keeps its
identity, primary composer, and member order, then receives the source members
in their existing order.

The status-pip stack and the team menu both open the same swap/join choices.
Opening either moves focus to the first available choice; Escape closes the
choices and returns focus to whichever of the two opened them.

A team is named by its lead agent, canonical member zero, using the same label
its messages carry: the configured name, else the worktree leaf. The task
overview heading, arrangement targets, team menu, and team request toasts all
use that name; only a team with no current members keeps its short id.

Sibling teams share one faint internal rule through the message plane. It
starts below the composer block and its painted continuation, so it never runs
beside or above the controls. A single team has no rule, and the fleet never
paints one against either browser edge.

Interactive joins stop at six members. A seventh-member edge target is shown
as unavailable and sends no command. Press Enter or Space on the pip stack, or
tap it, for the equivalent **Swap positions** and **Join into** menu. Every
accepted interaction sends the complete topology at the revision currently
drawn by the app; cancellation, a changed revision, or a removed source clears
the lift without sending.

## Team status line

The activity age keeps its fixed three-character measure and meets the main
status with only the status line's ordinary inline spacing. It adds no trailing
margin of its own. The status line owns the complete second header row.
The status bar starts with a fixed leading segment centered beneath the team
total. Its 8px presence pips center as one dot, a vertical pair, a three-dot
triangle, a four-dot square, a centered two-over-three group, or a three-by-two
matrix for one through six members. Within the rectangular forms canonical
members continue bottom-to-top and right-to-left. The ticker uses the remaining
width; an empty team has no pip segment. The full row shares one surface and top
rule, while only status text dims for bound/unbound state.

## Work tabs

Global and team totals use four-pixel outer inline rails; global stems and team
stem containers use the thinner ordinary tab rails. Global and team totals share
one tab width sized for a one-digit badge; a wider count takes its room from the
badge's own inline padding with the digits centered, so the badge, both totals,
and their alignment never change with the count.
Presence pips live in an equally wide leading segment of the status row directly
beneath the team total, not a trailing task tab. Task handles remain borderless segments
inside their stem container; the final handle meets the container's single
right rail. The task scroller reserves the total's fixed width and uses mirrored
fade, padding, and scroll-padding boundaries at its ends. Counts and navigation
remain independent of the status matrix.
Global tab rails use the neutral line palette in every theme, including on
hover. Team totals, stem rollups, and unassigned task handles remain neutral.
An individually assigned team-local task handle retains its prior accent
treatment: its current claimant tints the text and face, hover strengthens that
face and may color its rails, and keyboard focus retains its visible ring.

The task overview's Dependency graph points from each prerequisite to every
dependent it blocks and labels that edge **then**, so the source card, arrow,
and target card read as one activity transition. Prerequisite and dependent
controls still filter by their named task relationship before the graph presents
that relationship in blocking order.

## Lane menu

Each Chat lane's menu button swaps that lane's composer for its seat panel
instead of opening a floating menu. The panel carries everything one agent
offers. It leads with what the agent needs from the operator now: consent
requests to Allow or Deny in place, held tasks iced, blocked, or in review,
a lease within two minutes of expiry, and direction waiting over five
minutes. Then come the Wake, Drain, Remove, Renew, and next-launch model,
reasoning effort, and fast mode commands and refusals; Renew submits
immediately, while Drain and Remove confirm first. Fast mode is one on/off
toggle right after effort, tinted like the Spice sidebar's Fast mode when
on. It shows and flips the seat's own flag. Fast mode is additive, so the
next launch also runs fast while the team's Turbo or the global setting is on,
and the status names which level enables it. Effort and fast mode each report
the running value beside the next launch, and a running OMP agent takes either
change at once, so its composer icon gains or drops the fast badge once omp
confirms. Presence and lease age,
the current task, latest activity, waiting direction, copyable agent channel,
occupant, session, and worktree follow, beside Create new team and Leave all
teams. Wide lanes lay those groups side by side; narrow lanes stack them and
scroll within the lane. The button, Escape, or a click elsewhere in the team
restores the composer.

Lane hamburger buttons reuse the composer's Flush glass face, neutral edge,
hover wash and contact shadow, retaining their 22px footprint. Open menus use a
neutral wash; keyboard focus remains explicit. Pending renewal keeps its small
recipient-colored status dot without turning the button rim into an accent light.

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
marks use distinct SVG gradients. Spice-sidebar agent tiles keep their full-size
provider emblem.

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
island moves rightward only: the renderer records exact raw/authenticated quote
depth as trusted metadata, and CSS uses it to dock the island against the card
edge at arbitrary nesting depth without inline styles or finite depth
selectors. It never moves upward, so it starts level with its hunk's first
line and cannot reach text it would not reflow, such as an inner quote's last
line or task badges. Every pair wears the same edges wherever it stands. A
zero-width two-line prop keeps associated image rows below the controls.
Each hunk directly owned by the message body starts one quote-block padding
unit lower, matching the quote control cadence. A hunk nested under a quote
card does not inherit that outer margin; its quote shell owns the spacing.
A top-level hunk immediately following a blockquote or authenticated quote card
uses two quote-block units (8px) of leading margin instead of the ordinary 4px.
This adds 4px at the quote-to-answer boundary without changing nested quote
padding, ordinary adjacent-hunk spacing, or the action island's own footprint.

Quote and Play form one 35px control pair with one internal seam and no outer
right edge. Their fill follows the owning lane or quote accent, with neutral
glyphs and paint-only hover, press, speaking, and reduced-motion states.
Playback belongs to one exact hunk: automatic speech uses direct prose, quoted
sources retain independent Play controls, and keyed portal roots preserve each
hunk’s callback. Narrow cards keep this order rather than adding a controls
header.

Task-handle chips use the surrounding rendered activity prose, without the
handle itself, as a bounded native hover title. Authored characters are escaped
before entering the attribute. An isolated handle has the explicit `Open task
HANDLE` fallback; activation, keyboard semantics, and the visible chip remain
unchanged.

Committed cards occupy a fixed 9px lattice perimeter and shared seams carved
from the admitted rectangle. Prose cards center their painted height within
whole-row cells; every card rests in a 5% lane-accent wash and hover raises it
to 10%, while retaining its lane rail and matching rail-radius corners.
Measurement clones use the same geometry.

A single-image message uses its card as an edge-to-edge cover. Focus paints one
card-sized boundary, and the rest and hover washes use whole-cover screens;
attribution stays in a compact top-left frost while actions remain top-right.
Multi-image or mixed-content hunks instead use one inline-start row with
stable preferred sizes; prose wraps beside it and narrow rows shrink images evenly.

## Attribution placement

Every attributable run between nested quote blocks is one speaker-owned hunk.
The hunk starts with that source's control island, relative age, name, and one
segmented classification pill. The pill orders ACK or NAK first, FINAL second
when present, then the BROADCAST role; each segment keeps its own
authenticated color and interaction. Ordinary prose begins immediately after
that compact attribution sequence on the same line.
Acknowledgment is one authenticated message disposition, not a source-text
marker. Every direct hunk owned by the answering speaker mounts the same ACK or
NAK segment in its signoff, including each resumed hunk around nested quotes;
quote-owned hunks do not inherit it. Pending settlement revises those repeated
segments on the existing card without adding prose or changing hunk geometry.
Literal `[ACK]`, `[ACK PENDING]`, and `[NAK]` text remains ordinary Markdown
when no authenticated directive owns it.
TASTE and REVIEW belong to the directive the resident sent, not to the card
answering it. A quoted directive context whose typed provenance is a taste
reminder or review feedback wears exactly one such pill right after its own
source age and name, where its words begin. Literal `[TASTE]` or `[REVIEW]`
in any prose remains ordinary Markdown.

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
