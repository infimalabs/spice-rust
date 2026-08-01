# FRAME

The skeletal form of spice — RESOLVED. Every fork is decided here per
the fleet's convergence; nothing below is open. ☒ throughout. Joints
J1–J85 are the permanent addresses; SCAFFOLD.md maps them into the
documents. If only these two files survive, the system can be rebuilt
from them.

Decided is not embalmed. Construction will teach, and when it does the
joint amends by deliberate edit: the new resolution lands here first
and the companion documents follow in the same change. The addresses
are permanent; their contents evolve on evidence, never by drift.

─────────────────────────────────────────────────────────────────────────
## I. THE SPINE (unchanged, preserved without relitigation)
─────────────────────────────────────────────────────────────────────────

J1  ☒ spice; one binary; resident = `spice up`; ladder = topology
       (watch → retro → gate → steer → fleet); hermetic rungs daemonless.
J2  ☒ Naming law: one word; paths qualify; compounds = package
       boundaries; case invisible; retired terms name successors;
       foreign skins kept; UUIDs lower-case 8-4-4-4-12.
J3  ☒ Clean break: no shared/imported authority state, ever; drain,
       export wanted work as a task document, back up, start empty.
J4  ☒ One resident writer; kernel lock before any authority opens;
       epoch for stale clients; no live takeover in release one.
J5  ☒ Tape: vendor bytes verbatim before interpretation; typed decode,
       absence, residue, canary; sediment is forensics only.
J6  ☒ Steering: durable directives; semantic ACK (acked, declined,
       lapsed, missed) plus operator cancel; disposition evidence is a
       transcript locus or an authenticated lane command; resend lineage
       escalates and exhaustion alarms by name; activation = loopback
       proof; deafness is a gate failure plus degrade-once evidence,
       never a runtime refusal (J81).
J7  ☒ Consent: ACP dispositions as facts; named ordered rules; budgets
       queue renewal; recorded floor for non-ACP.
J8  ☒ Folds: derived, gen-stamped, disposable, transactional publish,
       never read by authority, declared recovery.
J9  ☒ Wire DTOs compiled into the browser; one decoder; no emission.
J10 ☒ Rank-then-pack; overflow explicit; block-or-silent channels.
J11 ☒ Reply/Refusal/Fault; exits 0/2/1; withdrawn surfaces refuse
       naming their release.
J12 ☒ `#[expect(lint, reason)]`; deny-by-default; rim-only exit/env.
J13 ☒ Own-history ratchets; camping audited; mutation-kill anchor.
J14 ☒ Config: domain tables, single-word keys, glob only, named
       first-match rules, duration words, compiled builtins, deletion
       disables, one emission with provenance.
J15 ☒ Stigmergy: board-only coordination; hermetic rungs append
       nothing; resident recognizes disk evidence.
J16 ☒ Taste: blunt curated preference; judged; confirmed/cleared facts;
       operator commits govern membership.
J17 ☒ Never reopened: TW subprocess seam, taskrc, reports, rc
       overrides; JS emission + source-text UI tests; vendor files as
       citations; agent-to-agent messaging; multi-user authority;
       hidden projects; regex policy; anonymous rules;
       flex/sticky/jitter; silent skips; bundled old gate arms.

─────────────────────────────────────────────────────────────────────────
## II. THE MASTER FORK — RESOLVED
─────────────────────────────────────────────────────────────────────────

J18 ☒ ONE AUTHORITY. TaskChampion is retired entirely. The journal
       transaction owns immutable facts, AUTHORITATIVE TASK STATEMENT
       AND WORKFLOW HEAD TABLES, coordination heads, command receipts,
       and the wake cursor — one SQLite transaction commits all of it.
       Rationale of record: release one had already removed sync, TW
       interop, external writers, import, recurrence, working-set
       numbering, and undo — the seam repaid nothing; the old NO-GO
       reasoning was migration-bound and the clean break dissolved it.
       Cascade (all resolved): the three-step ledger, fingerprints,
       divergence states, FencedStorage, board/replica.rs, paired
       backup, dual migrations, composite cursors — all DELETED. Every
       command is one journal transaction (id + facts + head updates +
       result, atomic). Crash matrix = journal seams only. The board =
       head tables + packed rendering (no board fold). taskchampion
       leaves the dependency list. taskchampion.sqlite3 leaves DISK.
       `replica` is RETIRED in NAMES (successor: head). Flow, phase,
       review state, claims, witnesses: all journal — one coordination
       concept, one store. The measured scars (4ms/40ms, double-claim)
       remain as evidence in the battery root.

