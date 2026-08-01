# NAMES

The lexicon. One sense per term, carved once. This document binds through
wire types and the clap surface: the compiler and parser enforce it; text
search does not.

## The Law

1. **One word.** Every crate, verb, noun, field, and view is a single
   word: impactful, then obvious in retrospect.
2. **Paths qualify; compounds never do.** Where two ideas meet, the
   module system joins them: `claim::Active`, `wire::Hello`,
   `gate::kill`. A name that wants a space, dash, or underscore is a
   boundary announcing itself; split the package, never the word.
3. **Case is grammar, never meaning.** Rust conventions stand: types
   upper, the rest lower. One sense may wear noun and verb roles, as
   `Steer` and `steer` do; case distinguishes grammar, never meaning.
4. **Retired terms name their replacement.** A word leaves the lexicon
   only by pointing at its successor, once, here.
5. **Foreign forms keep their skin.** Git trailer keys
   (`Spice-Origin`, beside `Co-Authored-By`), external crate names
   (`rust-embed`, `aho-corasick`), and vendor flags are their owners'
   conventions: carried verbatim, coined never.
6. **UUIDs keep their standard skin.** A UUID is lower-case
   `8-4-4-4-12` text everywhere it leaves a type. Only that spelling is
   accepted or emitted.

## The Canon

### The system

| Word | Sense |
| --- | --- |
| spice | the product; the one binary; the only published crate |
| up | the foreground resident and sole authority process; `spice up` |
| down | the resident's finite state-machine end: quiesce, drain, terminate, kill if required, verify, then dark |
| rim | the binary crate's edge: clap, exit, and env; the only crate allowed any of them |
| journal | the sole authority: one SQLite transaction commits immutable facts, authoritative task statement and workflow heads, coordination heads, command identities, results, and the wake cursor |
| fact | one durable plane-crossing event: decision, recognition, verdict, or transition |
| envelope | the frame every fact wears: v, seq, at, actor, origin, optional cause, and kind |
| kind | the fact discriminant: a path of single words in a closed set; a wire field, never a key |
| seq | the journal's monotone sequence number and durable observation cursor |
| at | the one instant on the wire: RFC3339 UTC, millisecond precision, trailing Z |
| actor | who appended: operator, spice, or `lane:<id>`; the first attribution lens |
| head | an authoritative current-state table in the journal: task statements, workflow, and coordination state updated atomically with their facts |
| witness | one named predicate over a command's subject, such as a task claim head, directive disposition, trust digest, or document barrier |
| bus | the in-process owner of commit capability, guarded transactions, timers, and commit-signal broadcast |
| fold | a derived, generation-stamped, disposable view over declared sources; never authority |
| gen | a fold's generation stamp, published transactionally |
| horizon | the seq through which a fold's declared journal sources are complete |
| source | one declared fold input: fact kind, fold, head table, tracked config, or tape extent |
| wire | the current DTO schema for facts, socket messages, and browser frames; compiled into the app and decoded by one current path |
| rule | one named ordered entry in a decision table; first match answers, and facts or budgets cite the name |
| glob | the one pattern form in configuration: gitignore syntax on paths and star syntax on words |
| hello | the socket opening `{wire, capability, epoch}`; wire mismatch refuses by naming both versions |
| reply | the successful answer a leaf returns; exit 0 |
| refusal | a zero-write decline naming reason and remedy; exit 2; never a fact |
| fault | a store, protocol, or environment defect distinct from refusal and a lost race; exit 1 |
| thread | an origin-and-cause chain end to end; a journal-maintained index beside the heads |
| command | one idempotent request whose id, principal, kind, witnesses, facts, head updates, and result commit in one journal transaction; retry replays the result |
| epoch | the resident generation minted at up; hello and every reply name it, and a client addressing another faults as stale |
| trust | approval state for executable policy, hooks, skill, and vendor digests; changed bytes pend until an operator grant |
| install | the exact writing verb for hooks, skill, tracked config, and unit templates; its durable install record governs `--unapply` |
| builtin | a compiled default sealed by the binary and proof hashes; never a file |
| release | publication of one canonical target manifest and the artifact it identifies |
| recall | one ranked, bounded, typed account of intent, work, pending ACKs, lineage, failures, and repository state, served at seat, renewal, and compaction |
| version | the daemonless identity report: binary hash, wire version, journal schema version, and embedded app digest |
| lock | exclusive flock possession of a named resource: the resident holds the kernel writer lock for its life, and `spice lock` holds a configured resource lock or pool shard for the life of one child command |
| sill | the dependency-free floor crate owning the process door and flock possession: Spec, spawn, wait, and lock |
| Derived | the fold reply frame: fold, gen, cursor, and the `Packed<T>` it wraps; head-direct reads never wear it |
| payload | dist's receipt-free per-target inventory; proof consumes its digest and the release seal binds it |
| migration | the prepared and finalized forward journal transition run at up before epoch mint, with a crash drill and one current result |
| manifest | the one sealed release object: every target entry — binary digest, embedded app digest, toolchain and container identity — bound with its detached proof receipt |
| dist | the build orchestrator: verify pins, build app, stage, embed, build canvas, and prove |
| atomic | the sole filesystem-publication primitive: same-directory temporary, write, sync, rename, directory sync, and crash cleanup |
| Packed | the wire list frame: rows, explicit overflow, and horizon |
| budget | a named bound with a defined act at exhaustion; consent renews queues, pack marks overflow, and run's bound is measured by the gate |

