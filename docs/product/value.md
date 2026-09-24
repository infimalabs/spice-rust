# Why this is valuable

Not the code constraints — the *workflow* guarantees and the *interface
vocabulary* that make supervising a fleet of coding agents tractable for one
person.

---

## The thesis

Running one coding agent is a chat problem. Running a fleet is an **operations**
problem, and operations needs three things chat does not have.

```mermaid
flowchart LR
  D["DURABILITY<br/>accepted intent survives"] --> A["ATTRIBUTION<br/>nothing landed is anonymous"]
  A --> C["CALM<br/>nothing moves that I am reading"]
  C --> D
```

**Durability** begins when authority returns a live key or covering standing;
the browser's pre-acceptance queue is only an ephemeral prediction. Beyond that
boundary it turns *"I told it to"* from a memory exercise into a fact the
system holds. **Attribution** turns *"who did this and why"* from a reading
exercise into a query. **Calm** turns many live streams into something one pair
of eyes can actually track. Remove any one and the other two stop paying: an
un-attributed fleet can't be trusted, an un-durable one can't be steered, and a
jittering one caps throughput at the operator's ability to re-find their place.

Every model in this document serves exactly one of these three.

---

## Workflow invariants

The invariant register owns every cross-document label; the headings here carry
their own meaning.

### Steering is durable and has a typed lifecycle

```mermaid
flowchart TD
  O["operator sends"] --> J["journal accepts one directive fact<br/>atomically"]
  J --> F["filesystem inbox projects<br/>the still-live directive"]
  F --> B{"next admitted agent boundary"}
  B --> V["vendor post-tool hook"]
  B --> S["active or next top-level shell command"]
  V --> K["agent ACKs or declines<br/>operator may cancel"]
  S --> K
  K --> T["journal records the typed disposition"]
  T --> E(("settled"))
  J --> D["lapsed, or PendingInbox terminal-wake<br/>deadlettering from retained live standing,<br/>remains explicit and recoverable"]
  D -->|"recovery transaction accepted"| J
```

The directive is accepted in the journal before its disposable inbox projection
is delivered. Delivery is eligible at both the vendor's post-tool hook and an
active or next top-level shell command. They share the same live content, not a
suppression decision: vendor hooks consult a locked durable hook-delivery
record, while each shell command independently streams changed rendered-inbox
bytes throughout its immediate-child lifetime and may repeat live content seen
by another command. A *read* never changes disposition. `acked`, `declined`,
`canceled`, `lapsed`, `deadlettered`, and recovery are distinct durable facts,
and a `PendingInbox` automatic wake that returns `Unstarted` or a terminal-class
start failure deadletters every pending directive on that lane from its retained
live stage without requiring prior delivery. No other wake or arbitrary delivery
failure does. That asymmetry makes supervision auditable: "delivered" and
"taken" are separate facts, and the gap between them remains visible.

**Value:** you can walk away mid-instruction. **Without it:** steering becomes
fire-and-hope, and the operator becomes the durable store.

### Decline is a first-class answer

```mermaid
flowchart LR
  I["steering item"] --> A["ACK, taken"]
  I --> N["NACK, declined, with reason"]
  A --> R[retired]
  N --> R
```

Both dispositions retire the item; NACK records `declined`. Decline is
taught beside ACK in the agent-facing readouts and worktree skill. In Canvas,
the resident consumes the marker before publication and the message carries no
disposition styling, so both render as ordinary Markdown with the lane accent.

**Value:** an agent that disagrees says so instead of complying badly. **Without
it:** the only cheap path is fake agreement, and the transcript stops being
evidence.

### Silence is not consent

```mermaid
flowchart LR
  P["item pending"] --> S{"next named time horizon crossed<br/>with no typed disposition?"}
  S -- yes --> E["redisplay at the next stage"]
  E --> P
  S -- "acked, declined,<br/>or canceled" --> D[retired]
```

Unanswered steering re-surfaces at an admitted post-tool or top-level shell
boundary when a named horizon from its publication time has elapsed. Every
unanswered directive follows the same horizons and cadence. Escalation retains
one stable lineage key, so a resend reads as one thread of insistence rather
than a pile of duplicates.