─────────────────────────────────────────────────────────────────────────
## III. THE RESOLUTIONS
─────────────────────────────────────────────────────────────────────────

### Principal & trust
J19 ☒ Threat model: COOPERATIVE same-user occupants. Trust and actor
       checks are workflow interlocks and attribution, not security
       boundaries. Adversarial isolation → LATER (trigger: a second
       Unix identity actually runs a lane).
J20 ☒ Principals are connection capabilities: operator capability
       (created by up, held via the canvas discovery file) and lane
       capabilities (minted at seat, bound to occupant epoch). Actor,
       origin rights, and lane derive server-side at accept;
       caller-supplied attribution is data. Command ids bind the
       authenticated principal.
J21 ☒ Trust bootstrap: fixed-code quarantine boot — open journal +
       operator-only IPC, parse discovery grammar, digest WITHOUT
       executing, accept grants, then admit the fleet. No restart
       deadlock by construction.
J22 ☒ Verbs: `spice trust` / `trust grant DIGEST` / `trust revoke
       DIGEST` — resident band, operator capability only.

### Wire
J23 ☒ Three clocks separated: socket = current-version-only hello
       (refuse naming both); journal-at-rest = ONE forward migration
       at open (J37), never upcast-forever arms; browser = same
       binary, no clock. One current path everywhere.
J24 ☒ `expected` = subject witnesses: per-command named predicates
       (claim head of task X; disposition of directive K; trust digest
       state; doc-apply board barrier). Global seq only for
       whole-snapshot reads.
J25 ☒ Refusal trace: refusals are never facts; they land as bounded
       rows in the folds store via the bus, and chronology declares that
       ring as a named non-fold source. The rows are not re-derivable:
       replay and backup exclude them, and that loss is accepted out
       loud wherever folds are called disposable. The journal records
       the world; the operator usually sees the argument.
J26 ☒ hello{wire, capability, epoch}; replies name resident epoch;
       stale clients faulted by name.
J27 ☒ Vocabulary closed: + up::quiescing/draining/terminating/killing/
       verifying/dark, trust::revoked, claim::renewed, gate::found,
       gate::cleared, journal::migrated/backed/restored,
       steer::canceled, lane::held/resumed. Chain root = absent cause
       (the acceptance line amends to match). Every kind has a
       registered fold or dies, and every kind names its producer —
       admission is a generated check in both directions (J74).
J28 ☒ `doc --apply=<digest>` asserts the exact normalized preview
       digest; documents carry representable project and origin.

### Journal, folds, board
J29 ☒ Capability types: JournalRead / JournalGuard / JournalCommit;
       commit constructable only inside bus. Board contributes
       precondition closures INTO the actor's transaction — it never
       holds the writer (resolves the C1 cycle).
J30 ☒ The board is authority: head tables read directly (the join
       died with J18). Folds exist for canvas materializations only.
J31 ☒ Folds may declare fold and head-table sources by name; the
       registry lists them; admission covers both.
J32 ☒ One seq cursor (single authority). Non-fact inputs (tracked
       config, tape extents) declared by name in `source`.
J33 ☒ (absorbed by J29.)

### Disk
J34 ☒ Final topology: data/spicejournal.sqlite3 (SOLE authority),
       data/spicefolds.sqlite3 (disposable), tape/, locks/, resident,
       seq, canvas, inbox/, degraded/ (per lane), install, proof/<rev>,
       hooks/(+static), config.toml; tracked spice.toml + baseline.toml
       (versioned); RTK worktree-local under its own life.