### The board

| Word | Sense |
| --- | --- |
| board | the authoritative shared task surface the fleet orbits; coordination is stigmergy |
| task | one unit of wanted work |
| handle | a task's public name, `KEY-BORN`; KEY derives from title and BORN fixes identity |
| born | a base52 creation stamp; identity, never edited; used inside handles and tape names |
| project | the dotted grouping a task inhabits |
| family | the tasks one document owns, keyed by handle |
| phase | one station of a task's flow: design, plan, todo, verify, or review |
| flow | the ordered phases a task walks |
| urgency | the allocator's scalar, computed purely from authoritative heads |
| next | the sole allocation path: one deterministic, explainable eligibility query |
| claim | exclusive, leased possession of a task by one actor |
| claim::Active | the capability a won claim mints; each consuming command re-proves task, claim, lane, occupant, epoch, lease, and witnesses in its one journal transaction |
| shard | one slot of a configured lock pool; `spice lock` takes the first free shard or the exact one named |
| lease | a claim's deadline; expiry enters through a bus timer, never a sweeper |
| defer | set the deferral that wake clears |
| wake | clear a deferral; a woken row becomes eligible |
| free | release a claim; the row returns to eligibility |
| oops | friction captured blunt as wakeable work; observations are notes |
| depends | the direct-intent verb that wires one dependency edge between existing tasks under exact head witnesses, without a document round-trip |
| graph | one named board diagram rendered from authoritative heads |
| handout | the composed shareable document holding every named board diagram |
| doc | a task document: markdown projected onto the board as a DAG |
| dialect | the document grammar: forgiving read, one strict normal form, round-trip law |
| ledger | the dialect's normal-form export; a byte-stable fixed point |
| settled | a row work has touched; documents report drift there, never write |
| drift | document and board disagree where the board owns the field |
| loose | a family row the document no longer lists; reported, never deleted |
| origin | provenance minted at creation, never edited: `ack:<key>`, `task:<handle>`, `lane:<id>`, or `repo:<rev>` |
| cause | the seq of the provoking fact; absent at a declared chain root |

### The fleet

