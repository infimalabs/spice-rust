# WIRE

One current DTO schema crosses the socket, rests in the journal, and
compiles into the browser app. The `wire` crate owns the types. Every
consumer uses its decoder.

## Envelope

A fact is an internally tagged envelope:

```json
{
  "v": 1,
  "seq": 48213,
  "at": "2030-04-12T18:44:09.271Z",
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2",
  "origin": "task:SPICE-aBcD",
  "cause": 48190,
  "fact": {
    "kind": "task::phased",
    "task": "SPICE-aBcD",
    "phase": "verify"
  }
}
```

| Field | Contract |
| --- | --- |
| `v` | Current wire version. Writers emit exactly one version. |
| `seq` | Monotone journal position assigned by the committing transaction; it is the durable observation cursor. |
| `at` | Commit instant in RFC3339 UTC with millisecond precision and a trailing `Z`. Identifiers never double as instants. |
| `actor` | Principal that caused the append: `operator`, `spice`, or `lane:<id>`. The resident derives it from the accepted capability; client text cannot choose it. |
| `origin` | Root attribution carried through the work, such as an ACK key, task handle, lane id, or repository revision. The resident validates the authenticated principal's right to establish or inherit it. |
| `cause` | `seq` of the immediately provoking fact. It is absent at a chain root; zero and null sentinels are invalid. |
| `fact.kind` | Discriminant from the closed vocabulary below. A kind is a path of single words and always appears as a field, never as an object key. |
| `fact.*` | Kind-specific body. Its DTO is registered with the kind and admitted only when at least one fold declares it. |

`seq`, `at`, `actor`, and any inherited `origin` are assigned inside the
commit door. A client may propose domain data, but it cannot mint journal
position, time, attribution, or causality.

## Hello

Every socket begins with:

```json
{
  "wire": 1,
  "capability": "018f82d4-6f59-7d06-92e3-bce9131163c6",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
}
```

`wire` is the only accepted socket schema. `capability` authenticates an
operator connection or a lane connection and fixes the principal from
which actor, origin rights, and lane are derived. `epoch` is the resident
generation the client believes it is addressing. All three fields are
required. A wire mismatch refuses by naming both client and resident
versions. An epoch mismatch faults by naming both epochs.

Every UUID crossing a type boundary has one spelling: lower-case
`8-4-4-4-12`. Readers refuse braces, upper-case letters, compact text,
and every other spelling.

## Commands

A mutating request crosses the socket in one command envelope:

```json
{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "kind": "task::done",
  "expected": [
    {
      "kind": "task",
      "task": "SPICE-aBcD",
      "head": 48168
    },
    {
      "kind": "workflow",
      "task": "SPICE-aBcD",
      "head": 48169,
      "phase": "verify"
    },
    {
      "kind": "claim",
      "task": "SPICE-aBcD",
      "head": 48172
    },
    {
      "kind": "lease",
      "task": "SPICE-aBcD",
      "head": 48173
    },
    {
      "kind": "residency",
      "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
    }
  ],
  "body": {
    "task": "SPICE-aBcD",
    "validation": "cargo test --workspace"
  }
}
```

| Field | Contract |
| --- | --- |
| `v` | Current wire version. |
| `id` | Client-minted canonical UUID. The resident binds it to the authenticated principal and canonical request bytes. |
| `kind` | Closed command discriminant. It selects the body DTO, witness schema, transition, and reply DTO together. |
| `expected` | Named predicates over only the subjects on which this command relies. Order has no meaning; canonical encoding sorts them. A global journal `seq` is forbidden here. |
| `body` | Kind-specific intent. It contains no caller-authored actor, lane, epoch, journal position, result, or fact envelope. |

The command registry carries the witness law with the command, so a new
command without an explicit row does not compile:

