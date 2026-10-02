# Product model — supporting planes — Session forensics

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Session forensics

Compaction is the defining constraint. An agent's context is finite and its
history is not, so everything an agent knows about its own past is a *derived*
artifact — and the quality of that derivation is the difference between an agent
that resumes and one that restarts.

### The briefing

Answers, mechanically, the four questions a freshly compacted or freshly renewed
agent must not guess at.

```mermaid
flowchart LR
  WhatWasAsked["what was asked?"] --> B[briefing]
  WhatWasLastDelivered["what was last delivered?"] --> B
  WhatWasTheWorkingSet["what was the working set?"] --> B
  WhatSteeringIsPending["what steering is pending?"] --> B
```

**Invariant —** Asks are sourced from the **steering plane**, not from
transcript user messages. Transcript evidence contributes only the
what-happened class; it never supplies or backfills the ask, and no cutoff is
inferred from directive recovery or publication.

**Invariant —** Recovery shows the *recovered ask*, parsed from compaction
intent — never continuation boilerplate, never an assistant-text fallback
rendered as if the user said it.

**Invariant —** The asks section is **omitted when empty**. A briefing with
nothing to say says nothing rather than emitting a mantra.

### Which plane owns which answer

The transcript is the richest source available, and it is authoritative about
exactly one class of fact: what happened. It is authoritative about nothing that
is *wanted*.

```mermaid
flowchart LR
  subgraph WHAT_IS_WANTED["what is wanted"]
    SP["steering plane"] --> AS["outstanding directives<br/>each with the key that answers it"]
  end
  subgraph WHAT_HAPPENED["what happened"]
    TR["transcript"] --> EV["deliveries, commands, touched paths<br/>failures, compaction boundaries"]
  end
  subgraph WHAT_IS_HELD["what is held"]
    BD["board"] --> CL["the actor's own claim<br/>peers as a count"]
  end
  AS --> B[briefing]
  EV --> B
  CL --> B
```

**Invariant —** The transcript never supplies the ask. Reconstructing intent
from prose reads the agent's own restatement of a directive back to it as if it
were the directive.

**Invariant —** A directive is shown only for the thread being briefed, and
carries the key needed to answer it. A directive an agent cannot retire is a
reminder rather than a request.

**Invariant —** A pending directive uses the publication instant recorded in its
authoritative steering head. If the authoritative head set is invalid, the
steering probe is unknown; the briefing does not silently drop or restamp one
directive.

**Invariant —** Only the actor's own claim is enumerated; peers' active work is
a count. The briefing orients one agent, and another agent's task is not this
agent's business.

### Task creation deep links

A task retains the transcript boundary at which it was created. For a
lane-authored task, admission binds the authenticated lane to its current
occupant, causal vendor session, and exactly one accounted lane/vendor source
head. The immutable context carries session identity, lane, vendor, logical
stream, source-generation UUID, and the next complete-record member/offset.
`task show` renders those values as a shell-executable `spice session context`
command; operator- and Spice-authored tasks instead render a stable unavailable
reason.

**Invariant —** Creation context is written on `TaskAdded` and copied to every
statement projection. Edits, workflow transitions, claims, reviews, completion,
and later source-head movement cannot rewrite it.

**Invariant —** Rehydration reads only the current repository's retained
lane/vendor tape, verifies the exact source generation and complete-record
boundary, and returns a bounded surrounding window. A missing, replaced,
truncated, undecodable, or mid-record source refuses by name rather than
searching another transcript.


### Task lifecycle recovery links

An active task also retains the exact `ClaimWon` fact that opened its current
attempt. `task show` renders that fact's lane, occupant, epoch, accounted source
boundary, and registered vendor session as a second shell-executable
`spice session context` command. Claim carry and transcript continuation do not
move this opening boundary. If the source is incomplete, the registered stream
is absent or ambiguous, or no matching session declaration exists, the task
renders the specific unavailability reason instead of inventing a lookup.

The same compact view points to `spice doc ledger HANDLE` for current authored
rationale and append-only notes. When a claim, completion, or review records a
Git boundary, the task renders its fact identity and a shell-executable
`git show` command for the recorded claim head or integrated object. Git and
transcript content remain in their native authorities; task rows carry only
provenance and recovery coordinates.

### Acknowledgment provenance links

