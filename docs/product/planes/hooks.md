# Product model — supporting planes — Metrics and hooks

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Metrics

### Accounted transcript usage

```mermaid
flowchart LR
  T["durable transcript tape"] --> R["typed vendor decoder"]
  R --> U["normalized usage samples<br/>exact spans + event ordinal"]
  R --> N["complete records<br/>with no usage — count"]
  R --> X["residue records — count"]
  U --> B["one JournalIngested batch"]
  N --> B
  X --> B
  B --> H["TranscriptSourceHead<br/>source generation + next position"]
```

**Invariant —** Accounting retains each normalized usage sample beside the
exact ordered byte spans and event ordinal it was decoded from. A complete
record carrying no usage and a residue record remain visible as separate
counts; neither is promoted into an invented activity fact.

**Invariant —** Accounting consumes the typed vendor decoder's records and
events. It does not maintain a second private interpretation of vendor JSON.

**Invariant —** One accounted batch names its lane, vendor, logical stream,
source generation, starting position, and ending position. Its usage samples,
non-usage count, and residue count account for every record boundary the batch
crossed.

### Exactly-once source accounting

```mermaid
flowchart LR
  S["source generation +<br/>member/offset position"] --> C{"generation matches<br/>the stored source head?"}
  C -- yes --> R["continue at the exact next position"]
  C -- no, prior head --> Z["start at member 1, offset 0<br/>and name the superseded generation"]
  C -- no prior head --> O["start at member 1, offset 0"]
  R --> W["JournalIngested batch AND<br/>advanced source head = ONE write"]
  Z --> W
  O --> W
```

**Invariant —** Resumption is an exact source-generation and position
checkpoint, not a path and not an offset alone. A replacement under the same
path restarts from that generation's origin and explicitly names the generation
it supersedes.

**Invariant —** The accounted batch and the source head it advances commit in
one guarded journal transaction. A pass that does not commit leaves neither;
the continuation guard refuses gaps, stale heads, and implicit generation
changes before either value crosses.

## Hooks

### Two hook systems, deliberately distinct

```mermaid
flowchart LR
  G["GIT GATE HOOKS<br/>pre-commit, commit-msg, reference-transaction"] --> R["refuse or allow a repo mutation"]
  D["AGENT SHELL HOOKS<br/>wrapper, supervisor"] --> S["shape and steer the agent's own commands"]
```

**Invariant —** These are different systems with different owners and different
failure semantics, and the product says so out loud. Conflating them is a known
confusion the design specifically resolves.

### The command-driven supervisor and instrumented command boundary

The resident-owned supervisor sits between the vendor command stream and the
lane terminal. Steering reaches that stream from journal authority; shell
instrumentation remains responsible only for one-shot command execution and
private output relays.

```mermaid
flowchart TD
  J["journal standing is LIVE"] --> N["resident lane notification"]
  O["occupant starts"] --> C["supervisor catch-up"]
  N --> R["authenticated current-inbox read"]
  C --> R
  R --> D{"changed nonempty rendering?"}
  D -- yes, unheard key --> I["interrupt active host calls"]
  I --> S["interject on vendor command stream"]
  D -- yes, repeated key --> U["compose occupant-aware nudge"]
  U --> S
  D -- no --> W["wait for next notification"]
  A["agent's top-level shell command"] --> T["supervisor terminal"]
  T --> X["wrapper — ONE hop, exactly once"]
  X --> P["spawn one immediate command shell<br/>over private stdout/stderr relays"]
  P --> SRC["a SOURCED script<br/>shares this shell's functions"]
  P --> EX["an executed script, a child shell"]
  EX --> NAT["starts NATIVELY —<br/>the user's shell, no second interception"]
  P --> F["immediate child exits"]
  F --> V["revoke descendant relay writers;<br/>drain relays and finish"]
```

**Invariant —** Instrumentation is **one-shot, at the agent's own command
boundary**. The wrapper runs exactly once per agent command, and nothing beneath
it is instrumented again.