J35 ☒ Verbs: `spice backup OUTPUT` (quiesced single-db snapshot +
       tape manifest), `spice restore BUNDLE [--apply=<digest>]`,
       `spice restore verify BUNDLE`. Enumerated filenames, no globs.
       Restore is a lock-held recovery protocol outside the command
       door: verify and digest-pin the plan, stop admission, close
       authority, exchange staged bytes atomically, reopen and verify,
       mint a new epoch, then recognize journal::restored.
J36 ☒ Locus: active plaintext segment, sealed on rotation into
       indexed immutable gzip members; locus = (member, offset,
       length); survives compression and archive. Tape is confidential
       evidence: mode-0600 files under a 0700 tape/, exported only
       inside operator-held backup bundles, rendered only behind the
       operator capability; fixtures prove no tape byte crosses an
       unauthenticated surface.
J37 ☒ Migration is a command: prepare/finalize + crash drill; runs at
       up before epoch mint; schema version row in the journal;
       baseline.toml carries a version field.
J38 ☒ Lock mechanics: flock + CLOEXEC + spawn-only-via-exec (children
       never inherit); the fork-path test is in the battery.

### CLI
J39 ☒ Closure: every verb has a battery owner. Added families: trust
       (J22), backup/restore (J35), recall (J48), version (J46),
       lock (resource locks), board graph/handout (J41), task depends.
       A GENERATED CLI contract table (per leaf: principal, authority
       touched, result class, facts minted, battery owner) is a
       standing gate check that fails in both directions.
J40 ☒ Hermetic split into three labels: daemonless / offline /
       toolful. watch, retro = daemonless+offline; gate = daemonless+
       toolful; proof = daemonless, container+network posture named.
J41 ☒ `spice board [FAMILY]` — bare: whole board packed; with a
       family/handle: full depth, packed. The mid-zoom exists.
       `board graph VIEW` renders one named board diagram and
       `board handout OUTPUT` composes every named diagram into one
       shareable document; both are reads over authoritative heads.
J42 ☒ Allocator contract precedes postures: ONE deterministic,
       explainable eligibility query; task next is the sole allocation
       path; lane lives are policy parameters over it.
J43 ☒ oops = wakeable work, one meaning. Observations are notes.
J44 ☒ down = state machine: Seated → Quiescing → Draining →
       Terminating → Killing → Verifying → Dark; entry fact, deadline,
       admitted commands per state; never forges ACKs; durable residue
       named. All seven states are durable: each has its up::* entry
       fact, so a stalled shutdown resumes from the state it reached.
J45 ☒ `gate [--at REV]` = current engine over the selected tree with
       current policy. Old verdicts are receipts (data), never arms.
J46 ☒ `spice version` prints binary hash, wire version, journal
       schema version, and embedded app digest — compiled identities
       only, readable without a resident. The appliance can identify
       itself.

### Fleet
J47 ☒ The skill is a generated artifact from the clap tree, staleness-
       gated like everything generated. No fossil surface.
J48 ☒ `recall` — the missing product, named: one ranked, bounded,
       typed reply (operator intent, active/next task, pending ACKs,
       lineage, recent failures, repo state) at a composite read
       barrier. Verb `spice recall [LANE]`; auto-served at seat,
       renewal, and compaction. Replaces briefing everywhere; NAMES
       adds recall; the skill commands it.
J49 ☒ Renewal: resident-automatic on observed liveness + cadence;
       epoch-bound; maximum lease; a successor cannot renew an
       ancestor's token; claim::renewed is the fact. The timer wheel is
       a derivation over deadline-bearing heads — recomputed at epoch
       mint and after every deadline-touching commit, never a registry
       commands write into. Deadlines persist UTC; waits run on
       monotonic clocks; observed wall-clock regression faults by name.
J50 ☒ Compaction is answered: lane::compacted triggers recall
       injection + renewal check. Wired, not just observed.
J51 ☒ Resident life: foreground process; shipped systemd-user and
       launchd templates are install artifacts; `spice down` is the
       graceful end; no self-daemonize (one path).
J52 ☒ Daemonless rungs carry no drift alarm — stated honestly in CLI;
       the canary is a resident instrument.
J53 ☒ RTK: declared optional external tool (doctor-visible, config
       seam named); no dormant code paths; worktree-local data stands.

