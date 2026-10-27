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
  typed document slug field; the same slug in another project is a
  different task. Context-free classification places documents in
  `task.document`.

Applying an edited document replaces rather than patches: title, description,
acceptance, priority, and the explicit dependency graph are replaced, and an
absent value clears the old one. Flow is set only at creation. Annotations
are the exception: each publishes as an observation in the task's room unless
the room already holds that exact text, and omission never erases one. `due`
is normalized but has no board effect. The
dialect's `Flow:` field has its own closed vocabulary — `design`, `plan`,
`todo`, `verify`, `review` — distinct from the board's open phase words.

`spice doc ledger HANDLE` renders one task's current title, description,
acceptance, flow, priority, and notes interleaved with its workflow steps. A
bounded page names its next byte offset and the complete ledger's
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

Text output, for humans and agents alike, spells every journal position as
its condensed base-52 sequence code, and every position or cursor a command
takes back as text (`--after`, a thread reference) admits only that spelling.
The JSON outcome and wire payloads keep positions as integers. Persisted
control strings keep their recorded decimal spelling.

## Room and preparation commands

The [room loop](rooms.md#l-room--conversation-at-a-task) states what these
commands mean; this section fixes their operand bodies and bounds.

### Preparation command bodies

`preparation critique`, `cover`, `decide`, and `withdraw` each take one
canonical JSON object. Unknown fields refuse, nothing is lenient, and a
malformed body refuses with `preparation::argument-invalid`, giving the parse
detail and a complete body that command accepts. `preparation propose`
assembles its body and takes only the holder's justification, `preparation
adopt` takes two task handles, and `preparation nudge` one open Design's
handle.

- **Values.** A task is its bare birth stamp, without the project prefix
  (`dVzdhh8Nx` for `REPOSIT-dVzdhh8Nx`): nine characters, or eight for a
  retained millisecond identity, which is never reminted. A candidate is
  `{"controller": STAMP, "candidate": STAMP}`: the controlling Design's stamp
  and the nine-character native stamp naming the candidate under that
  controller, as `preparation show` and a proposal's reply print it. Critique,
  coverage, and decision identities are native base-52 sequences (`"1"`,
  `"2"`, …), new within their candidate or, for a decision, within its
  controller. A source is `{"kind": "journal", "fact": POSITION}` (a position
  `preparation show` prints), `{"kind": "contribution", "room": STAMP,
  "publication": STAMP}` (the `source` a `room contribute` answer prints,
  pasted unchanged), `{"kind": "artifact", "digest": HEX}`, or a transcript
  hunk.
- **`propose [--design HANDLE] [--recommend conclude|collapse] [--cite
  RELATION=KEY...] [--dry-run] JUSTIFICATION`.** The client assembles the
  proposal from the board and the Design's room; the holder writes only the
  nonblank justification. Outputs are every pending task whose preparation
  names the Design as parent and itself as construction, as creation under
  the held Design claim or `--preparation` records it, except those an earlier
  decision under the Design resolved; a returned or withdrawn proposal's
  outputs assemble again. Governed inputs are the outputs' other pending
  dependencies. Outputs make the recommendation `work`; a pass with none
  passes `--recommend conclude` or `collapse`, and the flag beside outputs
  refuses `preparation::recommend-with-outputs`, while neither refuses
  `preparation::proposal-empty`. An output without acceptance refuses
  `preparation::output-unaccepted`, and one that lost its edge to the Design
  refuses `preparation::output-ungated` naming it. References are every
  contribution in the room, related `for`, `against`, `evidence`, or
  `question` by its charge or kind, plus every earlier decision under the
  Design as `decision`; `--cite RELATION=KEY` adds a source, or relabels an
  assembled one, from any key `room contribute --cite` takes or a room
  publication stamp, and a quiet room with no cite refuses
  `preparation::proposal-unreferenced`. Every reference must lie in the
  controlling Design's scope: a contribution in its room, or a journal fact of
  its room, the Design, a governed input, an output, or a task prepared under
  it. The Design is the open Design the caller holds an active claim on
  (`--design HANDLE` names it when the caller holds several; holding none
  refuses `preparation::design-unheld`), the candidate stamp is minted fresh,
  and the Design's statement and claim heads, each output and input, and the
  room's newest publication are witnessed, so a statement edit, claim move, or
  room publication between that read and the write refuses
  (`preparation::proposal-statement-moved`, `proposal-claim-moved`, or
  `board::witness-moved`), and an unacknowledged Design generation refuses
  `proposal-claim-stale`. `--dry-run` prints the assembled proposal and
  records nothing. The write records the immutable proposal and releases that
  exact claim atomically.
- **`critique CRITIQUE_JSON`.** `critique` (`{"candidate": …, "critique":
  SEQUENCE}`), `category`, `source`, and `standing` (`clean` or
  `material-finding`).
- **`cover COVERAGE_JSON`.** `coverage` (`{"candidate": …, "coverage":
  SEQUENCE}`), nonempty unique `scopes`, and `source`. The command attributes
  the reviewer authority itself.
- **`decide DECISION_JSON`.** `decision` (`{"controller": STAMP, "decision":
  SEQUENCE}`), `source`, and one outcome over an exact proposal under that
  controller. A work proposal uses `{"kind": "resolved", "candidate": …,
  "outputs": [{"output": STAMP, "verdict": "accepted"|"rejected", "comment":
  TEXT?}, …], "comment": TEXT?}` and names every proposal output exactly once.
  Comments are optional but nonblank when present. Exactly the accepted outputs
  receive execution authorization; rejecting all outputs authorizes nothing.
  An outputless conclude or collapse proposal uses `{"kind": "accepted",
  "candidate": …}` and stages the corresponding guarded Design conclusion in
  the same transaction. Any proposal can instead use `{"kind": "returned",
  "candidate": …, "reason": TEXT}`; the nonblank material feedback records
  durably without authorizing or concluding. Historical selected, returned,
  and collapse records remain readable after migration.
- **`withdraw WITHDRAWAL_JSON`.** `candidate` (`{"controller": STAMP,
  "candidate": STAMP}`) names the author's own undecided candidate and
  `reason` is nonblank, such as which version corrects it.

### Limits and their repairs

| Bound | Value | Refusal and repair |
| --- | --- | --- |
| Designs per working group | 6, as lanes per team | `group::design-limit`: move one out or choose another group |
| Group membership | Design tasks that are open or completed; a completed Design grants its group no standing | `group::design-only`: choose a Design whose current phase is `design` or one already completed, or finish its admitted intake first |
| Partition revision | the one read | `group::partition-moved`: read the groups and retry |
| Group disband | one standing group at the partition revision read | `group::missing` or `group::partition-moved`: read the groups and retry only if authority changed; a disband leaves every Design ungrouped and its deferral unchanged |
| Design possession | undeferred open Design, grouped or not | a deferred Design refuses capture as `claim::capture-ineligible`, steal as `claim::steal-ineligible`, and a nudge as `preparation::nudge-deferred`: run `spice task wake DESIGN` (`--claim` to take it), or move it into a working group, which wakes it. An operator `spice task defer DESIGN` of a held Design asks its holder to checkpoint and `spice task free`; a repeat asks again only when no stop stands for the current claim |
| Steeping Design | open, undeferred Design whose cycle is not yet ready for proposal | allocation never offers it; capture, steal, or recovery by name claims a Design still gathering input before it steeps, a steal taking it from any holder, and its holder ends the pass with `spice preparation propose`; `claim::design-held` on capture, steal, or recovery of a Design awaiting decision or waiting on selected work, naming the Design, its cycle, and that cycle's next action; `spice preparation nudge DESIGN` readies a gathering Design for allocation |
| Nudge hold | 120 seconds from the nudge, for the lane its message went to | allocation offers the Design to no other lane meanwhile; capture by name stays open |
| Activity-only attention | one day | aging drops the reason, never history or duties |
| Route fanout per publication | 32 | the packet refuses whole |
| Publication content | 64 KiB; 80 KiB with 1 KiB reserved per route for its ACK/NAK return | the packet refuses whole; an observation refuses `room::post-too-large` |
| Relevance sources per retained observation | 8 | an observation never cites more |
| Design contribution text | 4096 bytes, nonempty and stored without leading or trailing whitespace | `room::contribution-empty` or `room::contribution-too-long`; the command normalizes outer whitespace, while a noncanonical journal fact is refused |
| Design contribution charge and question item | a charge from -2 to +2 weighing one one-based question item of the Design statement the command witnessed, or a question; 0 and ±2 cite at least one source | `room::contribution-kind-missing`, `room::contribution-charge-invalid`, `room::contribution-question-invalid`, `room::contribution-question-missing`, `room::contribution-question-unknown`, or `room::contribution-uncited`; retained contributions from before charges name no item |
| Design contribution citation key | a task handle or stamp, a contribution publication stamp in the Design's room, a base-52 journal code, or an artifact digest | `room::contribution-cite-unknown` or `room::contribution-cite-invalid` |
| Sources per Design contribution | 8 distinct exact authorities | `room::contribution-sources-exceeded` or `room::contribution-source-duplicate`; two transcript links to the same locus and text range remain one authority when only their accounted boundary differs |
| Contribution room and proof | an open Design, by a lane, with one source witness per cited authority | `room::contribution-not-design`, `room::contribution-design-concluded`, `room::contribution-lane-only`, or `room::contribution-source-witness`; the journal refuses a contribution without its matching successful command receipt |
| Exact Design maturity statement heads | current plus three recent heads; older heads fold into one aggregate | display the aggregate instead of expanding history |
| Distinct lanes per Design maturity set | 64 sorted identities | display the retained count with a bounded marker; a lane beyond an item's bound cannot count as conceding |
| Design maturity question-item series | 32 per Design, least recently charged dropped first | a dropped item reads as quiet until charged again |
| Post to a deleted room | any anchor that is not deleted, settled included | `room::anchor-deleted`, naming the standing: a deleted anchor's room stays readable, and its open receipts still settle |
| Terminal Design or negative Plan settlement | every pending downstream dependency branch closes in the same transaction | a held branch refuses `task::controlled-descendant-claimed` and names its holder unless explicit force is permitted |
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
  `--history` add metadata, workflow steps, notes, and the task's room
  publications newest first under the shared limits, resuming inside the room
  at a `room:PUBLICATION` cursor; any principal that may read the task reads
  them there, while `spice journal` stays operator-only. A claim answer's
  `room_context` carries the same newest room rows under the interactive
  defaults, so the next phase's holder and the reviewer receive the task's
  conversation.
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
- A task holds at most 32 artifact citations. Every `task artifact add`,
  `list` page, and `remove` answer carries the task's `budget`: its `count`,
  the `bound`, and the `remaining` slots, whole-task even on a partial page;
  the add refusal at the bound names `task artifact remove`. Removal drops one
  citation of a pending task, deleting the payload only when no remaining
  citation shares its digest; it refuses a completed or deleted task, an
  unknown id, and an artifact its room cites. A removed id is never issued
  again.
- Backup and restore share one bundle bound on members and aggregate bytes;
  restore refuses an oversized manifest before reading a member, and an
  oversized current authority before changing anything.

Mutation replies are outside this contract: they stay complete and concise.

## Browser Work deep links

Work is a durable task-set location over the always-mounted Chat surface, not a
separate page. `#/work` names all execution work; its canonical refinements name
one exact project, all or project-scoped Design planning, one team UUID, or one
working-group room stamp. A project segment is the complete admitted label, never
a root or prefix of one. Team and group URLs serialize only their stable identity;
the hydrated topology or working-group partition supplies current membership.
Missing typed identities and every malformed or query-bearing spelling return
to `#/chat` with a visible refusal. Project, individual-task, relationship, and
composed selections remain browser-local and release a Work URL before opening.
The complete route grammar and overview behavior live in
[`app/OVERVIEW.md`](../../app/OVERVIEW.md#task-overview).

The command bar hangs two global totals from its top edge immediately before
the Spice button: all open execution work, then all open Design work. Each is
the same badge-only horn tab as before, one task-tab gap from the other; the
Design total stands off the button by the button's own gap from the window
edge, with no per-project tabs and no row reserved above the bar. Counts, danger
marks, titles, and navigation retain the exact scope each total names.

## Kept jobs

The occupant's `job` tool reads the same contract. Its default `list` is every
job still owed to the lane — running, or ended and not yet heard — the set the
seat shows as Kept jobs; delivered jobs are read only through `history`, newest
first, with an exact continuation. Every job records the exact ambient task
held at launch, or no task when the turn is unclaimed. A task overview lists
only jobs carrying one of its selected task identities.

`wait` passively observes a finite job and leaves it running. An occupant
holding a claim may end its turn while a deadline-bounded job runs; the lane
stays vacant until that job ends or emits an event, then wakes a successor that
still holds the claim. A service never ends on its own and never keeps a lane
idle. A service launched while a task is held runs while that exact claim win
remains active or until `stop`, surviving occupant succession within the claim.
A service launched by an unclaimed turn belongs to that turn instead and is
stopped when the turn exits. Service readiness is read from its output or the
served surface, never by waiting on its process: `wait` without a pattern
answers a running service at once. Every seat start lists the kept commands
still running, and says nothing when none run. The lane read lists each kept
job with its kind — `command` when deadline-bounded, `service`, `watch` for a
command with `events`, or `watched-service` for a service with `events` — and
its standing, `running` or `unheard`.

`wait` with an `until` regular expression follows any job, service included,
until a line of its log matches, and answers with that line while the job keeps
running. Lines written before the wait count, so a service that already
announced its readiness is answered at once. The log is read once, in order,
from where the wait last read, holding only the line still being written, and
only the log's own growth wakes the next read; there is no timer. The wait
otherwise answers at its timeout, when a message arrives, or when the job ends,
then saying whether its last lines matched. A pattern that does not compile, or
an `until` on any other operation, is refused before anything waits.

A shell call with `events` is answered at once with the job's id; each stdout
line becomes a background notice to the seated occupant, in order and exactly
once, at the next step boundary, and stderr goes only to the job's log. A
burst beyond the event bound ends the job's events with one notice while the
command keeps running, and the ending is delivered after the last event. An
event of a deadline-bounded command that nobody hears within the delivery grace
wakes a vacant lane exactly as an unheard ending does. A service may carry
`events` too, but because a service never keeps a lane idle, its events reach
only a seated occupant and never wake the lane; a later seat hears what earlier
seats did not.

## Principals

A principal is one human the repository has admitted: a fixed identity born by
one `principal::admitted` fact, a recorded profile (name and email), roles, and
the capability it connects with. The
[identity invariants](invariants/identity.md#identity-a-human-principal-is-born-once-by-an-explicit-admission)
own the rules; this section fixes the surfaces.

- `spice principal list` reads every principal in birth order with its
  identity, code, birth position, admission instant, roles, and profile.
- `spice principal admit [--name NAME] [--email EMAIL] [--operator]` bears one
  and prints its capability once; opening the app at `ORIGIN/#CAPABILITY`
  connects as that principal.
- `spice principal profile PRINCIPAL [--name NAME] [--email EMAIL]` replaces
  the given profile fields.
- `spice principal role grant|revoke PRINCIPAL operator` changes the one role.
- `spice principal deauthorize PRINCIPAL` retires the capability an admission
  issued, ending that principal's sessions.

Every one of these is an operator-role command. The app's global menu shows
**Principals**, whose connected human's own row seats their personal inbox,
and sends the same commands: each principal reads with its avatar and
initials, name, code and identity, email, roles, capability standing, and
admission time, beside Edit profile, Grant or Revoke operator, and Revoke
capability. Admit a human takes a required name, an optional email, and the
operator role, and the reply's
capability shows once in that panel as the opening link with a copy control;
nothing stores it, and a replayed admission never shows it again. A principal
without the operator role sees only their own row and personal inbox there,
because the resident refuses it the listing.

Facts the historical `operator` actor recorded read as the earliest-born
principal, and every authored item names its author's principal. A principal's
code is the vowel-free base-52 spelling of the position of the admission that
bore it: it identifies one human in text and the catalog, never a route.
Addressing stays a pointer to a concern, `#KEY` of a direction or `#TASK`; a
`#KEY` implies the principal who sent that direction, so no principal has an
MSG selector or route of its own.

## Extension surfaces

Repository configuration may mount a command under the `spice` namespace;
[built-in verbs win at every depth](invariants/session.md#extension-built-in-verbs-and-registered-actions-win-at-every-depth),
so a dotted mount extends a built-in verb only with a novel action name. The
compiled registry is the only executable extension authority: configuration
cannot discover or load executable values into the vendor, wrapper, or gate
namespaces.

## Diagnostics

`spice doctor` rolls up one closed roster of stateful subsystems — board,
journal, steering, lane, transport, installation, and release. It reads the
journal through its bounded indexes, so its cost does not grow with journal
history; `spice journal verify` owns whole-journal verification. Host-probing
checks stay out of hook verdicts: diagnosing the environment and refusing a
commit are different jobs.

`spice demo` replays a canned transcript through the real renderer in an
isolated seeded root, with no model call and no mutation of the caller's
repository: the turns are posts in the seeded task's room, read in that task's
sheet. The seeded lanes are simulated and never run, so their team plane
shows no agent prose. Nothing opens on the operator's display without an
explicit `--open`.
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

Speech is the viewing device's own system synthesis: the browser's
`speechSynthesis`, or under `spice app` the desktop shell's synthesizer, which
offers the downloaded voices the embedded web view withholds (see
[the console's speech surface](../../app/CONSOLE.md#speech)); nothing renders
audio on the resident. One global queue speaks sequentially and never overlaps
utterances. An utterance settles on the browser's end or error, and a failure
is a nonblocking notice that never blocks the visible stream — the transcript
remains the record. `say.voices` is an ordered list of voice names, scoped to
the agent's worktree: each device speaks an agent's messages with the first
listed name it publishes, and the agent keeps that voice for the page. With an
empty list, an Apple mobile browser—iPhone, iPad, iPod, or iPadOS presenting
its desktop-mode Macintosh user agent, recognized by a `Mobile/` token or by
`navigator.maxTouchPoints` above one, which a Mac reports as zero—uses the
device default voice because its published voices do not carry macOS tier
metadata. Other browsers
spread roster agents across published macOS premium voices first: Safari's
`com.apple.voice.premium.` identifier or Chromium's exact ` (Premium)` name
suffix, served locally. The pool keeps the default premium voice's language,
or the browser's language when none is default; within it the pool is the
premium voices, else the enhanced voices (`com.apple.voice.enhanced.` or
` (Enhanced)`), else every speaking voice the browser serves locally in that
language, never a macOS novelty voice (`com.apple.speech.synthesis.voice.`:
Bells, Bubbles, Zarvox, ...), and the browser default speaks only when the
language has no such voice.
An agent keeps the voice its lane hashes to unless a teammate already holds
it, so teammates sound different until a team outnumbers the voices, and
agents on different teams may share. The first time a Macintosh page steps
below premium, one toast names the language, how many voices the browser
publishes, the languages its premium voices do speak, what speaks instead,
and the remedy: download a premium voice or set `say.voices`. Other platforms
have no premium tier to miss and run the same descent silently. A speaker
outside the roster with no list uses the browser default. A browser that has
not yet published voices is waited for under a bound when configured or
desktop spreading needs the list; a browser that
never publishes any voice refuses, and a published list holding none of the
configured names refuses, naming the voices the browser offers.