| Command | Required named witnesses |
| --- | --- |
| Add a task | Intended handle is absent and every dependency handle exists at its named workflow head. |
| Edit a task statement | Exact task statement head. |
| Append a task note | Task identity; no statement revision witness, because notes append rather than replace. |
| Defer or wake a task | Exact workflow head and current defer disposition. |
| Allocate or win a claim | Exact task statement and workflow heads, current claim head, eligibility rule, lane occupancy, and resident epoch. The eligibility query and the win share the transaction. |
| Carry, renew, free, or lapse a claim | Exact claim head, lease head, bound lane and occupant, and resident epoch. A successor may carry a proven ancestor token but may not renew it. |
| Phase, finish, or review a task | Exact task statement and workflow heads, winning claim head, live lease, assignment or review witness where required, bound lane and occupant, and resident epoch. |
| Assign review | Exact workflow and claim heads plus the anti-self-review witness. |
| Apply a document | Exact normalized preview digest and one named board barrier containing the task and workflow heads the preview read. |
| Send, dispose, or cancel a directive | Exact directive disposition; resend also names the preceding directive in its lineage; cancel additionally requires operator capability. A disposition's evidence is a transcript locus or the authenticated lane command that carried it, and the fact records which. |
| Wire a dependency edge | Exact task statement and workflow heads of both tasks and the absence of the edge; no board barrier — one edge is direct intent. |
| Decide consent | Exact request disposition, named rule, and budget predicate when a rule spends or renews a budget. |
| Pend, grant, or revoke trust | Exact executable digest and its current trust state. Operator capability is additionally required for grant and revoke. |
| Seat or change lane state | Exact residency head, lane and occupant identities when present, and resident epoch. |
| Record a taste verdict | Exact bag, evidence locus, and tracked taste configuration digest. |
| Recognize canary, vendor, commit, proof, or publication evidence | Exact observation key and the current head that makes recognition idempotent; publication also witnesses the sealed proof and canonical manifest digest. |
| Change resident state | Exact residency head, current epoch, and the deadline or completion predicate admitting the requested edge. |

Witness failure returns `Refusal` with the predicate, expected value,
observed value, and remedy. Unrelated journal traffic cannot lose a race
because it is not witnessed. A global `seq` appears only on a
whole-snapshot read barrier.

One guarded journal transaction authenticates the principal, binds and
deduplicates `id`, validates epoch and every witness, stores the canonical
request and evaluated witnesses, appends every fact, updates every
affected head, advances the wake cursor, and stores the result. The
transaction either commits all of those effects or none. If the reply is
lost after commit, a retry of the same `id`, principal, and canonical
request replays the stored result. Reuse of an id for different bytes
faults as an id collision.

One operation stands outside this door by declaration: restore replaces
the journal itself, so it cannot record its own identity in the store it
exchanges. It runs as the lock-held recovery protocol named in DISK —
digest-pinned plan, admission stopped, authority closed, atomic
exchange, reopen and verify, new epoch — and the reopened journal then
recognizes `journal::restored`. Nothing else may claim that exception.

Every reply names the command id and resident epoch and carries one tagged
outcome:

```json
{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "outcome": {
    "kind": "reply",
    "seq": {
      "first": 48213,
      "last": 48214
    },
    "body": {
      "task": "SPICE-aBcD",
      "status": "done"
    }
  }
}
```

| Outcome | Payload | Exit |
| --- | --- | --- |
| `Reply` | Typed result and its journal relationship: a committed inclusive `seq` range, a read-only snapshot `cursor`, or `direct: {}` for a daemonless answer with no journal position. | `0` |
| `Refusal` | Stable reason, failed witness when applicable, observed state, and actionable remedy. It carries no committed range. | `2` |
| `Fault` | Named protocol, store, or environment defect, including stale epoch and id collision. | `1` |

## Refusal

A refusal is an answer about an event that did not occur, never a fact.
It causes no journal or head mutation. The bus writes a bounded telemetry
row to the `bus::refusals` ring in the folds store with command kind,
actor, time, reason, failed witness, remedy, and resident epoch. At
read, the chronology view joins these rows as a declared source without
granting them `seq`, `origin`, or `cause`. Retention is a fixed row
bound per reason and actor; eviction changes no authoritative answer.

