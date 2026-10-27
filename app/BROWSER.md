# Browser checks

## Browser evidence follows the host system theme

Use the repository-owned `launchAppBrowser` helper for disposable app browser
checks. It gives the headless context the host's `prefers-color-scheme`, so
callers do not repeat theme setup:

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

Playwright starts Chromium with `--use-mock-keychain --password-store=basic`,
so these checks never read the macOS login Keychain. A lane shell that starts
a Chromium executable directly without `--use-mock-keychain` is refused with
`browser::keychain-unmocked`; such a launch would make macOS ask the operator
for their Keychain password.

`spice dev xtask -- app-demo` supplies `appUrl`. It stages the worktree's
current app sources itself before it serves anything, so an app source edit is
visible without a separate `spice dev xtask -- app`; its
`[app-demo] ... sources=... staged_generation=...` line names the source
identity the served generation was compiled from.

An intentional theme-matrix check passes `contextOptions: { colorScheme }`; the
returned `colorScheme` and `colorSchemeSource` report the value and whether it
came from `"system"` or `"explicit"`. A stored `spice:app-theme` preference
still wins over the browser preference.

## Mosaic card sizing

Prose cards fit their content and sit vertically centered in their allocated
lattice cells, capped at the cell height. Check the real message structure,
including its `.mosaic-message-content` clipping grid, prose, quotes and controls:
bare text in a card does not exercise the nested grid's intrinsic sizing.
Measurement cards keep natural height; boundary rows and image covers fill their
cells.

## Theme selector material

The Spice sidebar keeps Light, System, and Dark as three pressed buttons in one
theme group. Their selected face slides behind the buttons and uses the same
transmitted, lit glass recipe as an active team or working-group slider thumb.
The buttons remain the semantic and keyboard controls; the moving face is
presentation only. Its gradient, edge, and light catches remain legible when
backdrop filtering is unavailable. Reduced-motion presentation moves the face
without travel.

