# Menus

The global Spice menu, team arrangement, and each lane's menu.

## Resident topology in the global menu

The Spice view leads with connection state, the inspectable version stack, and
the theme selector. Global TURBO stands beside the Spice button in its header,
starting where the theme selector starts, with the weekly percentage above the
button. An available app generation turns the version into the explicit reload
control. A superseded app generation keeps running until its session
fails against the resident serving the replacement; that failure reloads the
tab into the served generation once per generation, unless a composer draft or
unsettled submission would be lost, in which case the tab stays and reconnects
behind the reload control.

Resident topology is one interactive roster, and the working-group catalog is
the same roster over designs. Each team lists its agents with presence, name,
worktree, model, pending steering, liveness, held task, kept jobs, and provider
emblem; a teamed agent opens its composer in Chat.

One roster component draws both kinds. Formed groups stand in their shared
order, each a card named for member zero holding its members' tiles; **New
Team** or **New working group** follows, then **Unassigned Agents** or
**Designs**, which lists only members no group holds. While the kind's
configuration and arrangement are admitted, every tile and card drags; a mouse
drags the whole tile or card, a finger or pen drags a tile by its name or a
card by its header, so the rest still scrolls the sheet and a tap still opens
Chat, and holding a lift near the sheet's edge scrolls it toward an off-screen
target. A member released on the New target begins a group holding only it; on
a card, or a tile in another group, joins that group; on a tile in its own
group takes that place; on the ungrouped list leaves its group, and a group
whose last member leaves ends. A card released on another card or its tiles
takes that place among the groups. Reordering restates the whole arrangement
(`team::arrange` or `group::arrange`); every other release moves one member.
A release where nothing would change, a group's only member on New, or a card
on New or the ungrouped list sends nothing. Groups stop at six members: a full
group's card and tiles disable their mouse and touch drop target, while its
members can still reorder or leave, and the authority refuses a stale client's
seventh member. Every change carries the revision the roster was drawn from.
No roster gesture mounts, reorders, closes, or focuses a view.

## Who the app names

The app names humans from principal profiles, never Git. The connected
principal's own Principals row and inbox heading show their name, initials, and
Gravatar.
Activity, task steps, room lines, and quote bylines use the author's profile;
a quote shows initials with the full name as its accessible label. Historical
`operator` facts name the earliest-born principal.

Before subscribing, the app reads the connected-readable directory and uses
the caller's roles to select operator-only streams and reads. Any `principal::`
fact re-reads it: profile edits publish in place; role edits reconnect so reads
and subjects change together. A profile without a name reads as `human <id>`;
the historical actor reads as Operator.

## Principals in the global menu

After the global settings, the Spice view shows **Principals**. The connected
principal appears there once, as their own row marked `(you)`, and that row
seats their personal inbox with its heading, open count, items, and controls.
The row and inbox stand whether or not this connection may list principals,
and while the listing loads or is refused; until the principal directory
arrives the row reads as loading. An operator-role principal also sees every
principal the repository has admitted, oldest first. Each reads with its avatar
and initials, name, email, roles, capability standing (issued, local for the
earliest-born, whose capability each resident generation mints, or revoked),
and admission age with the exact instant on hover. Its controls send the
commands `spice principal` sends: **Edit profile** opens a form whose changed
fields replace the recorded ones, **Grant operator** or **Revoke operator**
changes the one role, and **Revoke capability** asks once more before retiring
an issued capability. **Admit a human**, at the heading's trailing end once the
listing arrives, opens beneath the heading a form taking a required name, an
optional email, and the operator role; the reply's capability shows once above
the list as the opening link `ORIGIN/#CAPABILITY` with **Copy link** and
**Done**, held only in that panel and dropped with its connection. The listing
re-reads on every `principal::` fact, so another session's change shows at
once. The listing is operator-only: a principal without the operator role sees
only their own row and its inbox, and a listing the resident refuses reads
beneath that row as taking the operator role. One change is in flight at a
time; its progress and any refusal, such as the last operator's revocation,
read under the heading.

## Jobs in the global menu