**Invariant —** The gate that guarantees "exactly once" is **structural, not a
counter**: the outer and inner stages are distinct packaged environments, so
there is no marker to read, forge, or lose. Descendants go uninstrumented
because they are pointed at the user's own startup files, not because they were
told to skip a step.

**Invariant —** A descendant process therefore gets the user's shell,
unmodified. A script that assumed a plain shell behaves exactly as it would
outside the agent, which is what makes leaving the instrumentation on safe.

**Invariant —** A *sourced* script is not a descendant, and shares the immediate
shell's functions and options. The boundary being drawn is the **process**
boundary, and sourcing does not cross it.

**Invariant —** Supervisor delivery reads journal authority. Startup performs a
catch-up read; each resident notification prompts another authenticated read.
Changed nonempty renderings are interjected on the vendor command stream,
identical bytes are suppressed, repeated keys receive occupant-aware nudges,
unheard keys interrupt in-flight commands, and compaction resets the
occupant-local telling state.

**Invariant —** The wrapper waits on that one immediate process identity while
private relays carry its output. The wait survives the child forking, waiting,
or replacing itself; when that identity exits, the wrapper revokes inherited
relay writers, drains committed output, and finishes even if a background
descendant remains alive.


**Invariant —** The complete top-level command string is handed to the wrapper
exactly once. That is what makes rewriting possible at all: it is the last point
at which the command still exists as one string rather than as a running
process.

#### Shell-command advice

Leading `cd [--] ABSOLUTE_PATH` followed by `&&`, `;`, or a newline receives
succinct advice when the literal path already equals the command's admitted
pre-execution working directory. Quoting and escaped spaces are understood;
relative paths, expansions, redirections, pipelines, directory-changing options,
and unknown or overridden hook working directories remain silent. Symlinks are
not resolved to equate different logical directory spellings.

OMP attaches the advice only to the initial host-shell response seen by the
agent. It is not stored in job notices, replayed by completion or log reads, or
sent as an operator notification. Advice never changes the command, permission
decision, child output, or exit status, and never invokes RTK.

A final `head` or `tail` pipeline stage receives a gentle hint that it is
probably unnecessary: rely on the tool's native output wrapping instead of
arbitrarily pre-truncating unknown output. The hint names whichever of the two
ends the pipeline. This advice has no native-capability, flag, or
working-directory gate. A standalone `head` or `tail`, an intermediate pipeline
stage, and quoted or escaped pipe characters do not trigger it. Both hints can
appear together; neither rewrites the pipeline or claims an equivalent
transformation.

Each hint appears once on every matching command invocation. Neither session
identity nor compaction suppresses later matching commands; no delivery receipts
or repository state are needed. Nonmatches remain silent. These are
command-attached suggestions, not narration prods: OMP's progress prod is a
separate injected system notice that omp delivers at its next tool or turn
boundary without interrupting a pending tool call, while shell advice
accompanies that call's initial response. No ACK, escalation, or
periodic reminder is attached to either hint.

Spice's OMP shell and job results show at most 16 KiB of log text, preserving
UTF-8 character boundaries. When bytes are omitted, the native wrapper includes
the path to the complete keeper-owned log. Use `read` on that path with a line
range to recover earlier output or inspect the end. Foreground results,
background snapshots, completion delivery, and later job reads share this
preview. Small output has no additional recovery metadata. The complete log is
not an OMP artifact of an already-truncated payload: it retains the command's
original captured bytes.

#### Active-work narration

The resident owns one narration streak per current occupant. The configured
`lane.narration` bound remains 24 tool calls by default. A monotonic deadline
also makes the first reminder due after three minutes without visible assistant
prose, and the second reminder due by five minutes. Calls and elapsed time share
the prod ordinal: reaching both thresholds does not send the same reminder
twice. Further call-count bounds retain their existing repeat behavior.