| Word | Sense |
| --- | --- |
| lane | one supervised agent seat in one worktree |
| occupant | the agent process currently seated in a lane and supervised as one process group |
| Steer, Drive, Drain | the three lane lives: direction held, allocation continued, and work exhausted before end; each is the same sense as its verb |
| vendor | an agent CLI spice operates: claude, codex, or acp |
| listen | the inbound seam from vendor bytes to typed events, residue, and canary state |
| event | one decoded transcript moment: prose, reasoning, tool, usage, compaction, or failure |
| locus | a stable `(member, offset, length)` citation into recorded vendor bytes |
| residue | bytes the decoder could not type; accounted, never dropped |
| canary | the resident vitality instrument; an absent event kind alarms by name |
| tape | vendor bytes committed verbatim before interpretation, active in plaintext and sealed as indexed immutable gzip members; mode-0600 confidential evidence rendered only behind the operator capability |
| watch | daemonless, offline live rendering of detected vendor sessions; the ladder's first rung |
| retro | daemonless, offline replay of recorded sessions; the ladder's second rung |
| steer | the outbound force: a durable directive sent to a lane |
| directive | one durable steering intent; retired only by a typed disposition |
| inbox | the rendered pending directives a lane reads |
| ack | the closed semantic retirement protocol: acked, declined, lapsed, or missed |
| disposition | one typed terminal outcome for a directive or consent request |
| resend | steering's only sanctioned repetition; appends lineage and escalates |
| activation | the seat-time loopback proof whose transcript echo proves the lane hears steering |
| hook | an installed interception point; paths qualify it as `gate::hook` or `steer::hook` |
| run | the agent wrapper; its end-to-end bound is measured only by the gate |
| consent | the pre-action actuator: ACP dispositions recorded as facts under named ordered rules |
| pend | hold a request for the operator while reversible work continues |
| floor | the recorded non-ACP posture, landed once per occupancy as `consent::floored`; silent bypass does not exist |
| hold | the automatic pause on a lane whose occupants repeatedly die young; `lane::held` records it, and `spice lane wake` clears it through `lane::resumed` |
| lineage | the ancestor chain renewal appends and never rewrites; the second attribution lens |
| session | one occupancy's identity and time boundary; the third attribution lens |
| drain | the graceful end: stop admission, finish or preserve claims, retire what evidence permits, end occupants, and verify |
| skill | the generated vendor artifact under `.agents/skills/`, derived from the clap tree and staleness-gated |
| RTK | the optional external tool and its per-thread, per-worktree history; agent context under its own life, never repository authority or authority backup |

The attribution lenses are `actor`, `lineage`, and `session`. `source`
has only its fold-input sense.

### The tastes and the gates

| Word | Sense |
| --- | --- |
| taste | one blunt, curated operator preference judged against live prose |
| judge | the local confirmer of taste hits; confirmed and cleared both land as facts |
| gate | the enforcement surface: staged checks, receipts, and refusals |
| check | one member of the gate's staged battery; it passes, fails, or is an explicit unsupported or unmeasured state, never a silent skip |
| ratchet | regression-only enforcement against a file's own inherited measure |
| baseline | the versioned measure a ratchet defends; it shrinks on improvement and rises only by operator commit |
| camp | the standing audit for implausible pile-ups at any limit |
| kill | the mutation ratchet; failures require N-run confirmation before the latch moves |
| receipt | a verdict pinned to tree, gate-set digest, and policy digest; old lineage remains valid history |
| corpus | a recorded fixture set replayed as law: dialect refusals, vendor streams, and proof battery |
| evidence | recorded support a standing thing cites; it ages and is never inferred |
| proof | the sealed replay that verifies one target manifest's binary, embedded app, toolchain, container, and evidence |
| doctor | daemonless environment truth stated once; it prints and appends nothing |

### The canvas

| Word | Sense |
| --- | --- |
| canvas | the resident's loopback operator surface; read, steer, and grant surfaces derive from the presented capability |
| app | the canvas browser body: a pure wire consumer compiled to wasm and embedded by dist |
| magnitude | the view of effort, pressure, residue, and consent rates |
| flow | the view of task flows in motion; paths distinguish `canvas::flow` from a task's flow |
| topology | the view of lanes, worktrees, and lineage |
| chronology | the view of time: compaction, consent, trust, commands, recovery, and time blocked on the operator |
| rank | order candidates by an inspectable domain key |
| pack | fit ranked candidates into `Packed<T>` and report overflow and horizon |

The four axes remain the derivation law. Consent appears on magnitude
and chronology; trust, commands, and recovery appear on chronology.
They are panes on those axes, not new views.

### Files of record

| File | Sense |
| --- | --- |
| DOCTRINE.md | the doctrine of record: kernel invariants, decision records, interregnum, and cutover law |
| NAMES.md | this lexicon |
| LATER.md | the named waitlist and clean-break drop ledger |
| TASTES.md | the authoring and curation doctrine for tastes |
| docs/record/ | the archived gauntlet corpus and review record |

## The Web

Each crate word's full manifest. The table has one row for every
workspace member and maps each word to its surfaces; the dependency
arrows live in TREE.md alone.