The [glass control studies](design/glass-controls.html) are an interactive
reference gallery for the [team composer
controls](CONSOLE.md#team-composer-controls). They send no commands, and
editing them changes no production style.

## Task graph workflow cues

The task overview overlays each Mermaid task node with the authoritative
availability label, and that overlay is the node's only frame: the diagram
draws no stroke of its own around a task node, so selection and state read
from one outline. `PASSIVE` marks an unclaimed Design gathering input: it
keeps a solid node frame and state tag. `WAITING` is healthy workflow progress
that is not running yet, including a Design waiting on its selected work:
it wears the base of the surface containing the map, plum inside a
Design-flavored overview and the page's green elsewhere, and a dotted outline
on both the node frame and its state tag sets it apart from passive and
in-flight's solid frames and deleted work's dashed one. Both passive and waiting
Designs consume the resident's cycle without changing allocation policy.
`DONE` remains quiet settled provenance
with a single neutral frame and rounded tag. `READY FOR PROPOSAL` marks the
unpossessed Design whose tab wears the proposal mark, in its live tone, and
`AWAITING DECISION` the Design whose tab wears the decision mark, in warning.
The outline and tag shapes keep the states distinct without color, including
reduced-contrast and forced-color presentation; waiting never borrows warning,
danger, or operator-attention emphasis.

## Browser branding

The tab title and menu use the lowercase brand `spice`, trailing any changing
state: `5 tasks - spice`, `1 task - spice`, `7 tasks (stale) - spice`,
`Loading - spice`, `Disconnected - spice`, and `spice` before the app loads.
Once a board snapshot arrives, the title and the pepper icon's badge carry the
exact remaining public task count; the badge is exact through 99, then `99+`,
and zero is explicit. Reconnect keeps the last count marked stale until current
data returns. Speech never retitles the tab.

The global Spice summary stands in the open Spice view's header, which keeps
one fixed height. Its title reads `spice` in lowercase; ordinary sidebar and
sheet kickers retain uppercase. The bare product version sits on the title's
baseline beside it, since the title already names the product; a header too
narrow for it hides the plain version. Beneath them the current connection
state is the header's status line: it runs under the theme selector and TURBO
and ends in an ellipsis before the weekly percentage. An available update reads
as the version in the focus accent with a reload mark and stays actionable:
where the header is too narrow for the version beside the title, on a phone or
beside the Windows caption buttons, the update takes the status line's place
while the state stays announced. The admitted journal stream cursor is in the
connection state's hover title and accessible name, and only while the resident
connection is current; startup and reconnect states never present a retained
runtime's cursor as live.

The version's hover title and accessible name carry the full identity, and an
update's carry the same rows beside the generation it offers. Three of its
numbers count different things and are never compared. The crossing wire
generation is the outer envelope every request and frame carries; a build
speaks exactly one and admits no other, so it stays at its one value while
the others advance. The journal migration schema counts the stored journal's
migrations, the only schema of the three. The browser app protocol counts
changes to the browser/resident contract, which a loaded page must match to
connect.

## Composer images

The chooser keeps admitted image bytes when the complete draft fits one durable
steering record. A larger decodable camera or gallery image is flattened onto
white, resized, and encoded as JPEG within the record budget the draft's other
attachments leave. The shared attachment limits remain the submission
authority: an attachment that does not fit is refused in the composer, never
after Send.

Retained steering attachments and transcript images carry an image-specific
Quote action beside full-size opening. It wears the same quote glyph,
dimensions, edges, colours, and interaction treatment as an ordinary message's
Quote action. The action drafts exactly that image under its admitted name. Its
target names the durable steering or tape source, source ordinal, and
decoded-byte digest; paths, data URLs, tool intent, and browser-generated
diagrams never become quote identity. A sent image quote resolves from the same
durable source and renders an explicit unavailable state if that source no
longer authenticates the named image.

In the full-size image viewer, A activates **Annotate** when the image is a
draft image the viewer can annotate, and D activates **Download**: each key
does exactly what its button does, including a refusal, and nothing while the
button is absent or disabled. Held-key repeats, modified keys, and keys typed
into a field never trigger them, and inside the annotation editor A keeps its
arrow-tool meaning.

While an image pasted into a composer is open full size, Right Arrow opens the
next image pasted into that same composer and Left Arrow the previous one, in
the order the composer renders them across its main draft strip and each quote
shard's strip. Navigation never leaves that composer, either end holds without
wrapping, and annotating, IME composition, and Alt, Control, or Command keep
the arrows from stepping. Each image opened this way annotates its own draft
attachment.

The image annotation editor closes immediately on Cancel, Escape, or backdrop
while its marks equal the set it opened with; once they differ, those exits
confirm before discarding, and undoing back to the opening set removes the
confirmation. Clicking empty canvas only clears selection. Apply commits
without a prompt, and dirty annotation work joins the page-unload warning.

Annotation modifiers act live: pressing or releasing one mid-gesture updates
the mark without moving the pointer. While drawing a line or arrow, or dragging
one of its ends, Option or Alt routes it as an elbow and Command or Control as a
curve; Shift snaps a straight mark to 45 degrees and mirrors a bent one, so a
curve bows to the other side of its chord and an elbow turns along its other
axis. M mirrors a selected curve or elbow as one undoable edit. The mirrored
bend survives moves, undo, redo, and export. Shift draws or resizes a rectangle
as a square, and Option or Alt draws one from its centre. Dragging a mark with
Shift keeps it to horizontal or vertical travel, and Option or Alt drags a copy
while the original stays in place.

## Message cards

Every attributable prose run between nested quote blocks is one speaker-owned
hunk. It opens with the source's Quote/Play controls, relative age, name, and
one classification pill: ACK or NAK first, FINAL next, then BROADCAST. Play
speaks only that hunk's rendered text; Quote targets that hunk's exact
authenticated source, operator-authored included. Direct text after a nested
quote starts another hunk with its source's controls and attribution; the
rule applies recursively to agent prose and resolved operator, agent, and peer
quotes.

Every message Play and Quote action uses the shared message control size and
the same open-edge pair docked at the card's inline end. Provider failures lead
their exact error text with the card's relative age and name, without prose
punctuation rewriting. Other non-prose cards keep one attribution-and-action
island in their card corner. A single-image cover places that same unframed
island over its image; it does not introduce another inset panel or action
treatment.

Every visible inline atom keeps its hunk's `1.25` line pitch: links, task
handles, emphasis, emoji, superscript and subscript, inline math, and quote
attribution never enlarge a line box. The image row therefore begins after
exactly two lines, and its status stays inside the image grid rather than
changing prose leading.

Acknowledgment is an authenticated message disposition, not source text. Every
direct hunk of the answering speaker wears the same ACK or NAK segment, quoted
hunks never inherit it, and pending settlement revises those segments in place.
TASTE marks the quoted directive the resident sent, not the answer. A typed
REVIEW presentation replaces its prose delivery envelope with a finding card;
that card starts with the authenticated Spice source, REVIEW segment, and
reviewed task handle, then closes with typed system guidance that follow-ups are
already boarded and the agent must continue its current task. Its Play action
reads that same task, finding, and guidance in visible order; its Quote action
keeps the authenticated source target while deriving the composer preview from
the typed finding. The discarded envelope reaches neither the actions nor the
rendering, and the lead does not repeat after nested finding quotes. Literal
`[ACK]`, `[NAK]`, `[TASTE]`, or `[REVIEW]` without an authenticated owner stays
ordinary Markdown.

A quoted peer MSG keeps its original sender, arrow, and exact destination from
the creating journal fact; a fanout reply quotes only its own delivered
destination. An authored MSG appears as an outgoing quote led by its card's
relative age and lists every addressed occupant in header order. Its protocol
header remains hidden.
A lane's name and accent belong to the lane, so an occupant that has since left,
whether its lane is vacant or reseated, still reads under them as a historical
attribution. A lane absent from the roster reads as unavailable, or as the
recipient's address, never as raw identifiers.

A quote whose sole recipient is the lane already named around it leaves that
receiver implied. Around a quote means the prose card that lane spoke, or the
quote card it spoke for a nested quote. The signoff then shows only the age and
sender. The arrow and receiver stay in its accessible name, and its hover title
names both ends, speaker → receiver. Several recipients, another lane, or a
context with no speaker, such as a direction card in the team plane, keep the
arrow and receivers visible.

A directive that an MSG delivered, quoted as the context an acknowledgment
answers, shows the packet body without its header and wears no pill, as an
authored MSG does. The opening room publication proves the packet; any other
directive stays literal, whatever its words spell.
An operator packet names the occupant whose acknowledgment the card presents
as its destination. A quote link to such a directive presents the same way:
its card shows the packet body and the lane the directive was
delivered to after the arrow. A quote's head keeps the card head's type scale
and name color.

Each authenticated operator direction a lane's transcript recorded is also a
top-level card in that lane's team plane. Its canonical source row is the
earliest recorded delivery, while its placement and relative age use the
direction's operator-authored instant. The card rests in the operator's neutral
accent. Its body is the direction as one quote reading operator → receiving
members, with that quote's Quote and Play controls and its steering quote
target. One send fanned out to several members of a team is one card naming
each of them, so a resend, a reload, or an older page adds no second
card. The direction's relative age
links to that card as any card's age does, and a quote of the same direction
inside another card links there too; with no direction card in the team plane,
that age stays unlinked. A peer delivery or a post-compaction replay is no
direction card, and the answering ACK or NAK card still quotes the direction
it answers.

Each quote layer ends with one task target after its speaker's last word,
filled with origin-task badges; it repeats on no other hunk, and Play never
reads it. The badges remain in ordinary inline flow, with no floated placement
or reserved minimum height, so the prose uses its full line and wraps the badge
only when needed. An isolated task-handle chip's hover title is its surrounding
activity prose, or `Open task HANDLE` alone. Directly trailing sentence
punctuation stays on the chip's line without preventing a break before it.
A quote's rail shows its speaker: an agent's or peer lane's quote wears that
lane's accent; a direction card's quote wears its first receiving member's;
other operator, Spice, taste, broadcast, review, and unattributed
quotes take the neutral accent; an
unavailable quote keeps its warning rail.

A card's linked relative age is its permalink. Arriving through it outlines
the card briefly, then fades the outline (cleared at once without motion), and
focuses the card without a visible focus ring.

## Composer activity rail

A lane's trace runs unframed and flush across the foot of its primary
composer, a 32-pixel rail behind the draft covering the last ten minutes of
that lane's activity at the rail's own measured size, with gaps where the lane
was off, from an occupant's end or a vacant seat to the next occupant's start;
a reload keeps those gaps from the stream's recent occupancy. There each kind
of work with activity in the window, reasoning, tool, and prose, draws one
smoothed curve over a fill fading toward the floor, ending in a dot at now; in
the dark theme the curves glow. The curves keep a four-pixel headroom under the
rail's top, so the graph itself is 28 pixels tall. Turn, direction, provider
failure, compaction, fresh session, and sign-out draw as markers over the
curves, the rules spanning the whole rail and the direction dots sitting in
the headroom, so every marker stands clear above the highest peak. The
operator's direction dot is neutral ink; an MSG another lane sent this lane
draws the same dot at its send instant in the sending lane's accent, while the
sender's own rail and a send that names no lane mark nothing. Quiet is
not off: a running lane with no activity still draws its low line. Between
redraws the curves, gaps, and markers slide left at the window's own rate, so
they glide rather than step however wide the composer stretches, while the
now-dots hold the right edge; reduced motion keeps the step. The rail
takes no pointer input, drains to gray while the lane's seat is empty, and fades out
once the draft holds text. Quote shards carry no rail. The lane's provider
emblem is its menu button's glyph, and that button's hover title names the
provider standing the emblem names.

The app's `style-src 'self'` policy refuses inline style attributes, so the
rail is SVG geometry in the pane's measured CSS pixels, every colour and glow
comes from the stylesheet, and the slide's span and duration reach it as
custom properties written through CSSOM. A Playwright check of it must
keep that policy enforced: a context with `bypassCSP` would hide a drawing that
draws nothing in production.

Yew's `html!` lowercases literal element names, and SVG recognises
camel-case elements such as `clipPath` only by their exact case, so the rail
writes those as dynamic tags (`<@{"clipPath"}>`).
