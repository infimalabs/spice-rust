# Product model — documents, wire, extension, media

Task documents, the output contract, extension surfaces, diagnostics, and
media.

## Task documents

An authored task document is a digest-bound batch input, not a lossless
serialization of the board. `spice doc PATH` (or `-` for standard input)
classifies ordinary Markdown, emits the one canonical dialect spelling, previews
the tasks it would touch, and applies only that exact digest.

- **Determinism.** The same document produces the same graph on any machine.
- **Never-raise.** Hostile input produces refusals, not crashes; BOM and CRLF
  spellings normalize to UTF-8 LF with a warning.
- **Fixed point.** The emitter has one escape rule its reader reverses exactly;
  classifying emitted text and emitting it again changes no byte.
- **Cycles are refused, not resolved.** A dependency cycle refuses with the
  cycle named.
- **Author text is never parsed as attributes.** A task titled
  `priority: high` is a title.
- **Preview is consequence-free.** Apply reclassifies the document, requires
  the canonical digest the operator named, and binds the board head it reads;
  movement after that read refuses before mutation.
- **Apply is one guarded transaction.** It is all-or-nothing, and retrying a
  digest that already has a `DocApplied` fact returns that receipt. Any refusal
  writes no task or document fact.
- **Matching is project plus slug.** A row matches on its exact project and
  internal `document-slug:<slug>` tag; the same slug in another project is a
  different task. Context-free classification places documents in
  `task.document`.

Applying an edited document replaces rather than patches: title, description,
acceptance, tags, priority, and the explicit dependency graph are replaced, and
an absent value clears the old one. Flow is set only at creation. Annotations
are the exception: they append as task notes, skipping exact duplicates, and
omission never erases a note. `due` is normalized but has no board effect. The
dialect's `Flow:` field has its own closed vocabulary — `design`, `plan`,
`todo`, `verify`, `review` — distinct from the board's open phase words.

`spice doc ledger HANDLE` renders one task's current title, description,
acceptance, flow, tags, priority, and notes interleaved with its workflow
steps. A bounded page names its next byte offset and the complete ledger's
digest; a continuation refuses rather than joining bytes from a changed
ledger. `--output` remains the complete export. The ledger is a readable
recovery view, not the inverse of apply: it exports no family or dependency
graph, and a task the closed dialect cannot express refuses with
`doc::ledger-unrepresentable`.

## The JSON outcome

Every finite `spice --json` invocation emits one newline-terminated `Outcome`
document. A daemonless reply is `{"kind":"reply","body":...}`; a read adds its
snapshot `cursor` and a mutation its committed `seq` range, and the absence of
both is the one spelling of a daemonless reply. Refusals and faults use the
same envelope with their own `kind`. Selecting `--json` changes only the
document shape, never the exit code or stream: replies go to standard output,
refusals and faults to standard error.

## Room and preparation commands