The ring is the one content in the folds store that no replay can
re-derive: it is written at refusal time and nowhere else, it is
excluded from backup with the rest of the folds database, and deleting
or replaying the store loses it. That loss is accepted and stated here
rather than hidden behind "replay heals it" — a telemetry-store fault
can never mutate or roll back the authoritative command outcome, and
the journal remains the record of the world.

A rejected hermetic check is likewise a refusal. A fault remains a fault:
neither outcome is promoted into history merely to make it visible.

## Authority

The journal is the only authority. One SQLite transaction owns immutable
facts, command identity and result, the wake cursor, and the head rows
that answer current state:

| Head | Authoritative content |
| --- | --- |
| Task | Handle, UUID, title, project, tags, dependencies, provenance, and statement revision. |
| Workflow | Flow, current phase, status, review state, assignment, deferral, and completion. |
| Claim | Winning actor, winning fact, bound lane, occupant, resident epoch, and active disposition. |
| Lease | Start, renewal, maximum, expiry, and the resident epoch that may advance it. Instants are UTC; the waits they arm run on monotonic clocks, re-derived from these heads at every epoch mint. |
| Witness | Named assignment, review, directive, consent, budget, document barrier, and transition predicates that later commands must re-prove. |
| Trust | Executable digest and `pended`, `granted`, or `revoked` state. |
| Finding | Current gate-finding posture: identity, severity, evidence locus, owner, originating receipt, and open or cleared state. |
| Residency | Resident epoch and state, lanes, occupants, lineage, capabilities, and their current dispositions. |
| Cursor | Last committed journal `seq` and the durable wake position used to heal missed notifications. |

Board reads query task, workflow, claim, lease, and witness heads directly,
then rank and pack the answer. There is no board fold. Thread is a
journal-maintained index over `seq`, `origin`, and `cause`, not a second
history. Folds may read named heads but can neither amend them nor answer
an authority question.

The only public commit capability is `JournalCommit`, constructable
inside the bus. `JournalRead` observes. `JournalGuard` admits a
transaction. Board logic contributes typed precondition closures into
that guarded transaction and never holds the writer.

## Crossing

An observation crosses into a fact only as a decision, recognition,
verdict, or lifecycle transition. Measurements, logs, decode residue,
process samples, and failed attempts remain bounded telemetry. A fact
kind without a registered consuming fold and refusal fixture fails the
generated admission check.

Only the resident appends. Hermetic rungs never open the socket and
never write the journal; a read-only open through `JournalRead` is
admitted for identity and verification reads, and it refuses by name
while a prepared-but-unfinalized migration exists. Beyond that, rungs
publish receipts and evidence bundles atomically on disk.
The resident may recognize those artifacts and append the corresponding
fact, such as `commit::landed` or `proof::sealed`, after validating
identity, digest, and current head. Recognition is idempotent under its
observation key.

Coordination is board-only stigmergy. Lanes do not message one another;
they change shared task and directive state through resident commands
and learn through ranked reads. Daemonless operation stays honest:
without the resident, a rung may produce an artifact but cannot append a
fact, acquire or renew a claim, retire a directive, or mutate a head.

Tape preserves raw vendor bytes before any interpretation. Its dialect
decoder emits typed listening events for recognition; tape bytes are not
wire DTOs and never cross the socket as facts.

## Vocabulary

The fact vocabulary is closed. Each kind is a plane-crossing verb, and
every body has exactly one DTO.