Every task or claim fact carries a causal origin, and an `ack:KEY` origin names
the steering directive a lane acknowledged when it did the work. Operator
steering a lane acknowledges while it holds a task, in any phase, joins that
task's record through its `steer::acked` fact, unless another task cites the
key as its origin: that ask became its own work. Peer messages and resident
notices never join. `task show` lists each distinct key on the record, the
task's own origin first, as one `acknowledgment ack:KEY` line naming every
fact that carries it, and marks each note written under a key with that key.
A claim answer resolves the same keys, so the next phase's holder and the
reviewer receive the steering that shaped the work. Each key then resolves to
its directive: identity, addressed lane, authoring actor, publication instant,
verbatim text, and attachment names. An acknowledged or declined directive
also renders the answering lane and instant plus a shell-executable
`spice session context` command at the answering transcript record, naming the
session that lane declared on that exact tape generation at or before the
record. That window holds the delivered ask and the lane's answer; the task's
own creation context then continues the trail. A live, lapsed, canceled, or
dead-lettered directive, a legacy locus without a tape generation, or a tape
with no prior declaration renders its specific unavailability reason instead
of guessing a session.

### The briefing

A briefing answers one question: what binds this lane now. It carries pending
asks, this lane's claim and held-work posture, current repository and
environment posture, typed unknowns, and the minimal live working set: the
newest turn's counts and clipped prompt, the latest final answer, the newest
compaction instant, and the exact `session sweep` and `session turns` commands
that read everything else. It never replays completed turns or their tool
traffic, however few compactions precede them.

**Invariant —** Pending asks are their own bounded page (16 rows, 16 KiB,
1 KiB of text per ask); when asks are withheld the page names
`spice inbox LANE` as its continuation. An unread steering ledger omits `asks`
and reports a typed unknown, so a failed read never looks like an empty inbox.

**Invariant —** `residues` in a briefing is a count. The residue detail is
carried by the history views, which share the same fold.

**Invariant —** A briefing or sweep accepts only a positive finite whole-action
deadline shared by source discovery and report construction. Recoverable
authority-read, configuration, journal, and board errors become typed visible
unknowns and construction continues. If any work hangs past the deadline, the
action refuses with `session::deadline-exceeded`, names the action, requested or
resolved inputs, and budget, and returns no partial report.

**Invariant —** An unavailable authority value remains absent, distinct from
numeric zero. Its typed unknown names the authority, probe, and error detail;
Rust does not invent a clean measurement for an unavailable read.

### History pages

`sweep`, `turns`, `messages`, `phases`, `timeline`, `slices`, `commands`,
`context`, `compactions`, `commits`, and `summary` each return one bounded page
through the shared output admission.

```mermaid
flowchart LR
  S["scan: newest records at or<br/>before the cursor"] --> A{"next record fits<br/>rows and bytes left?"}
  A -- yes --> K["admit, charged its<br/>encoded length"] --> A
  A -- no --> O["close the page; name the<br/>exact continuation"]
```

**Invariant —** Every page defaults to 20 rows, 8 KiB of encoded records, and
240 bytes per text field; `--limit`, `--max-bytes`, and `--max-text` raise those
up to 500 rows, 128 KiB, and 4 KiB, and a larger request refuses with
`session::bound-exceeded`. Each record is charged its full encoded length, so
no single record bypasses the aggregate byte limit.

**Invariant —** Records list newest first: compaction windows for `sweep`,
turns, messages, commands, compactions, commits, timeline events, and slices by
their newest instant. `phases` lists in phase order; `context` lists nearest to
the task-creation boundary first.

**Invariant —** A page that withholds records carries an omission with the
withheld count and the exact command that resumes at the first withheld record:
the caller's words plus `--after CURSOR`. A timestamped cursor is that record's
instant, with `#N` when N records sharing it were already shown, so records
arriving later never shift a page boundary. `phases` resumes at `phase-N` and
`context` at `before-N` or `after-N`.

**Invariant —** A record too large for the remaining bytes but within the
maximum closes the page, and its continuation raises `--max-bytes` to what that
record alone needs. A record beyond the interactive maximum continues only with
`--export FILE`, which writes the complete page under the repository ceiling
with a digest receipt; a record beyond that ceiling refuses with
`session::record-exceeds-ceiling`.

**Invariant —** A sweep row summarizes one compaction window (activity counts
and its latest final answer, never its dialogue) and names the `session turns`
command bounded to exactly that window's turns.