### Taste & gate
J54 ☒ Ratchet honesty: baselines are SHRINK-ONLY FLOORS — they move
       down when the fleet improves, up only by operator commit;
       regression-only enforcement; renamed from ceiling vocabulary.
       Every stored measure is normalized to lower-is-better debt
       (mutation stores SURVIVOR rate, never kill rate), so one
       downward comparison law governs every floor.
J55 ☒ Baseline integrity: versioned; kill-rate deflaked (N-run
       confirmation) before latch; re-derived inside the sealed proof.
J56 ☒ Findings ride receipts; the resident appends one fact per new
       finding at observation (origin gate): gate::found on discovery,
       gate::cleared on resolution, with the Finding head answering
       current posture. The loop closes.
J57 ☒ Receipts pin (tree, gate-set digest, policy digest). A set
       change starts new lineage; old receipts remain valid history.
J58 ☒ TASTES.md gets its authoring leaf. The gauntlet corpus archives
       under docs/record/. LATER carries the clean-break drop ledger.
J59 ☒ Tests: Rust idiom — `#[cfg(test)]` units may see crate
       internals; `tests/` integration uses public surface only;
       shared corpus fixtures; private-internals doctrine maps to
       exactly that split.
J60 ☒ Interregnum: Python spice is the NAMED scaffold authority until
       the enforcement-flip leaf; the Rust gate runs measure-only from
       wave 1; the flip lands after the vertical slice proves.

### Canvas & app
J61 ☒ Views re-derived from the vocabulary: the four axes stay
       (magnitude, flow, topology, chronology); the new planes surface
       as panes on them (consent → chronology + magnitude; trust →
       chronology; commands/recovery → chronology). One derivation
       leaf audits the mapping.
J62 ☒ `dist` — the build orchestrator (cargo xtask pattern): verify
       pins → build app (pinned trunk, wasm target) → stage → embed →
       build canvas → emit the receipt-free target payload. A workspace
       binary the shipped appliance does not link. Owns the
       cargo-invisible edge; the rim orders dist → proof → seal.
J63 ☒ One pack law, one type: Packed<T> (rows, overflow, horizon)
       lives in wire; every list crosses as it; app and canvas render
       it. Two renderers, one contract. Fold-sourced replies wrap it in
       Derived<T> {fold, gen, cursor, packed}; head-direct reads never
       carry fold identity, so neither shape lies about provenance.
J64 ☒ Canvas capabilities: read vs steer vs grant surfaces derive
       from J20 principals. The loopback server binds 127.0.0.1 only,
       validates Host and Origin against its own endpoint, and accepts
       the capability solely inside the hello payload — never from a
       URL, query string, or cookie. The mode-0600 discovery file is
       the one capability transport.
J65 ☒ proof consumes dist's receipt-free target payload; the sealed
       release manifest proves the appliance contains the app.

### Release & proof
J66 ☒ The release pipeline is acyclic. dist emits one RECEIPT-FREE
       target payload per tuple (binary digest, embedded app digest,
       toolchain + container identity); proof consumes that payload
       digest and emits a DETACHED receipt; a post-proof seal composes
       ONE canonical release manifest holding both target entries and
       their receipts; publication consumes only the sealed manifest.
       Targets = x86_64-unknown-linux-musl (sealed Linux container)
       and aarch64-apple-darwin (named native sealed runner). "Any
       copy is every copy" holds per target.
J67 ☒ Supply contract: committed Cargo.lock, vendored crates in the
       proof container, network-disabled empty-cache build.
J68 ☒ One process door: Spec{argv, cwd, env, group, deadlines,
       output, redaction, grace}; every wait names owner, bound,
       cancellation, restart; tests use deadlines and process groups,
       never sleeps. Owner: the sill floor crate (spec, spawn, lock) —
       dependency-free, reachable by every spawning crate; callers
       pass env explicitly, so the rim's environment denial stands.
       Generated blocking-surface check stands (J74).
J69 ☒ `atomic` — the one filesystem-publication primitive (same-dir
       temp, write, sync, rename, dir-sync, crash cleanup); markers,
       inbox, discovery, degraded all ride it.