| Word | Crate | CLI | Kinds | Folds | At rest |
| --- | --- | --- | --- | --- | --- |
| spice | spice | every leaf through the rim; install, version | `up::*` admission and process outcomes | — | generated skill, hooks, unit templates, install record; seeds tracked spice.toml and baseline.toml |
| wire | wire | — | every fact kind and command DTO | — | compiled into app and target binary |
| journal | journal | journal, replay, reindex, verify, backup, restore | all durable facts | — | `data/spicejournal.sqlite3`, `seq` |
| bus | bus | — | guarded append of all facts | — | — |
| fold | fold | fold, journal replay | —; refusal telemetry is not fact | every disposable materialization | `data/spicefolds.sqlite3` |
| dialect | dialect | doc, doc ledger | `doc::applied` input | — | operator-owned task documents |
| board | board | board, board graph, board handout, task, doc apply | `task::*`, `claim::*`, `doc::applied` | — (feeds `canvas::flow` and `canvas::magnitude`) | task, workflow, and coordination heads in the journal |
| listen | listen | watch, retro | `canary::*`, `vendor::armed` recognition | canary answer | — |
| tape | tape | retro | — | — | `tape/` active and sealed members |
| vendor | vendor | — | `vendor::armed` | — | bytes preserved by tape |
| lane | lane | up, down, lane, lane add, lane drain, lane wake | `lane::*`, `claim::renewed`, `up::*` | feeds `canvas::topology` | residency, lease, and hold heads in the journal |
| steer | steer | steer, steer cancel, inbox, ack, run | `steer::*` (the ack module recognizes acked, declined, lapsed, missed) | pending inbox | journal directives, `inbox/`, `hooks/` |
| consent | consent | consent, allow, deny, policy | `consent::*` | consent queue, magnitude, chronology | journal heads and tracked `[consent]` config |
| taste | taste | taste, taste add, taste drop | `taste::confirmed`, `taste::cleared` | active tastes | journal facts, tracked `[taste]` config, `TASTES.md` |
| gate | gate | gate, camp, kill, receipt | `gate::found`, `gate::cleared`, `commit::landed` | verdict and chronology answers | `baseline.toml`, Git receipts |
| canvas | canvas | canvas | — | packed magnitude, flow, topology, chronology | `canvas` discovery file |
| app | app | — | — | packed frames consumed | embedded in each target binary |
| proof | proof | proof, proof replay, release publish | `proof::sealed`, `release::published` | release history | `proof/<rev>` |
| dist | dist | — | — | — | staged app and canonical target manifest |
| atomic | atomic | — | — | — | every filesystem publication: markers, inbox, discovery, degraded, tape rotation, baseline, and manifest writes ride it |
| sill | sill | lock, lock run | — | — | `locks/` holder metadata; the kernel-lock and pool-shard flocks |

## Retired

| Was | Now | Why |
| --- | --- | --- |
| serve | up, canvas | the process and surface were one word wearing two jobs |
| study, studies | gate | enforcement is one organ, not a curriculum |
| flex, sticky, jitter | ratchet | elastic limits invited camping; own-history has no fixed wall |
| ingest | doc | the verb named mechanism; the noun names the thing |
| taskdoc | doc, dialect | the document and its grammar are two words |
| incepted | born | plainer and obvious in retrospect |
| inbox item | directive | the item was a shape; the directive is intent |
| conscience | taste | the plane is named by its unit |
| maxim | taste | the definition was already the name on every surface |
| projection | fold | the substrate's own word everywhere |
| watch UI, observer mode | watch | daemonless terminal rendering has no resident socket |
| unclaim | free | the prefix was a compound in disguise |
| lifetime | life | the three lane lives shed the compound's tail |
| driver | vendor | one foreign seam, one word |
| team | lineage | the only grouping facts mint is renewal ancestry |
| supervisor process | occupant, lane | seats and their occupants are the governing ideas |
| constitution | gate | enforcement is a surface, not a polity |
| bypassPermissions | consent::floored | the posture is recorded; silent bypass does not exist |
| replica | head | current task, workflow, and coordination state now commits with facts under one authority |
| briefing | recall | recall is bounded, ranked, typed, and served at every continuity boundary |
| source (attribution lens) | actor | attribution names who appended; source keeps only its fold-input sense |
| taskchampion | deleted | one journal transaction owns task and coordination state, so a second authority has no role |
| ceiling | floor, baseline | measures are lower-is-better debt defended from below; nothing enforces from above |

This table is the source of the generated gate deny-list over public
symbols, config keys, and clap names.