**Invariant —** Recency is measured against the newest activity actually
observed in the material, not against wall-clock now. A transcript examined a
day later has the same shape it had when it was written.

**Invariant —** Windows **tile contiguously** through the scanned horizon. A
session that never compacted still yields exactly one window, labelled by its
missing compaction anchor.

**Invariant —** Only a compaction that actually severed context is a boundary.

**Invariant —** An uncompressed seekable transcript discovers boundaries
**backward from the end** and stops as soon as it has enough, so its discovery
cost scales with the page asked for. A gzip transcript cannot reverse-seek;
its admitted path scans forward and retains only the newest needed markers.

**Invariant —** Dialogue inside each turn retains the decoder-admitted
transcript order; displayed text collapses whitespace runs and is clipped with
its withheld byte count stated.

**Invariant —** Unknowns are **loud**. A window the engine could not read must
not look like a window with nothing in it.

### The unit of the record

Forensics needs an atom. The atom is one operator ask, and everything observed
is attributed to whichever ask was open when it happened.

```mermaid
stateDiagram-v2
  [*] --> Preamble : activity before any ask has opened
  [*] --> Open : an ask arrives
  Preamble --> Open : an ask arrives
  Open --> Open : assistant text, tool call, error, compaction
  Open --> Open : a different prompt identity — a NEW record begins
  Open --> Complete : the transcript STATES the exchange ended
  Open --> Unterminated : the transcript ends without saying so
  Complete --> [*]
  Unterminated --> [*] : extent taken from its last observed activity
  note right of Unterminated
    a window containing an
    incomplete exchange is
    itself reported as open
  end note
```

**Invariant —** Activity observed before any ask has opened gets a record of its
own rather than being discarded. Startup work is work.

**Invariant —** An exchange is complete only when the transcript **states** that
it ended. Inferring completion from the end of the file makes every truncated
transcript look finished.

**Invariant —** A record preserves the original interleaving of operator
messages, assistant commentary, and final answers — not just separate buckets.
Order is most of the meaning.

**Invariant —** Records merged from several transcripts order by time with
source identity breaking ties, and the differing timestamp spellings of
different vendors compare as the same instant. Identical inputs always yield an
identical sequence.

**Invariant —** Recorded text is exactly what the transcript held; only
explicitly derived fields are reformatted. A transcript's display echo of a
message is not a second occurrence of it, and a final answer restated at close
is recorded once.

**Invariant —** An invocation and its outcome are **separate facts**. A command
whose outcome was never observed is neither successful nor failed, and an
outcome attaches to the invocation it names rather than to whichever one ran
most recently.

**Invariant —** A fact whose classification disagrees with the evidence backing
it **aborts the projection**. A degraded record is worse than no record, because
it is indistinguishable from a good one.

**Invariant —** Requesting `--turn-id` against a nonempty dataset whose turns
uniformly lack identities is capability-refused, naming `--start`, `--end`, and
`--contains` as workable alternatives. Ordinary nonmatching filters return an
empty selection.

### Message-shape classification

```mermaid
flowchart LR
  M[message] --> D["deterministic classifier"]
  D --> A[genuine ask]
  D --> S[scaffolding]
  D --> C[compaction summary]
  D --> T[tool output]
```

**Invariant —** Classification is deterministic and structural, never a prefix
sniff. Everything downstream — briefing, maxims, and metrics — depends on
telling authored prose from generated text.

**Invariant —** Every operator-role message gets **exactly one** shape, decided
from that message's own text alone, so the same text always classifies the same
way regardless of where it appears.

**Invariant —** Rehydration summaries, harness retry nudges, session scaffolding,
background-task notices, and bootstrap prompts are generated text, never
authored prose. Text consisting wholly of one machine envelope is generated even
when the envelope's tag is unrecognized — the shape is the evidence, not the
vocabulary.

**Invariant —** An intent that cannot be recovered from a rehydration summary is
represented by an explicit marker, distinguishable from a genuinely empty
intent.

**Invariant —** Judge text and maxim scanning **exclude** generated diff, patch,
and tool-output bodies. The system judges what the agent wrote, not what it read.

### Resolving the subject