**Value:** the failure mode of a busy agent is *delay*, not *loss*.

### One actor, one claim, bounded

```mermaid
flowchart LR
  N["task next"] --> C["claim::won<br/>disposition active, TTL 3600s"]
  C --> W[work]
  W --> C
  C -- "claim::lapsed accepted" --> L["disposition lapsed"]
  L --> RL["readiness derived from<br/>all current predicates"]
  RL --> N
```

An actor holds at most one active claim. An elapsed lease returns the task to
eligibility only through an accepted typed lapse transaction, without rewriting
claim history. An explicit steal is a separate typed operation: it replaces the
exact claim named by the command, and a displaced occupant learns through an
ownership refusal rather than a fabricated inbox message. Claim context remains
reconstructible through the claim's bounded lease and exact opening and closing
source boundaries.

**Value:** two agents never hold the same task at once, and the board
self-heals from crashes without an operator sweep. Independent tasks can still
meet at an explicit, recoverable baseline conflict.

### Each task declares its own phase flow

```mermaid
flowchart LR
  N["task with no named flow"] --> A{"acceptance exists?"}
  A -- no --> P["plan → todo → review"]
  A -- yes --> T["todo → review"]
  O["operator-named flow"] --> V["nonempty unique<br/>lowercase-word phases"]
  V --> C["current phase is a member"]
```

A task's *flow* is its ordered phase vocabulary. It is any nonempty sequence of
unique, nonempty lowercase ASCII words; there is no global phase enumeration or
slot ceiling, and the current phase must be a member. When no flow is named, a
task without acceptance receives `plan` to `todo` to `review`; every other task
receives `todo` to `review`, independent of visibility. A hidden `.oops` row is
parked in `plan` but retains the complete `plan` to `todo` to `review` flow and
requires explicit promotion.

Every accepted phase transition is a durable journal fact projected by the
board. A new task-owned tree additionally produces one attested synthesized
baseline-first landing; when the baseline already contains the work, integration
instead advances or fast-forwards without a synthetic merge. The complete phase
history is therefore the journal sequence, not the merge sequence. In particular,
a review that finds nothing advances the phase without producing a commit, so it
lands no merge and leaves no trace in a first-parent walk.

```mermaid
flowchart LR
  R["review pass"] --> C{"tree consequence?"}
  C -- "new task-owned tree" --> M["one attested synthesized<br/>baseline-first landing"]
  C -- "baseline already contains work" --> F["advance / fast-forward;<br/>no synthetic merge"]
  C -- "no tree change" --> A["phase advances in journal and board<br/>(no merge in Git)"]
```

What Git first-parent history measures is therefore the review **change** rate,
not review coverage. Counting merges understates the work by every clean review;
the journal and board do not.

### Cross-review is preferred, never required

```mermaid
flowchart LR
  R["eligible candidate"] --> S{"actor's own<br/>unassigned review?"}
  S -- no --> E["ordinary class"]
  S -- yes --> L["last-resort class"]
  E --> A["select by priority,<br/>graph weight, urgency, locality"]
  L --> A
```

This is a **soft** guarantee with a strict selection class, not a refusal or a
numeric priority adjustment. Any eligible candidate that is not the actor's own
unassigned review outranks every such self-review before priority, graph weight,
urgency, and locality are compared. Only when no other eligible work exists does
the self-review class become selectable, so a truly quiet solo board never
deadlocks.

**Value:** the fleet gets independent review whenever it *can*, and degrades to
progress rather than stalling when it can't. This is the sharpest example of the
system's general posture: prefer the good property, never let it stop the work.

### New task-owned work lands as one attested merge

```mermaid
gitGraph
   commit id: "baseline (first parent)"
   branch task
   commit id: "working change Ash, unattributed"
   commit id: "working change Elm, unattributed"
   checkout main
   merge task tag: "Task-Key, Task-Project, Task-Phase, Task-Session"
```

Mutating Git integration happens only at task phase boundaries. When new
task-owned work requires a synthesized landing, that merge is computed before
the checked-out repo is mutated, so the visible terminal state is either the
untouched pre-merge tree or a recoverable merge — never stranded conflict
markers. The trailers ride the merge, not the working commits. When the
baseline already contains the work, integration advances or fast-forwards with
no synthetic merge.

