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

Flush etched glass is the shipped primary panel described under
[Team composer controls](#team-composer-controls); the gallery reproduces its
geometry, 2px recipient rim, and contact depression. Smoked pearl and Frosted
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

Pick up a team from its status-pip stack. After the same six-pixel lift
threshold used by composer movement, the source surface stays reserved and a
bounded ghost follows the pointer. The middle three fifths of another team
exchanges exactly those two team positions. Either outer fifth joins into the
team under the pointer: the receiver keeps its identity, primary composer, and
member order, then receives the source members in their existing order.

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
margin of its own.

## Work tabs

Global totals and stems, plus team totals and stem containers, use the same
four-pixel outer inline rails. Task handles remain borderless segments inside
their stem container; the final handle meets the container's single right rail.
The rows keep their existing shapes, spacing, scrolling, counts, and navigation.

## Team composer controls

The primary panel uses Flush etched glass in a sharp-cornered 80 × 160px frame.
Round 34px TURBO and team-menu buttons share the first row; round 56px SEND
sits in the middle, above two pill-shaped slider flyers. Faces transmit the
neutral background, with fine etched edges and no opaque recipient-color fill,
domes, or nested ridges. Team and Settings global TURBO retain the same lettering:
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
lockup exactly when the active submission reaches every member. `FULL` uses the
focus color, and both words compress into the ordinary `SEND` label's width.
Ordinary submission derives that state from the reachable members whose
composers hold text, quotes, or images, including wakeable non-running members.
While a broadcast preview is active, its actual broadcast recipients replace
that calculation; broadcast mode alone never implies full coverage. Partial
sends and single-member teams show only `SEND`. The lockup stays inside the
existing button geometry and does not change its accessible name.

Pointer and keyboard contact move SEND down 1.5px, deepen its upper contact
shadow, and dim the rim; release restores its flat face without grid reflow.
Touch-only contact is projected from native pointer events and clears on release,
cancellation, or lost capture without intercepting activation. Reduced motion
removes transitions, not contact feedback. Held ordinary-submit modifiers add a
focused-agent command ring; a held broadcast uses its occupied recipients' rim
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