```mermaid
flowchart TD
  I["input"] --> E{"exists on disk?"}
  E -- yes --> P["read it as that thing"]
  E -- no --> N["normalize as a session identifier"]
  N --> F["try each compiled vendor,<br/>current context first"]
  F --> R{"resolved?"}
  R -- yes --> T["transcript"]
  R -- no --> X["refuse on the first failing<br/>explicit input in caller order"]
  Z["no subject named"] --> A{"captured live vendor<br/>identity present?"}
  A -- yes --> S["first live vendor identity's<br/>running transcript"]
  A -- no --> C["all sorted admitted .jsonl<br/>and .jsonl.gz in cwd"]
  C -- none --> X
```

**Invariant —** Explicit subjects resolve in caller order. The first failing
input refuses the request rather than producing an empty report; successfully
resolved canonical paths contribute once even when named repeatedly.

**Invariant —** With no subject, resolution uses the first captured live vendor
identity when present. Otherwise it admits every sorted `.jsonl` and
`.jsonl.gz` in the current directory, refusing only when no admitted source
resolves.

**Invariant —** An agent identity counts as current only while it is in the
running process's environment. An identity merely persisted in the tree from an
earlier session is history, not context.

**Invariant —** Every spelling of an identifier normalizes to one canonical
form, and a transcript named more than once contributes exactly once.

### The working-state meter

```mermaid
flowchart LR
  C["agent runs a command"] --> S["snapshot live state"]
  S --> R{"durable state key changed?"}
  R -- no --> Q[suppress]
  R -- yes --> W["record the current key,<br/>durably"]
  W --> V{"visible fact to report?"}
  V -- no --> Q
  V -- yes --> E["one short sentence on stderr"]
```

**Invariant —** One line, high signal, repeat-suppressed, on the agent's own
command stderr. It rides a channel the agent already reads.

**Invariant —** It reflects **actual state**. A constant encouragement is worse
than silence: it trains the reader to skip the channel.

**Invariant —** The record of the last reduced state survives process exit, so
identical state remains silent across short commands until its durable key
changes. A quiet renewal is still recorded, allowing a later countdown cycle to
announce its boundaries once.

**Invariant —** The countdown band of an expiring claim is **part of the
identity** of what was displayed. Command-start evaluation speaks when elapsed
time has moved the claim across the 599-, 299-, or 59-second boundary, and a
cancellable timer targets only the next boundary while the supervised command
remains active. The final band schedules no further refresh; the timer stops
with the command and never launches one.

**Invariant —** When the record of the last reduced state is missing, unusable,
or timestamped later than now, the meter reports any visible fact. Suppression
is a claim about what the reader already knows, and an unusable record supports
no such claim.

**Invariant —** Failing to collect working state degrades to silence and never
changes the outcome of the command being decorated. A status line that can break
a command is not a status line.

**Invariant —** A meter with nothing true to report emits nothing, rather than an
empty frame.

### One turn through the plane

The live path persists intent before attempting delivery, then the
command-driven supervisor interjects it into the agent's command stream.

```mermaid
sequenceDiagram
  autonumber
  actor OP as Operator
  participant ST as Steering plane
  participant SV as Supervisor
  participant AG as Agent
  participant TR as Transcript

  OP->>ST: post an ask
  ST->>ST: persist BEFORE any delivery attempt
  ST->>SV: resident kick
  SV->>ST: authenticated inbox read
  ST-->>SV: one readout
  alt readout is new
    SV->>AG: interrupt tool waits and interject
    AG->>TR: ACK or NAK in prose
  else readout would repeat
    SV-->>AG: suppress delivery
  end
```

Renewal reconstructs the briefing from durable asks and transcript evidence.

```mermaid
sequenceDiagram
  autonumber
  participant AG as Agent
  participant BR as Briefing
  participant ST as Steering plane
  participant TR as Transcript

  Note over AG,TR: context fills — the vendor compacts or renews
  AG->>BR: what was asked, delivered, touched, pending?
  BR->>ST: outstanding asks — the primary source
  ST-->>BR: durable asks
  BR->>TR: recent windows + recovered compaction intent
  TR-->>BR: ordered verbatim evidence — unreadable input flagged loudly
  BR-->>AG: briefing — omit the asks section when empty
```

Ordinary command feedback is repeat-suppressed and observational.

```mermaid
sequenceDiagram
  autonumber
  participant AG as Agent
  participant MT as Working-state meter

  AG->>MT: run a command
  MT-->>AG: one line when the durable state key changes
```

---