**Value:** `--first-parent` is the ledger of synthesized mutating landings.
Every such landing carries its task, project, phase, and the agent session that
did it, while the journal and board retain every transition whether it produced
a merge or only advanced to an already-containing baseline. That division makes
release notes and fleet forensics queries instead of
archaeology without pretending a clean review changed Git.

### Every task says where it came from

```mermaid
flowchart LR
  A["ack:&lt;key&gt;<br/>authored convention"] --> T[task]
  P["task:&lt;born-stamp&gt;<br/>parent convention"] --> T
  S["gate:&lt;digest&gt; or doc:&lt;digest&gt;<br/>typed-route convention"] --> T
  O["other:&lt;value&gt;<br/>open authored class"] --> T
  T --> B["one origin realm<br/>always required"]
```

Every task carries one origin realm. The wire contract validates only a
nonempty class, a colon, and a nonempty value. Ordinary authored creation
conventionally names a steering directive or parent work; when the actor holds
one active claim, the server may derive that `task:` realm. A typed system route
may instead supply a proved `gate:` or `doc:` cause. Other authored classes are
accepted and preserved, so these routes are conventions rather than a closed
realm enumeration.

**Value:** you can always answer *"why does this task exist?"* without reading a
transcript. In a fleet that files its own follow-ups, this is the difference
between a board and a landfill.

### The lane outlives the agent

```mermaid
flowchart LR
  T["team / team"] --> L
  L["lane (operator's)"] --> CurrentOccupant["occupant: thread Ash"]
  CurrentOccupant -- renewal --> Successor["occupant: thread Elm"]
  L --> H["history, drafts,<br/>scroll position survive"]
  T --> P["shared task policy survives<br/>because team is unchanged"]
```

A lane is an operator-owned container over a worktree; the agent is a tenant.
Renewal is ordinary steering, not a forced termination, and hands the lane to a
new thread while the message stream stays readable.

**Value:** agents become *disposable*. If the agent were the lane, every renewal
would cost the operator their context, and the rational move would be to hoard
long-lived agents — exactly the thing that makes context rot.

### Taste changes begin as evidence-backed proposals

```mermaid
flowchart LR
  M["active tracked maxim bags"] --> F["a trigger matches prose"]
  F --> E{"judge enabled?"}
  E -- no --> S["direct ordinary steering"]
  E -- yes --> J["deterministically shuffled framings tried in sequence<br/>one fresh bounded judge child each"]
  J -- "exact normalized YES" --> S2["confirmed ordinary steering"]
  J -- "exact normalized NO" --> C2["cleared judgment fact"]
  J -- "unavailable, error, timeout,<br/>or ambiguity exhausted" --> D["candidate dropped;<br/>no direct publication"]
  S --> T["match, direct decision, recurrence<br/>metrics"]
  S2 --> T2["match, judged decision, recurrence<br/>metrics"]
  C2 --> T2
  T --> P["report supplies evidence for shared scope<br/>path / extension / vendor / model / phase"]
  T2 --> P
  S --> C["recurring answered corrections mined"]
  S2 --> C
  C --> H["hidden maxim proposal"]
  P --> O{"operator accepts<br/>a tracked change?"}
  H --> O
  O -- yes --> M
  O -- no --> U["active maxims unchanged"]
  U --> M
```

Two loops close here. Maxim *effectiveness* is measured per bag per vendor and
the report supplies evidence for scoping changes; separately, recurring
corrections answered by ACK or decline are clustered into a proposed new maxim
filed on a hidden board.
Neither path edits the tracked set. Only operator acceptance of the resulting
tracked change can add, remove, or rescope an active maxim; a proposal that is
discarded or deferred leaves the active set unchanged.

**Value:** taste arrives early — while the agent is still speaking, not at
review — and the taste set is evidence-driven rather than accumulated by
opinion. **Without it:** a maxim list only ever grows, and false positives
quietly train agents to ignore it.

### An agent's past is derived, never assumed

```mermaid
flowchart LR
  D["durable planes<br/>steering, board"] --> B[briefing]
  T["transcript<br/>what HAPPENED"] --> B
  B --> A["agent resumes"]
  A --> T
  B --> U["what could not be read<br/>is stated, not omitted"]
```