Turn start, assistant prose, reasoning, or a tool call establishes active work.
Prose resets its clock and call count; explicit turn completion stops the clock.
Tool results, an outstanding shell command, and `job.wait` do not reset it.
The resident sleeps until the next exact deadline in its existing event loop;
there is no periodic narration poll. A moved or exited occupant cannot receive
a successor's reminder.

Each due reminder commits an occupant-guarded `narration::prodded` fact. OMP's
existing supervisor command channel then interjects a progress notice and
interrupts outstanding tool waits, **not their underlying commands**. The
second prod asks for progress as overdue. No ACK key is minted or required.

The guarantee is deadline-driven recording and an immediate best-effort local
notification attempt while the resident is serving, subject to event-loop and
OS scheduling. An unavailable supervisor channel or a hung model can delay
receipt or prose. The three/five-minute targets do
not claim the ability to force a model to speak or justify cancelling healthy
work.

#### Cadence is reachability

A live directive is rendered by the command-driven supervisor, which catches
up at occupant startup and reacts to resident notifications while the turn is
active, interrupting outstanding tool waits for an unheard key.

```mermaid
flowchart TD
  D["live steering in journal authority"] --> C["command-driven supervisor"]
  C --> SR["authenticated startup or kick read<br/>plus occupant-local dedupe"]
  SR --> O["observation only;<br/>durable lifecycle stays live"]
  D --> N["no occupant is running"]
  N --> W["delivery waits;<br/>durable lifecycle stays live"]
  C --> BG["the agent backgrounds the work<br/>and ends its turn"]
  BG --> S(["the resident vacates the lane;<br/>the detached keeper continues"])
  S --> X["the command ends unheard<br/>through the delivery grace"]
  X --> RL["the keeper requests a lane wake;<br/>startup catch-up delivers the completion"]
```

**Invariant —** Supervisor cadence is delivery cadence, not shell-command
cadence alone. Non-shell work remains reachable because an unheard key
interrupts outstanding tool waits. If no supervisor is running, the directive
remains live without fabricating a lifecycle transition; the next occupant's
startup catch-up delivers it. The supervisor deduplicates within the occupant
and compaction epoch.

**Invariant —** Every turn exit vacates the lane, including exit code zero. A
backgrounded command survives under its detached keeper. If a non-stopped
ending, or an event of a command started with `events`, remains unheard
through the delivery grace, the keeper requests a lane wake; a successor
receives the unheard events and then the completion during startup catch-up.
A lane whose active claim outlives the turn keeps that claim across the
vacancy: while its deadline-bounded job runs, the held claim waits for that job
rather than reseating the lane at once, though pending operator steering still
seats it, and the keeper's wake, or the job's admitted expiry when no wake
comes, seats a successor holding the same claim.

**Invariant —** This is lifecycle behavior, not an instruction to keep a turn
open. An agent holding a claim may end its turn while a deadline-bounded job
runs and is woken, with its claim, when that job ends or emits an event. It
uses `job wait` when a finite job's result blocks current work; ending the
turn neither cancels the job nor loses its completion. A service is
different: it never ends on its own but runs while its task claim remains
active, so it never keeps a lane idle, and `job wait` answers at once with its
output so far and leaves it running; the claim's end, or an earlier
`job stop`, ends it. Read its logs or exercise its served surface to establish
readiness. Every seat start, fresh or resumed, names the lane's kept commands
that still run, marking services, so a forgotten service is surfaced to the
next seat rather than left unseen.

**Invariant —** A job's ending is told once. An answer that reports how a
job ended (the foreground result, or `job wait`, `job logs` or `job stop`
after it ended) records it as heard, so no completion notice follows and no
successor's catch-up repeats it. The record is claimed atomically before a
notice is written, so when an answer and a notice race exactly one claims it
and the agent always hears the ending at least once. A seat whose input has
closed claims nothing, and a notice that fails to write gives its claim back,
so the next seat's catch-up still owes the ending.

#### Acknowledgment evidence