After Principals, the Spice view shows
**Jobs**, every job every lane keeps, apart from the topology roster. Jobs
stays ahead of Drafts, OOPS, working groups, resident topology, and the **Provider
quota** section that ends the view. The list and its inspection card share one
fixed four-row slot in every state; longer content scrolls inside it. At either
edge, arrow and page keys continue through the containing Spice menu instead of
trapping keyboard navigation in Jobs.

Rows show kind (▶ command, ◆ service, ● watch), agent accent and name, observed
start age, and the complete command on hover; unpublished starts show `?`, and
ended jobs awaiting delivery use hollow glyphs. Search covers agent, command,
and job key. Heading counts filter by kind or unheard endings; **Jobs** clears
filters.

Inspection shows command, working directory, keeper lifecycle, start, deadline,
ending evidence, and bounded output tail with an omission notice. **Summary**
and **Output** share the card; previous/next traverse matching jobs. Copy takes
the complete command or displayed output. Back returns to the list; Escape
first cancels a kill confirmation.

**Kill** requires running/starting evidence and current operator authority.
Confirmation names the exact job key, focuses **Cancel**, and asks the keeper
to stop that process group. Stopping, ended, unavailable, and absent jobs
cannot be killed. A stop is not replayed after reconnect, and neither
inspection nor stopping marks the agent's result delivered; an
operator-stopped ending still receives its ordinary agent notification.

The operator-only `lane::jobs-read`, `lane::job-read`, and `lane::job-stop`
operate on keeper records, not a second store. References carry the native
base52 job key plus encoded topology revision/lane slot. Topology movement
invalidates a reference before any stop marker; return to the list for a
current one. Catalogs refresh while shown, live inspection refreshes while
open, and disconnection or missing records is explicit.

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

Each team console takes its whole base from its age among the teams: teams
count up from the palette's start by when each became a team, so the oldest
wears green, the next blue, then gold, and an unteamed agent takes no place. The
lead wears that base and each later member the next slot up. Colours follow only
that age, so swapping two teams, or opening, closing, or reordering consoles in
one browser, changes none; only a team forming or an older one ending moves a
colour.

Team consoles follow the composer model: the first team in arrangement order
holds the right edge beside the app's controls and each later team, including
one newly formed, stacks to the left of its predecessor; a fleet wider than the
window shows that right edge first. Opened working groups follow the teams in
the same flow. Inside every console the toolbar, composers, and messages read
left to right.

Pick up a team from its pip matrix: dropping on the middle of another team
swaps the two teams' positions; dropping on either outer edge joins the source
into that team, which keeps its identity, primary composer, and member order and
appends the source members in order. The pip stack and the team menu open the
same **Swap positions** and **Join into** choices by keyboard or tap, and the
menu's **Disband team** leaves every member as an unassigned lone seat. Escape
returns focus to whichever opened the menu. Joins stop at six members: a target
that would make a seventh is shown unavailable and sends nothing. Every
accepted arrangement, including disbanding, sends the complete topology at the
drawn revision. **Disband team** toasts its typed refusal and refreshes that
topology; cancellation or a removed source sends nothing.

## Lane menu

Each Chat lane's compact menu button shows that lane's provider emblem, or
omp's pi when the model names no recognized provider, and its hover title
names the model, effort, and fast mode the emblem stands for. It swaps that
lane's composer for its seat panel. The panel leads with what the
agent needs from the operator — consent requests to Allow or Deny in place,
held tasks iced, blocked, or in review, Designs awaiting a decision, a lease
near expiry, and direction left waiting — then the Wake, Drain, Remove, Renew,
and next-launch model, effort, and fast-mode commands and their refusals. Renew
submits at once; Drain and Remove confirm first. Effort and fast mode show the
running value beside the next launch, and the fast-mode status names which
level enables it; fast mode follows
[the lane loop](../docs/product/loops.md#l-lane--the-place). Presence,
lease age, current task, latest activity, waiting direction, the lane identity,
occupant, session, and worktree follow, beside **Create new team** and **Leave
all teams**. The button, Escape, or a click elsewhere in the team restores the
composer.