Context is finite and history is not, so everything an agent knows about its own
past after a compaction is reconstructed. The reconstruction reads intent from
the durable planes and evidence from the transcript, and never lets one stand in
for the other — a transcript is authoritative about what happened and about
nothing that is wanted.

**Value:** an agent resumes instead of restarting, and renewal costs nothing,
which is what makes disposable agents affordable in practice. **Without it:** the agent's own
restatement of a directive becomes the directive, and every compaction is a
small game of telephone with the operator.

### External publication follows the exact plan the operator read

```mermaid
flowchart TD
  V["a publication verb with<br/>external consequences"] --> P["resolve and render the ordered plan"]
  P --> R["operator reads it"]
  R --> X["execute, pinned to that plan"]
  X --> C{"still the same plan?"}
  C -- no --> REF["refuse, naming the expected<br/>and supplied plan digests"]
  C -- yes --> GO["proceed, in the stated order"]
```

Externally visible release publication is executed from a resolved, sealed plan.
*Moving* a tag is not a plan option because the system refuses to do it at all
([A version names
exactly one commit](invariants/distribution.md#distribution-a-version-names-exactly-one-commit)) — a
published version names one commit forever. Each effect is expressed as a plan
first, rendered in full, and executed only against the digest the operator
authorized; any changed input requires a new preview.

This is the durable-steering guarantee pointed at decisions rather than
instructions: the operator's
approval is made durable and binding in exactly the way their steering is.

**Value:** approval covers a specific set of operations rather than an
intention, so the gap between reading and authorizing carries no risk.
**Without it:** consent is given for a summary while something else runs, and
the discovery happens after the one step that cannot be undone.

---

## UI components

### The Canvas view rail

```mermaid
flowchart LR
  C["Canvas"] --> I["Info<br/>what is connected and possessed?"]
  C --> W["Work<br/>what work is moving?"]
  C --> H["Chat<br/>converse with seated agents"]
```

Info, Work, and Chat are the sole panel names in global navigation and each
team's tab rail. Info and Work present the internal Topology and Flow axes;
those analytical identities and the domain team, task, and agent names stay
unchanged. The global rail changes the question asked of the whole Canvas,
not the mode of one lane or team.

The global rail is the first row inside the Spice Canvas menu, immediately
above global settings. It spans the menu width and divides that width equally
among every destination, so adding another global destination preserves equal
access rather than shrinking one privileged control.

Panel variants and DOM mode identifiers use Info, Work, and Chat directly.
The selected global destination is the `page` URL query: switching destinations
pushes browser history, and Back, Forward, reload, and a copied URL restore the
same destination. A legacy URL without `page` continues to open Chat. Team
selection, stored speech preferences, and narration cursors retain their
existing formats and restore unchanged; no panel-name data migration or storage
reset is needed.

### The global Work page

The global Work page keeps the complete open-work set visible as one board:
scope controls and totals establish context, each project's swimlane contains
the same in-flight, ready, blocked, and deferred status buckets, and History
follows the open board. Status is named in text and reinforced consistently by
the bucket header, count, empty well, and card edge; color never carries status
alone. Empty buckets remain visible evidence rather than disappearing.

Cards preserve their identity, flow, project, ownership, and map controls while
the detail drawer and relationship map expose progressively deeper evidence.
Those surfaces share the board's typography, borders, spacing, and interaction
states. The board moves from four columns to two and then one as width narrows;
narrow empty buckets stay compact so absent work does not dominate the page.

### The team console

The Chat view tiles one console per live team. A console contains
one or more member lanes, and every row answers a question the operator would
otherwise have to go and ask.

```mermaid
block-beta
  columns 2
  T["team row<br/>member lights, mode rail, team menu"] q1["which team is this,<br/>and who is alive?"]
  V["team view<br/>Info | Work | Chat"] q2["what can I do with the team<br/>without leaving?"]
  S["member surfaces<br/>lane/worktree address identity;<br/>pane-specific standing fields"] q3["where is each lane addressed,<br/>and what does this pane expose?"]
  M["merged message stream<br/>newest first, explicit lane attribution"] q4["what did the team<br/>just say?"]
```

The whole strip is scannable top to bottom in about a second, and the order is
deliberate: liveness before routing, routing before state, state before words.
An operator sweeping several teams reads the first two rows of each and stops at
whichever team needs attention.

The member row describes the cross-mode surface, not one field bundle. Compose,
Metrics, and Info expose lane/worktree identity and their own applicable agent,
liveness or age, pending, and held-work fields; no pane repeats the whole set.

The team menu's **Import agent** action opens one panel for creating a new agent
or importing an existing one. The complete Seat agent form sits above the import
choices; Info contains only agent and team information. An empty fleet exposes
the same panel from its toolbar, so the first agent can still be seated.
A successful import keeps the panel open so the operator can import additional
agents without reopening it.

An expanded member's action drawer offers **Create new team** when that member
belongs to a multi-agent team. It moves only that member into a fresh team,
preserves every sibling in the source team, and inherits the source team's
allocation policy. The action is disabled for a sole agent.

**Leave all teams** moves only the selected member to its own lane identity,
preserving its seat and every other member. It is disabled only when that lane
already stands outside a team.

All six fields remain visible, with aligned, bounded labels beside their controls:
three paired rows when space permits, one field per row on narrow tiles.
The Seat agent button stays at the form's upper right. Long values scroll inside
single-line inputs rather than widening the tile; admission validation stays inline.

**Why it's the right unit:** the server-owned team topology is exactly what the
operator arranged. The team tiles as one surface while each member lane remains
the concrete address of one worktree. That keeps a fused team together without
turning its members into a broadcast address.

### Lane, team, occupant

Three objects that are easy to conflate and expensive to conflate. The thing
that separates them is not what they contain — it is **how long each one
lives**.

```mermaid
classDiagram
  class Org["ORG — server truth"] {
    <<authority>>
    one or more lanes
    shared task policy
    fusing merges teams
    membership converges across browsers
    visibility may differ locally
  }
  class Lane["LANE — the operator's"] {
    <<outlives its occupants>>
    one worktree
    always a concrete send address
    host renders, members send
    history, drafts, scroll
  }
  class Occupant["OCCUPANT — the current agent"] {
    <<replaceable, and meant to be>>
    one thread at a time
    renewal swaps THIS and nothing else
  }
  Org "1" *-- "1..*" Lane
  Lane "1" o-- "0..1" Occupant
```

Read the two relationships as lifetimes. A lane belongs to exactly one team at
a time and may move between teams, while the team is the unit that the current
browser's `Close team` presentation action hides. That action does not alter
the relationship, and **Agents → Open team** can restore a closed grouped
team during the same mount. The team owns the task policy shared by its members: renewal
leaves that policy in place because the team is unchanged, while a lane moved
to another team takes the destination policy rather than carrying one with
it. An occupant is held loosely and may be absent entirely —
that absence makes the lane vacant, while the UI independently presents it as
`bound` or `unbound` (unless queued steering outranks liveness). A fused host
renders one merged newest-first stream while every member stays its own concrete
send address.

**Value:** the operator manipulates topology directly — drag a team onto
another team's edge to fuse — and the server remains authority for membership,
ordering, and revision. Browsers can intentionally differ in which teams they
hide without disagreeing about that topology. Because the occupant is the only
replaceable part, replacing it is cheap enough to do casually, which is the
entire point of the separation.

### Closed loop: The fused composer

A team presents as one control surface: an ordered **strip of composer
columns**, one per member. The operator reads it across, with east/west overflow
when needed; it is not a set of tabs they switch between.

```mermaid
flowchart LR
  subgraph STRIP["the composer strip — one column per member, in member order"]
    direction LR
    AshShard["Ash shard, amber accent"]
    ElmShard["Elm shard, teal accent"]
    YewShard["Yew shard, violet accent"]
  end
  AshShard --> AshWorktree["Ash worktree"]
  ElmShard --> ElmWorktree["Elm worktree"]
  YewShard --> YewWorktree["Yew worktree"]
  AshWorktree --> M["one merged stream in the host lane<br/>newest first"]
  ElmWorktree --> M
  YewWorktree --> M
  M --> A(["each card names its lane;<br/>its lane-derived accent is continuity"])
  A --> STRIP
```

The loop is the point. Member order lays out the shards; it does not order
messages or assign identity. Each lane's stable identifier derives its accent,
so reordering never recolours history. The finite palette may repeat, therefore
cards carry explicit lane and worktree attribution and colour is only
presentation continuity. Drag a shard to reorder within the team; drag it out to
move that lane to another. A member that momentarily fails to resolve keeps its
seat rather than being dropped from the stack.

**Value:** addressing three agents is one surface instead of three, and nothing
is broadcast — every message still goes to exactly one worktree, through that
member's own concrete address. The merged stream is a reading convenience laid
over unchanged addressing, never a shared mailbox.

**Without it:** a team is either three separate surfaces the operator switches
between, or one surface that broadcasts. The first does not scale past a couple
of agents; the second makes every message everyone's problem.

### The autonomy dial

```mermaid
flowchart LR
  S["Steer<br/>explicit direction and<br/>task capture only"] --> D["Drive<br/>automatically draw work<br/>created for this team"]
  D --> R["Drain<br/>draw all otherwise<br/>assignable work"]
```

Three values, no more. The accepted value is a **server-side task-allocation
lifetime** and the client never re-derives task eligibility. Drive uses the
creator team captured when a task is added; later membership changes do not
move old work, and project names are not allocation filters. On request, however,
the browser immediately presents the requested value as pending and retains it
across ordinary reads until the command settles. The same optimistic control
labels the submit button, so the operator sees the autonomy currently being
requested as well as the eventual authoritative value.

**Value:** this is the product's most important control — the knob between "I
approve each thing" and "go find work". The pending presentation makes the
operator's request immediate without pretending it has already settled.

### The team mode rail and its scroll ownership

```mermaid
flowchart TD
  R["mode rail: Info | Work | Chat"] --> P["selected pane replaces content"]
  P -- "Info or Work" --> X["exclusive mode content<br/>no message plane mounted"]
  P -- "Chat" --> C["one shared vertical budget"]
  C --> H["composer columns<br/>horizontal overflow"]
  C --> S["downward scroll<br/>moves cards and hides composer"]
  S --> U["upward scroll<br/>moves cards and reveals composer"]
```

Three views per live team appear on one rail, with the selected mode retained per
team. Selecting a mode directly replaces pane content; there is no slide
transition. Info and Work replace Chat completely and do not mount the message
plane. Chat's member columns own horizontal overflow. The browser retains native
ownership of every vertical wheel, touch, and keyboard gesture. The composer
overlays the top of the message plane, whose content starts below a spacer of
the composer's height, and moves exactly as far as the cards scroll until it is
fully hidden or revealed. It reads as part of the stream, never cancels the
event, and never writes the plane's scroll position. Home and End set the
corresponding composer exposure while reaching the native stream edge in one
command. The team toolbar and ticker remain fixed above both.

**Value:** the operator can turn the whole console into a reading surface
without a toggle, threshold, or animation. The composer remains mounted, so
drafts and controls return exactly as they were when the gesture reverses.

### The reading contract

```mermaid
flowchart LR
  N["new message"] --> I["insert only<br/>existing spans stay latched"]
  I --> F["frontier rises"]
  F --> S{"more than 80px<br/>from top?"}
  S -- yes --> C["jump + compensate same frame"]
  S -- no --> A["show the push-down<br/>(no position tween)"]
```

Newest-first masonry where a card that did not change gets **no style write at
all**. New content places by insert, never by repacking; a reader more than 80px
from the top is compensated in the same frame so the shift is invisible. The
0–80px near-top zone sees the push-down snap because there it is the signal.

**Value:** this is what "calm" actually costs, and it is the most exacting
requirement in the product. Reading many live agents at once is impossible if
the board reflows, so every reading-contract rule is load-bearing on its own.

### Status carries both tone and count

```mermaid
flowchart LR
  P["status pip<br/>per agent liveness"] --> L["saturation = recency"]
  G["lane lights<br/>per member in a team"] --> L
  F["task pills<br/>per project stem"] --> V["ready · active · unavailable counts<br/>(public and hidden alike)"]
  V --> T["base, active, or dormant presentation"]
```

Lane indicators use saturation for recency. Task pills never ask colour to
carry exact work state: their three-slot string spells ready, active or
in-flight, and blocked or deferred counts. A hidden-system stem sorts after
the public ones and reads the same breakdown, because a stem the allocator
skips is exactly the one an operator has to triage by eye.

**Value:** the operator gets fast peripheral liveness and exact work counts at
the same time. The three semantic slots are always present, but the unpadded,
content-driven pill may resize as digit widths change.

### Steering rendered inside the reply

```mermaid
flowchart TD
  A["assistant message"] --> Q["ACK / NACK quote block<br/>(what was asked)"]
  Q --> B["the response to it"]
  B --> NextQuote["next ACK / NACK quote"]
  NextQuote --> NextResponse["its response"]
```

Messages lay out in the agent's own order: leading prose, then each recognized
ACK or NACK as its steering quote followed by that attempt's response. The
resident consumes the marker before publication, and the browser message DTO
carries no disposition; both paths therefore render as ordinary Markdown with
the lane accent rather than a distinct decline colour.

**Value:** you never have to hold "what did I ask?" in your head while reading
the answer. In a fleet where you sent that instruction twenty minutes and four
lanes ago, this is the difference between reading and re-deriving.

### Three attention channels

```mermaid
flowchart LR
  E["EYE, the board<br/>pips, pills, mosaic"] --> OP["operator"]
  A["EAR, speech<br/>quiet | speak | narrate"] --> OP
  G["GLANCE, badges<br/>pending counts, mode rail"] --> OP
```

Deliberately three, with different bandwidth. Card speech reads the selected
message's exact full text; topbar speech reads the trimmed visible main-content
text, up to 32 KiB. Narration reads the full text of each newly cursor-admitted
message. Nothing present at first materialization auto-plays.

**Value:** the ear is the only channel that works while you are looking
somewhere else, which is what lets one operator hold more lanes than they can
watch. It is best-effort by contract — audio never blocks the stream, because
the transcript remains the record.

---

## Why it composes

Each guarantee is cheap alone and compounding together.

```mermaid
flowchart LR
  DurableSteering["durable steering"] --> WalkAway["walk away mid-instruction"]
  Claims["claims + TTL"] --> AddAgent["add an agent without a conversation"]
  AttestedMerges["attested merges"] --> TrustOutput["attribute landed output without<br/>rereading working commits"]
  DisposableAgents["disposable agents"] --> Renew["renew instead of hoarding context"]
  DerivedMemory["derived memory"] --> Renew
  CalmBoard["calm board"] --> WatchFleet["watch a fleet at once"]
  WalkAway --> S["supervision scales past<br/>what one person can read"]
  AddAgent --> S
  TrustOutput --> S
  Renew --> S
  WatchFleet --> S
```

The result of that composition is a supervision rate one person cannot reach by
reading: many tasks a day, independently reviewed whenever an eligible peer
exists, across a fleet whose ordinary capacity follows work without a
hand-managed agent count. Explicit operator-authorized lane wake remains
available when direct intervention is wanted.

---

## The posture

One disposition recurs everywhere, and it is the easiest thing to lose.

Cross-review is preferred but never required. Launch-time advancement is
attempted but never raises. Optional command optimization leaves the native path
intact when unavailable. Adjudication is optional at configuration time:
disabled judging publishes a configured trigger hit directly, while enabled
judging fails closed and publishes only a confirmed candidate.

Each of these is a good property the system reaches for and declines to depend
on. The natural implementation of every one of them is a hard rule — "no agent
reviews its own work", "refuse to start on a stale tree", "require the
optimizer" — and each hard version deadlocks something. A solo agent cannot
review. A lane with an unreachable remote still has local work to do. An
optional optimizer cannot become a prerequisite for native execution. An
unavailable configured judge withholds only the speculative counsel candidate;
it never blocks the agent's primary work and never widens into direct
publication.

**Prefer the good property; never let it stop the work.** Where that reads as
laxity, it is the opposite: it is what lets the strict properties elsewhere —
durability, exclusivity, atomic landing — be strict without ever stranding
anyone.