| Kind | Meaning |
| --- | --- |
| `task::added` | A task statement and initial workflow head were created. |
| `task::phased` | A claimed phase completed and workflow advanced. |
| `task::done` | A task completed under a re-proven active claim. |
| `task::reviewed` | A review verdict landed with reviewer and assignment witnesses. |
| `task::assigned` | The allocator seated a reviewer under the anti-self-review witness. |
| `task::noted` | A durable task note appended. |
| `task::deferred` | A task received a wakeable deferral. |
| `task::woken` | A task's deferral cleared. |
| `task::edited` | An unsettled task statement changed. |
| `claim::won` | Exclusive possession was minted from the current eligibility and claim heads. |
| `claim::carried` | Possession passed to a successor with ancestor lineage preserved. |
| `claim::freed` | Possession was released and the task became eligible again. |
| `claim::lapsed` | The lease deadline passed and the resident expired possession. |
| `claim::renewed` | The resident extended a live, epoch-bound lease within its maximum. |
| `doc::applied` | The exact normalized plan digest landed atomically at its witnessed board barrier. |
| `steer::sent` | A directive was created. |
| `steer::resent` | The same intent was sent again with lineage and escalation carried. |
| `steer::acked` | A live directive was acknowledged at a transcript locus. |
| `steer::declined` | A live directive was declined at a transcript locus. |
| `steer::missed` | An ACK named no live directive; the unmatched key was recorded and nothing retired. |
| `steer::lapsed` | A directive reached expiry without a valid disposition. |
| `steer::canceled` | The operator withdrew a live directive; the lineage retired without an agent disposition. |
| `consent::asked` | An ACP permission request arrived, bound to its cause. |
| `consent::granted` | A named rule or operator allowed the request; any budget effect is carried. |
| `consent::denied` | A named rule or operator denied the request. |
| `consent::queued` | The request pended to the operator while reversible work could continue. |
| `consent::floored` | A non-ACP lane's consent floor was recorded once for its occupancy. |
| `lane::seated` | A lane and its capability were created. |
| `lane::started` | An occupant became live under the lane. |
| `lane::exited` | The resident observed the occupant end. |
| `lane::compacted` | The occupant crossed a context compaction boundary. |
| `lane::renewed` | A successor occupant was seated with its ancestor named. |
| `lane::drained` | A graceful occupant end completed. |
| `lane::degraded` | Work run without the resident left a marker that the next resident recognized once. |
| `lane::held` | Repeated young occupant deaths paused automatic re-seating; the hold names its evidence and deadline. |
| `lane::resumed` | A held lane returned to seating eligibility by operator wake or expired hold. |
| `taste::confirmed` | The judge confirmed a taste at an evidence locus. |
| `taste::cleared` | The judge cleared a taste at an evidence locus. |
| `canary::alarmed` | An expected listening event remained absent past its armed window. |
| `canary::contradicted` | Listening meters disagreed: activity existed without decoded context. |
| `vendor::armed` | An unproven vendor version first ran with residue measurement armed. |
| `trust::pended` | An executable surface digest changed and entered quarantine. |
| `trust::granted` | The operator approved the exact executable digest. |
| `trust::revoked` | The operator withdrew approval from the exact executable digest. |
| `commit::landed` | The resident recognized a repository commit and bound its receipt verdict. |
| `gate::found` | The resident recognized a new gate finding: stable identity, severity, evidence locus, owner, and originating receipt; the Finding head opened. |
| `gate::cleared` | A standing finding resolved at its evidence; the Finding head closed. |
| `journal::migrated` | The one forward schema migration finalized before epoch mint. |
| `journal::backed` | A quiesced backup bundle was produced with its enumerated tape manifest. |
| `journal::restored` | A restore protocol completed and the new epoch recognized the exchanged authority. |
| `up::seated` | The resident acquired the kernel writer lock and minted its epoch. |
| `up::quiescing` | The resident stopped admission and began a graceful end. |
| `up::draining` | The resident began preserving or finishing admitted work and ending occupants. |
| `up::terminating` | Grace expired and the resident began ending occupants that had not finished. |
| `up::killing` | Bounded escalation began against occupants that survived termination. |
| `up::verifying` | Occupants ended; the resident began verifying claims, tape seals, and residue. |
| `up::dark` | The resident committed the verified terminal state immediately before releasing authority. |
| `proof::sealed` | The resident recognized a valid evidence bundle bound to commit, tree, and manifest. |
| `release::published` | The sealed release manifest digest, and each target entry's artifact digest, published under their detached proof receipts. |