### The genesis graph
J70 ☒ Wave 0 = the decision records: the five forks land as doctrine
       leaves RECORDING these resolutions (authority, principal,
       clocks, release object, resident life) — no open design remains.
J71 ☒ The five ordering inversions repaired: trust after its
       authority; board after the store; assembly after task
       authority; activation after an admitted lane; the closure seed
       directly after the doctrine record. All five are named — a
       binding record elides nothing.
J72 ☒ Law leaves gain dependents: every wave-1 leaf After's the
       doctrine leaf; the terminal leaf's closure is generated (J74),
       and its claim matches its edges.
J73 ☒ The vertical slice: wave 1 lands one end-to-end path — fact → guarded
       append → head → fold → measure-only gate → static binary.
J74 ☒ Generated closure gates: CLI contract coverage, fact-fold
       admission (bidirectional: every kind consumed, every kind
       produced, every fold answer resolvable), graph/release closure,
       blocking surface, durable inventory, corpus conformance (the
       documents of record against NAMES, WIRE, and the contract
       tables), lattice conformance (each Cargo.toml against TREE's
       arrow list, failing both directions). Checks, not prose.
J75 ☒ Pre-apply hygiene: retired words swept from leaf titles (slugs
       freeze at apply); handle re-mint at cutover is named in the
       doctrine.

### Lexicon
J76 ☒ Attribution lenses renamed: actor, lineage, session. `source`
       keeps one sense (fold input).
J77 ☒ Lane lives stay Steer, Drive, Drain. The collision finding is
       resolved in the law, not the lexicon: one word may serve as
       mode and verb when they carry ONE sense — the life Steer is the
       verb steer standing still; case marks grammar, never meaning.
       Law 3 gains that clause. No vocabulary is manufactured.
J78 ☒ The Retired table binds: a generated gate deny-list over pub
       symbols, config keys, and clap names, derived FROM the table.
J79 ☒ Spice-Origin v1 grammar frozen explicitly; any change is a new
       trailer name, never a mutation.
J80 ☒ Config rule restated: tables are DOMAINS (today coinciding with
       crates); refactors never move operator surface.
J81 ☒ run's budget is gate-measured only; the runtime-refusal clause
       is struck (a wrapper that refuses commands would break the
       agent; regression is the enforcement).

─────────────────────────────────────────────────────────────────────────
## IV. THE ORDER
─────────────────────────────────────────────────────────────────────────

J82 ☒ Build order: wave 0 decision records → workspace pin + day-zero
       hooks + measure-only gate → wire v1 (envelope, commands,
       witnesses, Packed) → journal kernel (sill, atomic, guard, head,
       epoch, lock, resource locks) → the vertical slice → board
       (tasks-as-heads, claims, allocator, graphs, handout) → dialect +
       doc → steering + ack + recall → resident (quarantine boot →
       trust → consent → lanes + vendor + tape + listen + canary +
       daemonless rungs) → taste → gate full frame + enforcement flip →
       canvas + app via dist → proof + sealed release manifest +
       cutover.

─────────────────────────────────────────────────────────────────────────
## V. THE LEDGER
─────────────────────────────────────────────────────────────────────────

J83 ☒ 85 joints; 0 open. The workspace is 21 crates: spice, wire,
       journal, bus, fold, dialect, board, listen, tape, vendor, lane,
       steer, consent, taste, gate, canvas, app, proof, dist, atomic,
       sill. ack is a module of steer: every fact it mints wears
       steer's prefix and steer is its one consumer.
J84 ☒ Deleted by resolution: taskchampion (dependency, file, crate
       module, NAMES rows), the three-step ledger, fingerprints,
       FencedStorage, board::replica, composite cursors, task sync,
       briefing (successor: recall), the runtime run-refusal clause,
       the ack crate (successor: the steer::ack module), the browser
       inside sealed proof (successor: digest-and-serve verification
       plus the fixture corpus).
J85 ☒ The epigraph stands: the right large cuts were already made.
       The forks are now decided. Prove the slice, then build in
       order; when building teaches better, amend this record rather
       than drifting from it. The rest is population — see SCAFFOLD.md.