The [room loop](loops.md#l-room--conversation-at-a-task) states what these
commands mean; this section fixes their operand bodies and bounds.

### Preparation command bodies

`preparation retain`, `critique`, `cover`, and `decide` each take one canonical
JSON object. Unknown fields refuse, nothing is lenient, and a malformed body
refuses with `preparation::argument-invalid`, giving the parse detail and a
complete body that command accepts. `preparation adopt` takes two task handles
instead.

- **Values.** A task is its bare nine-character birth stamp, without the
  project prefix (`dVzdhh8Nx` for `REPOSIT-dVzdhh8Nx`). A candidate is
  `{"controller": STAMP, "candidate": STAMP}`: the controlling Design's stamp
  and a new nine-character native stamp naming the candidate under that
  controller. Critique, coverage, and decision identities are native base-52
  sequences (`"1"`, `"2"`, …), new within their candidate or, for a decision,
  within its controller. A source is
  `{"kind": "journal", "fact": POSITION}` (a position `preparation show`
  prints), `{"kind": "artifact", "digest": HEX}`, or a transcript hunk.
- **`retain CANDIDATE_JSON`.** `candidate`, `governed_inputs` (tasks the
  candidate assumes), `outputs` (at least one gated output), `references`
  (at least one `{"relation": NAME, "source": SOURCE}`), and optional
  `evidence` (question results retained with the candidate). Inputs and outputs
  are unique and disjoint.
- **`critique CRITIQUE_JSON`.** `critique` (`{"candidate": …, "critique":
  SEQUENCE}`), `category`, `source`, and `standing` (`clean` or
  `material-finding`).
- **`cover COVERAGE_JSON`.** `coverage` (`{"candidate": …, "coverage":
  SEQUENCE}`), nonempty unique `scopes`, and `source`. The command attributes
  the reviewer authority itself.
- **`decide DECISION_JSON`.** `decision` (`{"controller": STAMP, "decision":
  SEQUENCE}`), `source`, and one outcome over an exact candidate under that
  controller: `{"kind": "selected", "candidate": …, "selected_outputs": […]}`,
  `{"kind": "returned", "candidate": …, "reason": TEXT}`, or
  `{"kind": "collapse", "candidate": …, "reason": TEXT}`. A selected subset is
  nonempty and unique; return and collapse-request reasons are nonblank.

### Limits and their repairs

| Bound | Value | Refusal and repair |
| --- | --- | --- |
| Designs per working group | 6, as lanes per team | `group::design-limit`: move one out or choose another group |
| Group membership | Design tasks only | `group::design-only`: choose a Design or declare its Design intent |
| Partition revision | the one read | `group::partition-moved`: read the groups and retry |
| Design possession | inside a working group | `claim::design-passive` on capture, steal, or recovery and `room::design-passive` on coordination: `spice group move DESIGN --to GROUP` or `--new` |
| Deactivating a held Design | a holder address proved by its seat or its claimed occupant's birth | `room::holder-address-unavailable`: the move records nothing; free or reseat the holder and retry |
| Probe creation | the bare `probe` flow without intent, created directly under an open Design inside a working group | `task::probe-flow`: omit `--intent` and every other phase; `task::probe-design`: create it under an active open Design while holding it or with `--preparation DESIGN` |
| Candidate outputs | no probe | `preparation::probe-output`: cite the probe's note or artifact as a candidate reference instead |
| Automated resumes per rule | 1 through 8 | `room::auto-resume-out-of-range` |
| Scoped request field | 2048 bytes | `room::coordination-scope-too-large` |
| Live coordination per room | one request | `room::coordination-live`: retire or put aside the unanswered request |
| Activity-only attention | one day | aging drops the reason, never history or duties |
| Route fanout per publication | 32 | the packet refuses whole |
| Publication content | 64 KiB; 80 KiB with 1 KiB reserved per route for its ACK/NAK return | the packet refuses whole; an operator answer refuses `room::answer-too-large` |
| Pending question results per requester | 128 | further ASKs refuse; MSG, controls, and other requesters still admit |
| Relevance sources per observation | 8 | an observation never cites more |
| Reports per OOPS grooming record | 100 parked reports, in an open Design room | `room::oops-batch-too-large`; `room::oops-batch-not-open`: groom under a Design room |
| Design contribution text | 4096 bytes, nonempty after trimming | `room::contribution-empty` or `room::contribution-too-long` |
| Sources per Design contribution | 8 distinct | `room::contribution-sources-exceeded` or `room::contribution-source-duplicate` |
| Contribution room | an open Design, by a lane | `room::contribution-not-design`, `room::contribution-design-concluded`, or `room::contribution-lane-only` |
| Post to a settled room | an unresolved anchor only | `room::anchor-settled`, naming the standing: a completed, closed, or deleted anchor's room stays readable, and its open receipts and results still settle |
| Late answer | the immutable deadline | `room::deadline-passed`: close with `spice room cancel RESULT --clause TEXT --override` |
| Answer or close by a non-party | stored parties only | `room::not-respondent` or `room::override-required`, naming the `--override` command |
| Required evidence | each result answered | `task::evidence-unsatisfied`: ask again, or the operator replaces or waives it |
| Positive Design conclusion with gated outputs | none unselected | `preparation::controlled-outputs-pending`: select the exact subset with `preparation decide`, or close or delete each unselected output; until then the allocator holds the final-phase Design for the operator and starts no lane for it |
| Candidate or decision controller | an open Design | `preparation::controller-not-design` or `preparation::controller-concluded`: a Plan's outputs follow its positive review |
| Selecting Design outputs | no pending Design dependency, including review corrections | `preparation::controller-blocked`: settle the dependency first, so a finding keeps the Design pending |

## Bounded reads

A read shows current, relevant state by default; history and detail are
explicit flags, and no flag makes terminal output unbounded. Every
output-bearing surface admits through the one contract in `wire::output`:

- **Limits.** A surface declares interactive defaults and maxima for rows
  (`--limit`), aggregate encoded bytes (`--max-bytes`), and per-field text
  (`--max-text`), all under one repository ceiling. A request above the
  surface maximum is refused, not clamped. Zero rows is a count-only read.
- **Relevance-first admission.** Records are offered current and actionable
  first. Every record is charged its full encoded length, the first included,
  and the first record that does not fit closes admission, so a page boundary
  is a single cursor.
- **No hidden omission.** A partial answer states what it withheld, the limit
  that closed admission, the next record's size, and the exact continuation
  command (`--after CURSOR`); a record too large for the surface continues
  through a file export. Clipped text keeps a UTF-8 boundary and reports its
  withheld bytes.
- **Exact export.** Complete documents and evidence are written atomically to
  an explicitly named file with a receipt naming its path, length, and digest;
  `-` is refused.

Surfaces that refine the law:

- `spice inbox` and `spice consent` are authority queues: every pending key or
  request is indexed outside every limit, so no item hides behind a page.
- `spice task show` never clips the fields `spice task modify` replaces, so a
  field read there is exactly what a replacement overwrites. `--context` and
  `--history` add metadata, workflow steps, and notes under the shared limits;
  any principal that may read the task reads them there, while `spice journal`
  stays operator-only.
- `spice board` is a posture — counts, the reader's own claim outside every
  budget, and the ready, review, and blocked rows on its route — not a dump.
  The complete board is only `spice board [FAMILY] --output FILE`.
- `spice retro` and `spice watch` show decoded reasoning only as a count of
  withheld bytes, in every form including export. A cursor from a replaced
  tape refuses rather than replaying against the replacement. Bare `watch`
  is one snapshot; `--follow` requires a bounded deadline and blocks on native
  filesystem notifications rather than rescanning.
- `task artifact show` never writes raw payload bytes to the terminal; exact
  extraction is `--output PATH`, verified against the retained digest, and a
  payload that fails its digest is neither shown nor extracted.
- Backup and restore share one bundle bound on members and aggregate bytes;
  restore refuses an oversized manifest before reading a member, and an
  oversized current authority before changing anything.

Mutation replies are outside this contract: they stay complete and concise.

## Kept jobs

The occupant's `job` tool reads the same contract. Its default `list` is every
job still owed to the lane — running, or ended and not yet heard — the set the
seat shows as Kept jobs; delivered jobs are read only through `history`, newest
first, with an exact continuation. `wait` passively observes a finite job and
leaves it running. An occupant holding a claim may end its turn while a
deadline-bounded job runs; the lane stays vacant until that job ends or emits
an event, then wakes a successor that still holds the claim. A service never
ends on its own and never keeps a lane idle: it runs while its claim is active
or until `stop`, and its readiness is read from its logs or served surface,
never by waiting on its process. Every seat start lists the kept commands
still running, and says nothing when none run. The lane read lists each kept
job with its kind — `command` when deadline-bounded, `service`, or `watch` for
a command with `events` — and its standing, `running` or `unheard`.

A shell call with `events` is answered at once with the job's id; each stdout
line becomes a background notice to the seated occupant, in order and exactly
once, at the next step boundary, and stderr goes only to the job's log. A
burst beyond the event bound ends the job's events with one notice while the
command keeps running, and the ending is delivered after the last event. A
service refuses `events`. An event nobody hears within the delivery grace
wakes a vacant lane exactly as an unheard ending does.

## Extension surfaces

Repository configuration may mount a command under the `spice` namespace;
[built-in verbs win at every depth](invariants/session.md#extension-built-in-verbs-and-registered-actions-win-at-every-depth),
so a dotted mount extends a built-in verb only with a novel action name. The
compiled registry is the only executable extension authority: configuration
cannot discover or load executable values into the vendor, wrapper, or gate
namespaces.

## Diagnostics

`spice doctor` rolls up one closed roster of stateful subsystems — board,
journal, steering, lane, transport, installation, and release. Host-probing
checks stay out of hook verdicts: diagnosing the environment and refusing a
commit are different jobs.

`spice demo` replays a canned transcript through the real renderer in an
isolated seeded root, with no model call and no mutation of the caller's
repository. Nothing opens on the operator's display without an explicit
`--open`.
The seed's primary task is held by Ava through the same claim authority as live
work, so its team task strip, lane ownership accents, roster, task overview, and
board reads all describe one shared team-owned row.

Interrupting a running demo stops its resident and preserves the root, and
every demo reply prints that root's `spice demo --reuse ROOT` launch command,
which restarts the resident over the state lanes left behind. Reuse admits a
root by seed identity, not seed state: its manifest and canned transcript are
unchanged, its journal still holds the seeded task, lanes, and turns, and its
HEAD descends from the seed commit. Its branch, project policy, residency, and
journal may all have advanced. A root that fails any identity check refuses
with a named repair, and a root whose resident is still up refuses as
`resident::already-up`.

## Media

An attachment is part of the operating record, not a transient upload: it
stays browsable beside the message it arrived with. A steering directive keeps
image bytes only in its attachment values; its prose places them with bounded,
digest-bound `attachment:` markers, which display projections expand from the
same record and which are never fetch authority. A send whose prose duplicates
an attachment's data URL refuses before journal mutation.

Speech is the viewing browser's own system synthesis; nothing renders audio on
the resident. One global queue speaks sequentially and never overlaps
utterances. An utterance settles on the browser's end or error, and a failure
is a nonblocking notice that never blocks the visible stream — the transcript
remains the record. `say.voices` is an ordered list of voice names, scoped to
the agent's worktree: each device speaks an agent's messages with the first
listed name it publishes, and the agent keeps that voice for the page. With an
empty list, each device spreads roster agents across its published macOS
premium voices only: Safari's `com.apple.voice.premium.` identifier or
Chromium's exact ` (Premium)` name suffix, served locally. Enhanced, standard,
novelty, remote, and unmarked voices never enter. Among the premium voices the
pool keeps the default premium voice's language, or the browser's language when
none is default: an agent keeps the voice its lane hashes to unless a teammate
already holds it, so teammates sound different until a team outnumbers the
voices, and agents on different teams may share. A published list without a
premium voice in that language refuses, naming the language and the premium
languages it does offer; it never falls back to another tier or the browser
default. A speaker outside the roster with no list uses the browser default. A
browser that has not yet published voices is waited for under a bound; a
published list holding none of the configured names refuses, naming the voices
the browser offers.