The fact vocabulary does not stand in for the command registry. A command
name may coincide with a fact kind, but a command may also emit several
facts and update several heads; admission binds the typed transition, not
the shared spelling.

Every kind also names its producer, and the generated admission check
enumerates producers as well as consumers. The producer classes are
closed: the command door mints `task::*`, `claim::won/carried/freed`,
`doc::applied`, `steer::sent/resent/canceled`, `consent::*` decisions,
`trust::*`, and `up::*`; bus timers mint `claim::lapsed`,
`claim::renewed`, `steer::lapsed`, and `lane::held/resumed` deadlines;
the ack scanner mints `steer::acked/declined/missed`; resident
recognition mints `lane::*` observations, `canary::*`, `vendor::armed`,
`commit::landed`, `gate::found/cleared`, `journal::*`, `proof::sealed`,
and `release::published`; the judge mints `taste::*`. A kind with no
producer class is as dead as a kind with no fold — a bus timer that
never fires is a generated-check failure, not a quiet stall.

## Folds

A fold is a generation-stamped, disposable answer. Every implementation
declares six trait answers:

| Answer | Contract |
| --- | --- |
| `source` | Complete named input set: fact kinds, another fold, head tables, tracked configuration, or tape extents. Undeclared input is a defect. |
| `cursor` | Greatest journal `seq` the generation has applied. |
| `horizon` | Greatest `seq` through which all declared journal inputs are known complete. |
| `rebuild` | Isolated re-derivation into a new generation inside the folds store. |
| `beyond` | Policy after horizon: `Wait`, or `Answer` with the horizon explicitly marked. |
| `recovery` | Exact healing command, `spice journal replay <fold>`. |

The const registry is exhaustive:

| Fold | Answer | Sources |
| --- | --- | --- |
| `steer::inbox` | Pending directives per lane. | `steer::*`; witness and residency heads. |
| `consent::queue` | Pending consent and remaining budgets. | `consent::*`; witness and residency heads; tracked consent configuration. |
| `lane::canary` | Armed windows, listening meters, and alarms. | `canary::*`; `vendor::armed`; residency heads; declared tape extents. |
| `taste::active` | Active tastes and evidence ages. | `taste::*`; tracked taste configuration. |
| `canvas::verdicts` | Commit and finding verdict history. | `commit::landed`; `gate::found`; `gate::cleared`; Finding heads. |
| `canvas::magnitude` | Effort, pressure, residue, and consent rates. | `claim::*`; `consent::*`; `canary::*`; `vendor::armed`; `lane::canary`; task, claim, lease, and residency heads. |
| `canvas::flow` | Tasks moving through phases. | `task::*`; `claim::*`; `doc::applied`; task, workflow, claim, and lease heads. |
| `canvas::topology` | Lanes, worktrees, occupant lineage, and possession. | `lane::*`; `claim::carried`; `claim::renewed`; `up::*`; claim and residency heads. |
| `canvas::chronology` | Compaction, queues, human delay, trust, recovery, and lifecycle transitions in time. | `lane::*`; `steer::*`; `consent::*`; `canary::*`; `doc::applied`; `trust::*`; `up::*`; `journal::*`; `gate::*`; stored command results; `steer::inbox`; `consent::queue`; `lane::canary`; `canvas::verdicts`; `bus::refusals`; witness and residency heads. |
| `release::history` | Sealed and published artifacts. | `proof::sealed`; `release::published`. |
| `bus::refusals` | The bounded refusal-telemetry ring. | Its own ring, written by the bus at refusal time. Not re-derivable: rebuild is none and recovery is honestly none — bounded retention is the whole life. |

