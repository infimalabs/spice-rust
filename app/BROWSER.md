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

An intentional theme-matrix check passes `contextOptions: { colorScheme }`; the
returned `colorScheme` and `colorSchemeSource` report the value and whether it
came from `"system"` or `"explicit"`. A stored `spice:app-theme` preference
still wins over the browser preference.

The [glass control studies](design/glass-controls.html) are an interactive
reference gallery for the [team composer
controls](CONSOLE.md#team-composer-controls). They send no commands, and
editing them changes no production style.

## Browser branding

The tab title and menu use the lowercase brand `spice`, trailing any changing
state: `5 tasks - spice`, `1 task - spice`, `7 tasks (stale) - spice`,
`Loading - spice`, `Disconnected - spice`, and `spice` before the app loads.
Once a board snapshot arrives, the title and the pepper icon's badge carry the
exact remaining public task count; the badge is exact through 99, then `99+`,
and zero is explicit. Reconnect keeps the last count marked stale until current
data returns. Speech never retitles the tab.

## Resident topology in the global menu

The Spice view leads with connection state, the inspectable version stack, and
global TURBO. An available app generation turns the version into the explicit
reload control. A superseded app generation keeps running until its session
fails against the resident serving the replacement; that failure reloads the
tab into the served generation once per generation, unless a composer draft or
unsettled submission would be lost, in which case the tab stays and reconnects
behind the reload control.

Resident topology is one interactive roster. Each team lists its agents with
presence, name, worktree, model, pending steering, liveness, held task, kept
jobs, and provider emblem. A teamed agent opens its composer in Chat. When team
configuration is admitted, agents drag onto another team, onto **New Team**, or
onto **Unassigned Agents**; every drop carries the topology revision shown, and
unavailable configuration disables dragging without disabling Chat links. Teams
stop at six agents: a full team disables its mouse and touch drop target, while
its members can still leave for another team or Unassigned Agents. The resident
also refuses membership commands that would exceed six; stale clients cannot
bypass the limit. A mouse drags the whole slot; a finger or pen drags the agent's
name, so the rest
of the slot still scrolls the sheet and a tap still opens Chat, and holding a
lifted agent near the sheet's edge scrolls it toward an off-screen target.

## Jobs in the global menu

The Spice view ends with **Jobs**, every job every lane keeps, apart from the
topology roster. Each job is one fixed-height row in its agent's accent: a
glyph naming its kind (▶ command, ◆ service, ● watch) solid while running and
hollow once it ended unheard, the agent, its age, and the command. The heading
counts each kind, zero included, and the unheard. Rows run oldest first, so a
new job joins at the end and long-lived services settle at the top; every cell
has a fixed width, so a tick or a standing change moves no row.

## Provider quota rows

**Accounts lacking usage** and **Disabled credentials** appear only when they
have entries. Each account-limit row shows the limit name, a capacity bar
filled with what remains, the amount — `N% left` for a percent limit, `R / L
unit left` for a counted one — and the reset cadence. An account sharing the
weekly limit wears the same accent as its layer in the Spice button's quota
fill; others stay neutral.

The reset-credit summary states available and redeemable counts, the available
count standing in when the provider omits redeemable. Each redeemable credit
shows its provider title, program, and remaining count when present, its grant
age (`Granted at unknown time` when absent or invalid), and any valid expiry as
a countdown, exact times on hover. A provider-reported raw credit balance shows
on its own account as whole credits, truncated so a fraction never reads as
more than can be spent; it is neither a time-window meter nor a reset grant,
accounts never share or aggregate it, and Spice never infers it.

## Arranging teams

A team is named by its lead agent, canonical member zero, with the label its
messages carry: the configured name, else the worktree leaf. Only a team with
no current members keeps its short id.

Pick up a team from its pip matrix: dropping on the middle of another team
swaps the two teams' positions; dropping on either outer edge joins the source
into that team, which keeps its identity, primary composer, and member order and
appends the source members in order. The pip stack and the team menu open the
same **Swap positions** and **Join into** choices by keyboard or tap; Escape
returns focus to whichever opened them. Joins stop at six members: a target
that would make a seventh is shown unavailable and sends nothing. Every
accepted arrangement sends the complete topology at the drawn revision;
cancellation, a changed revision, or a removed source sends nothing.

## Lane menu

Each Chat lane's menu button swaps that lane's composer for its seat panel. The
panel leads with what the agent needs from the operator — consent requests
to Allow or Deny in place, held tasks iced, blocked, or in review, a lease near
expiry, and direction left waiting — then the Wake, Drain, Remove, Renew, and
next-launch model, effort, and fast-mode commands and their refusals. Renew
submits at once; Drain and Remove confirm first. Effort and fast mode show the
running value beside the next launch, and the fast-mode status names which
level enables it; fast mode follows [the lane
loop](../docs/product/loops.md#l-lane--the-place). Presence, lease age, current task, latest
activity, waiting direction, the agent channel, occupant, session, and worktree
follow, beside **Create new team** and **Leave all teams**. The button, Escape,
or a click elsewhere in the team restores the composer.

## Composer images

The chooser keeps admitted image bytes when the complete draft fits one durable
steering record. A larger decodable camera or gallery image is flattened onto
white, resized, and encoded as JPEG within the record budget the draft's other
attachments leave. The shared attachment limits remain the submission
authority: an attachment that does not fit is refused in the composer, never
after Send.

The image annotation editor closes immediately on Cancel, Escape, or backdrop
while its marks equal the set it opened with; once they differ, those exits
confirm before discarding, and undoing back to the opening set removes the
confirmation. Clicking empty canvas only clears selection. Apply commits
without a prompt, and dirty annotation work joins the page-unload warning.

## Message cards

Every attributable prose run between nested quote blocks is one speaker-owned
hunk. It opens with the source's Quote/Play controls, relative age, name, and
one classification pill: ACK or NAK first, FINAL next, then BROADCAST. Play
speaks only that hunk's rendered text; Quote targets that hunk's exact
authenticated source, operator-authored included. Direct text after a nested
quote starts another hunk with its source's controls and attribution; the
rule applies recursively to agent prose and resolved operator, agent, and peer
quotes.

Acknowledgment is an authenticated message disposition, not source text. Every
direct hunk of the answering speaker wears the same ACK or NAK segment, quoted
hunks never inherit it, and pending settlement revises those segments in place.
TASTE and REVIEW mark the quoted directive the resident sent, not the answer.
Literal `[ACK]`, `[NAK]`, `[TASTE]`, or `[REVIEW]` without an authenticated
owner stays ordinary Markdown.

A quoted peer MSG keeps its original sender, arrow, and exact destination from
the creating journal fact; a fanout reply quotes only its own delivered
destination. An outgoing packet lists every addressed occupant in header order.
Departed occupants never inherit a successor's name or accent.

Each quote layer ends with one task target after its speaker's last word,
filled with origin-task badges; it repeats on no other hunk, and Play never
reads it. An isolated task-handle chip's hover title is its surrounding
activity prose, or `Open task HANDLE` alone. Directly trailing sentence
punctuation stays on the chip's line without preventing a break before it. A
quote's rail shows its speaker: an agent's or peer lane's quote wears that
lane's accent; operator, Spice, taste, broadcast, review, and unattributed
quotes take the neutral accent; an
unavailable quote keeps its warning rail.

A card's linked relative age is its permalink. Arriving through it outlines
the card briefly, then fades the outline (cleared at once without motion), and
focuses the card without a visible focus ring.