The transcript tape is the one place an acknowledgment is read from. A header
is credited when the tape holds it as assistant text; nothing a tool call
carries about itself can stand in for that text, because a command that
witnessed its own key would retire direction nobody answered where the
operator can read it.

```mermaid
flowchart TD
  H["ACK or NAK header"] --> W{"where does it reach the tape?"}
  W -- "interjected vendor assistant text" --> M["resident retires the key<br/>as prose lands; no command"]
  W -- "tool input or reasoning only" --> A["not acknowledgment evidence;<br/>the key stays live"]
  W -- "assistant text ending the turn" --> X["retired on landing or at exit<br/>from the final transcript"]
  W -- "quoted or narrated in assistant text" --> Q["quoted mentions produce steer::ack-missed;<br/>the key stays live"]
  W -- "a placed header naming a key<br/>the occupant does not hold" --> U["one private notice names the token<br/>and lists the pending keys"]
  A --> N["speak the header as visible text,<br/>or let the turn end on it"]
  Q --> N
  U --> N
```

**Invariant —** Assistant text on the lane tape is the sole acknowledgment
evidence. Tool calls, their arguments, and reasoning blocks never retire a
key, even when they spell the header exactly.

**Invariant —** The resident retires the key. It recognizes the visible
assistant header as prose lands; no acknowledgment command exists. At occupant exit,
the resident also retires keys answered in the final transcript under the same
recognition rules, with no successor woken to cite them. Visible commentary
counts when persisted as assistant text; reasoning does not, regardless of how
the harness labels or displays it.

**Invariant —** Within one assistant prose the placed header decides its key.
A later citation of the same key in backticks or mid-sentence is the prose
talking about the header, not a retraction of it. A prose that only quotes or
buries a live key is refused as `steer::ack-missed` naming that miss and the
repair, leading a new line with the header; the live witness the lane cited has
not moved, so the refusal never sends it to reread a snapshot that shows the
same key.

**Invariant —** A placed header naming a token the occupant does not hold
retires nothing and is corrected once. The token is neither pending, nor
successfully delivered to this occupant, nor the lane seal: a mistyped key, or
one retired before this occupant heard it. The supervisor privately names that
exact token, says it acknowledged nothing, and lists the pending keys. A
successfully delivered key stays recognized for the occupant's lifetime after
retirement and compaction, so a closing reply may repeat its header without a
false correction. Failed delivery grants no recognition, and another occupant
inherits none. Context-local telling state still resets on compaction so pending
direction is replayed in full. The notice is a delivery correction, not journal
authority, and an unknown token is corrected again only after a compaction.

### Command rewriting

The rewriter is an **optional companion tool**, not part of the harness. The
selection logic lives on its side; what follows is the contract spice depends on
and the guarantees spice itself provides around it.

```mermaid
flowchart TD
  C["agent's native command"] --> R["optional rewriter"]
  R --> E{"outcome"}
  E -- "rewrite offered" --> W["use the rewrite"]
  E -- "clean no-match" --> N["run the original unchanged"]
  E -- "malformed or unavailable" --> D["diagnose, discard, run the original"]
```

**Invariant —** The rewriter can only ever *help*. Any malformed or unexpected
outcome falls through to the native command unchanged, with a bounded
diagnostic. *(The outcome shape that distinguishes these cases belongs to the companion.)*

**Invariant —** Health reporting is **non-blocking**. An unusable rewriter
degrades loudly and the agent keeps working.

**Invariant —** One trusted executable identity resolves once; every probe and
rewrite invokes exactly that identity. Resolution does no availability lookup.

### Attribution

```mermaid
flowchart LR
  D["vendor's native attribution"] --> P{"repo blocks the trailer?"}
  P -- yes --> O["disable native attribution to match"]
  P -- no --> T["trailer passes through"]
```

**Invariant —** Trailer policy is the repository's. The harness makes the vendor
agree with it up front rather than contradicting it at commit time.

---