The registry admits every fact kind and rejects an unconsumed kind at
generation time; it equally rejects a fold answer naming content no
admitted kind or head can supply, and a kind with no registered
producer. A registry row may mix fold and head inputs, but the derived
answer remains disposable. Rebuild writes a new `gen` in one
folds-store transaction and publishes it only after cursor and horizon
checks pass.

When a fold declares another fold as input, its `horizon` is the
minimum over every declared input — journal kinds, heads, and input
folds' own horizons alike — so a stale upstream generation can never
launder completeness downstream. `spice journal replay FOLD` re-derives
the named fold's input closure in dependency order before publishing
it; a republished fold never reads a mid-repair upstream generation.

Thread is not a fold. It is an authority-side index that reconstructs
causal chains from `origin` and `cause`; its attribution modes are
`actor`, `lineage`, and `session`.

Every list read uses the same frame:

```text
Packed<T> {
    rows: Vec<T>,
    overflow,
    horizon
}
```

Rows are ranked before packing. `overflow` is explicit and names the
omitted count and continuation position; it is never silent truncation.
A gate may refuse on overflow. An interactive view may render the packed
prefix only when it renders the marker and horizon with it.

A fold-sourced reply wraps the same frame in its provenance:

```text
Derived<T> {
    fold,
    gen,
    cursor,
    packed: Packed<T>
}
```

Head-direct reads — the board, the journal stream, lanes, recall —
return bare `Packed<T>` beside their reply's snapshot cursor; they have
no fold or generation and never invent one. Only fold-backed reads wear
`Derived<T>`, so generation identity and applied cursor are always
present where they exist and never fabricated where they do not.

Daemonless commands with no journal access answer beside `direct: {}`.
The empty object is the one explicit spelling of that absence: `null`, a
cursor, a committed range, and objects with unknown fields are refused.
Resident reads and commands continue to carry their real snapshot or
committed position; they cannot use the direct form.

## Decoder

`wire::decode` is the sole bytes-to-DTO path for socket frames, journal
rows, and browser frames. The socket, journal, and browser do not carry
three compatibility systems:

| Clock | Law |
| --- | --- |
| Socket | Current-version-only `hello`. A mismatch refuses and names both versions; no session begins. |
| Journal | One forward schema migration runs at open before epoch mint. After it commits, all rows decode through the current path; permanent historical upcasts do not exist. |
| Browser | No independent clock. The app is compiled from the same `wire` crate and embedded in the same binary that owns the socket. |

Unknown kinds fail the generated kind-to-fold admission table. A known
kind with a malformed body faults by kind and field. No app schema is
generated beside the Rust DTOs. Tape remains outside this decoder: the
listening dialect converts preserved vendor bytes into its own typed
events before resident recognition decides whether a fact crossed.

## Identity

Durable identity has one algorithm set, named here so no leaf chooses
its own. Every durable digest — document plan, trust surface, tape
member, gate receipt, target payload, sealed manifest, publication — is
BLAKE3 over a domain-tagged canonical encoding, rendered lower-case
hex; the domain tag names the digest class, so no two classes can
collide byte-for-byte. Command identity binds the canonical JSON
encoding of the envelope with sorted keys and sorted witnesses. UUIDs
are version 7, minted from the operating system's entropy source in
native code and from the platform's cryptographic entropy in the
browser, and always spelled lower-case `8-4-4-4-12`. Instants are
RFC3339 UTC with millisecond precision; deadlines persist as those
instants while waits run on monotonic clocks, and an observed wall-clock
regression faults by name.

Spice-Origin v1 is frozen as exactly one trailer line:

```text
Spice-Origin: v1 repo=<repository-uuid> task=<KEY-BORN>
```

Three space-separated fields after the key: the literal `v1`, the
repository identity, and the task handle. `Spice-Origin: v1
repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD` is valid;
any reordering, extra field, missing field, or other spelling is not
this trailer. Unknown or historical forms remain visible unresolved
evidence, and any future change is a new trailer name, never a
mutation of v1. Richer attribution stays in journal facts.
