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

## Room authority and browser preferences

Every stream subscription, first connect and same-scope reconnect alike,
rebuilds one deterministic projection of task-keyed room timelines,
repository working groups, participant reasons, preparation evidence,
decisions, authorizations, and claim-bound stop requests. The `rooms` subject
pages the facts behind current room authority at the subscription horizon:
every room publication, the current head of each group, proposal, and
authorization, and each coordination's direction and current steering head.
Live facts strictly after that horizon continue it, so the operator's
live-start fact stream neither repeats nor misses authority. The projection is
unavailable until its snapshot completes.

Durable task stamps and room publication stamps key cards across reloads and
unavailable source bytes. The current board snapshot alone resolves an
anchor's displayed handle, an output's control, and whether a stop's exact
claim win is still active; absence is shown as unavailable or missing rather
than reconstructed from an old handle.

Snapshot pages share one horizon, rise strictly, and never move backward. A
moved horizon, regressed page, live fact before the snapshot completes beyond
its horizon, live fact at or before it, or stream overflow invalidates the
projection and reconnects for a fresh snapshot. It is never merged
speculatively.

Working groups are shared catalog organization only. Open room views, view
order and layout, composer drafts, reader and scroll positions, and focus
anchors remain browser preferences. Those preferences cannot grant task, room,
group, team, or lane authority, and a group edit never mounts, reorders, or
closes a browser view.

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

After the operator panel the Spice view shows **Jobs**, every job every lane
keeps, apart from the topology roster; **Provider quota** ends the view. The
list and its full-width inspection card share the existing four-row slot.
Empty, loading, filtered, and inspection states retain that footprint; larger
lists and evidence scroll inside it rather than enlarging the sheet.

Rows show kind (▶ command, ◆ service, ● watch), agent accent and name, observed
start age, and the complete command in the tooltip. Ended jobs awaiting agent
delivery use hollow glyphs. Search matches agent, command, or job key; the
heading counts also filter by kind or unheard endings. Click **Jobs** to clear
the filters. An unpublished start displays `?`, not an invented age.

Open a row to inspect its command, working directory, keeper lifecycle, start,
deadline, and ending evidence. **Summary** and **Output** share the same card;
previous/next navigate matching jobs without opening another pane. Output is
the keeper's bounded tail, with an explicit notice when earlier output was
omitted. Copy copies the complete command or the displayed output. Back returns
to the list; Escape first cancels a kill confirmation, otherwise returns.

**Kill** requires running/starting evidence and current operator authority.
Confirmation names the exact job key, focuses **Cancel**, and asks the existing
keeper to terminate that command's process group. Stopping, ended, unavailable,
and no-longer-cataloged references cannot be killed from the card. A sent stop
is not replayed after reconnect; refresh reads what the keeper actually did.
Inspection and operator termination never mark the agent's result as delivered.
An operator-stopped ending remains eligible for its ordinary agent notification.

The operator-only `lane::jobs-read`, `lane::job-read`, and `lane::job-stop` reads
and control operate on the existing keeper records, not a second job store.
Their job payloads use the native base52 job key and an encoded topology
revision/lane slot, not a new UUID identity. Changed topology invalidates the
reference before any stop marker is written; return to the list for a current
reference. Existing lane summaries and retained keeper data keep their current
shape. Catalogs refresh while the panel is visible; live inspection refreshes
while its card is visible. Disconnection and missing records are explicit.

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
activity, waiting direction, the lane identity, occupant, session, and worktree
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
