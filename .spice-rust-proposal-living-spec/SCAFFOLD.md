# SCAFFOLD

The population map. Every document of the final corpus, every section it
must contain, every entry each section must hold — with `[POP]` marking
where your voluminous prose goes and `[J#]` naming the governing joint
in FRAME.md. Write into the slots top to bottom; no slot requires a
decision, only words. Documents marked (rewrite) contradict the
resolutions and are regenerated from this outline; (amend) means the
existing file stands with the listed edits; (new) has no predecessor.

═══════════════════════════════════════════════════════════════════════
## DOCTRINE.md (new)
═══════════════════════════════════════════════════════════════════════
1. The kernel — one paragraph per invariant [POP]:
   transcript-as-truth · board-as-steering-surface · stigmergy [J15] ·
   one authority, one writer [J18, J4] · clean break [J3] · loud
   refusal naming its release + its remedy [J11] · no polling: signals
   are hints, cursors are truth · block-or-silent [J10] · operator
   reads outcomes, never code · evidence before surface (LATER
   discipline).
2. The decision records (wave-0 doctrine) — one subsection each,
   stating the resolution + rationale + what it deleted [POP]:
   2.1 One authority [J18]  2.2 Principals & cooperative threat model
   [J19–J20]  2.3 Three clocks [J23]  2.4 The target manifest [J66]
   2.5 Resident life [J51].
3. The why-Rust ledger [POP: the dependency ledger, compiler-as-law,
   measured floors, distribution collapse].
4. The tastes as repo truth — pointer to TASTES.md.
5. The interregnum: Python spice as named scaffold authority; the
   enforcement flip [J60] [POP].
6. Handle re-mint at cutover, named [J75] [POP: one paragraph].

═══════════════════════════════════════════════════════════════════════
## NAMES.md (amend)
═══════════════════════════════════════════════════════════════════════
Laws 1–6: keep, with law 3 amended per [J77] (one sense may wear
noun and verb roles; case marks grammar, never meaning).
System table — ADD rows: recall [J48] · version [J46] · witness
  (subject predicate sense) [J24] · migration [J37] · manifest [J66] ·
  dist [J62] · atomic [J69] · Packed [J63] · head (authoritative
  current-state tables; successor of replica) [J18].
System table — AMEND rows: command (one journal transaction) [J18] ·
  epoch [J26] · budget (strike run-refusal clause) [J81] · journal
  (sole authority incl. task statements) [J18].
Board table — AMEND: claim::Active (re-prove in the one transaction);
  DELETE replica/adopt rows.
Fleet table — lives stay Steer, Drive, Drain (law 3 gains the
  one-sense/two-roles clause [J77]); lenses to actor, lineage, session
  [J76]; briefing→recall [J48].
Canvas table: keep; note panes derivation [J61].
Files table: DOCTRINE, NAMES, LATER, TASTES + docs/record/ archive
  [J58].
The Web — regenerate for 21 crates [J83] [POP: one row per word].
Retired — ADD: replica→head · briefing→recall · source(lens)→actor ·
  taskchampion→(deleted, J18) [each with one-clause why] [POP].
NOTE: the Retired table is generated INTO a gate deny-list [J78].

═══════════════════════════════════════════════════════════════════════
## TREE.md (rewrite)
═══════════════════════════════════════════════════════════════════════
1. Preamble: 21 members, publish=false, one pin (+wasm target), deny
   lints, expect-only [J12].
2. The tree — per crate, module list with one-line roles [POP each]:
   spice (rim: main, cli, up; assets/skill GENERATED [J47]; unit
     templates [J51])
   wire (envelope, fact, hello, reply, packed [J63], witness [J24])
   journal (guard [J29 capabilities], head [J18: task statement +
     workflow + coordination tables], seq, migrate [J37], replay)
   bus (writer ownership, timer)
   fold (contract, store, inventory; fold+head+ring sources [J31])
   dialect (classifier, node, ledger, refuse)
   board (tasks-as-heads [J18], claim, phase, urgency, next [J42],
     apply [J28], oops [J43], depends, graph, handout [J41])
   listen (event, claude, codex, residue, canary, watch, retro)
   tape (tee, seal [J36], read)
   vendor (record, claude, codex, acp)
   lane (table, occupant, life [Steer/Drive/Drain], hold [J27:
     lane::held/resumed], drain [J44], renew [J49])
   steer (directives incl. cancel, inbox, hook, run [J81], ack/
     [keys, scan — a module, not a crate])
   consent (policy, budget, pend)
   taste (bag, judge)
   gate (walk, measure, ratchet [J54], camp, kill [J55], receipt
     [J57], message, deny [J78], contract-table check [J39])
   canvas (axum, view [J61], thread; renders Packed)
   app (socket, state, pane, thread, view/; renders Packed)
   proof (container, replay, evidence; consumes dist payload [J65])
   dist (pins → app → stage → embed → canvas → payload [J62];
     workspace binary, never linked by spice)
   atomic (the one publish primitive [J69])
   sill (spec, spawn, lock — the process door and flock floor [J68])
3. The lattice — arrows, acyclic; wire+dialect+atomic+sill floor; gate
   journal-free; app ← wire only; canvas embeds dist output; every
   spawner reaches sill and every publisher reaches atomic [POP:
   the full arrow list, held by the lattice-conformance gate [J74]].
4. Postures table [POP: async/blocking/std-only per crate; the
   bus-owns-writer note; run measured end-to-end by gate].
5. Rim enforcement (clippy.toml + the two expects) [J12].
6. Externals — final list WITHOUT taskchampion; nix note; notify-as-
   hint note; pinned tools (trunk, container, cargo-mutants) [POP].

═══════════════════════════════════════════════════════════════════════
## CLI.md (rewrite)
═══════════════════════════════════════════════════════════════════════
1. Preamble: result triad, refusal grammar, direct-intent vs
   authored-input, three hermetic labels [J40].
2. Daemonless band [POP: one line per verb]:
   watch · retro · doctor · version [J46] · install
   [--apply=<digest>|--unapply] · lock · lock run [J39] · restore
   verify [J35] · gate [--at REV] [J45] · gate camp · gate kill ·
   gate receipt · proof · proof replay · release notes INPUT.
3. Resident band [POP]:
   up [J51] · down [J44 states named] · board [FAMILY] [J41] ·
   board graph VIEW / board handout OUTPUT [J41] ·
   task next/add/done/review/note/defer/wake/free/oops/depends [J43,
   J39] ·
   doc / doc --apply=<digest> [J28] / doc ledger ·
   lane / lane add [--life] [--vendor] / lane drain / lane wake ·
   steer / steer cancel · inbox · recall [LANE] [J48] ·
   consent / allow / deny / policy ·
   taste / taste add / taste drop ·
   trust / trust grant DIGEST / trust revoke DIGEST [J22] ·
   backup OUTPUT / restore BUNDLE [--apply=<digest>] [J35] ·
   journal / journal replay [FOLD] / journal reindex / journal verify ·
   fold · thread REF · canvas · release publish [--apply=<digest>].
4. Agent band (Labels column: daemonless/toolful posture per leaf):
   run [J81] · ack [J6 command evidence] · config.
5. Discipline bullets [POP: one short paragraph each]: origin minting ·
   config-never-policy · defaults compiled · every listing packs
   (Packed) [J63] · degrade-once · renewal automatic [J49] ·
   doc-apply sole editor · doctor appends nothing · meters on
   magnitude · missing-verbs (never list + LATER list) · generated
   contract table [J39].
6. One session end-to-end [POP: the example, updated verbs].

═══════════════════════════════════════════════════════════════════════
## WIRE.md (rewrite)
═══════════════════════════════════════════════════════════════════════
1. Envelope (v, seq, at, actor, origin, cause-absent-at-root [J27],
   kind-tagged) [POP: field glosses].
2. hello {wire, capability, epoch} [J26]; UUID form law.
3. Commands: ONE journal transaction — id + witnesses [J24] + facts +
   head updates + result, atomic; retried id replays [J18] [POP:
   envelope + reply + witness catalog per command].
4. Refusal telemetry: bounded rows in folds store, never facts [J25].
5. The one authority: what head tables hold (task statements, workflow,
   claims, leases, witnesses, trust, residency, cursor) [J18] [POP].
6. The crossing rule + hermetic/stigmergic boundary [J15].
7. The vocabulary — closed list [POP: gloss per kind]: task::* ·
   claim::won/carried/freed/lapsed/renewed [J49] · doc::applied ·
   steer::* · consent::* · lane::* · taste::confirmed/cleared ·
   canary::* · vendor::armed · trust::pended/granted/revoked [J27] ·
   commit::landed · gate::found/cleared [J56] · journal::migrated/
   backed/restored [J35, J37] ·
   up::seated/quiescing/draining/terminating/killing/verifying/dark
   [J44: all seven durable] ·
   proof::sealed · release::published.
8. Folds: six answers; fold+head+ring sources [J31, J25]; the registry
   [POP: every row with sources, incl. bus::refusals honesty];
   fold-on-fold horizon = min over inputs; thread as index; Packed as
   the read frame with Derived for fold-backed reads [J63].
9. The one decoder; three clocks [J23].

═══════════════════════════════════════════════════════════════════════
## DISK.md (rewrite)
═══════════════════════════════════════════════════════════════════════
1. The appliance (per-target identity [J66]; compiled defaults).
2. Tracked: spice.toml · baseline.toml (versioned, shrink-only floors
   [J54–55]) · .agents/skills/spice (generated [J47]).
3. Untracked topology [J34] [POP: gloss per path]: data/spicejournal ·
   data/spicefolds · locks/ [J68] · resident [J38 lock mechanics] ·
   seq · canvas (endpoint+capability+epoch tuple [J64]) · inbox/ ·
   degraded/ per lane · install record · proof/<rev> · tape/ [J36
   locus + confidentiality model] · hooks/(+static) · config.toml.
4. RTK worktree scope [J53].
5. Ownership law: sole authority; durable vs disposable; backup
   enumerates filenames [J35].
6. Operations discipline: verify · backup/restore · crash drills ·
   migration-as-command [J37] · retention [POP].
7. The clean cutover [J3] [POP].
8. Vendor sediment (forensics only).
9. Lifecycles table [POP: one row per path].

═══════════════════════════════════════════════════════════════════════
## CONFIG.md (amend)
═══════════════════════════════════════════════════════════════════════
Rule 1 restated: tables are domains [J80]. Rule 11 added: the generated
per-key registry with a default-deny local layer. Rules 2–10 keep (glob-only,
named rules, duration words, no-knob list gains listen decision:
canary window IS a knob → [listen] table with window [POP]). The
tree [POP: final table set — no board.sync]. The three files. The
superseded table.

═══════════════════════════════════════════════════════════════════════
## CANVAS.md (amend)
═══════════════════════════════════════════════════════════════════════
Keep the decision + grounds. AMEND: pack section to Packed-in-wire
[J63]; add capability surfaces [J64]; add the derivation section
mapping new planes onto the four axes [J61] [POP]; dist owns the build
edge [J62]; proof consumes the manifest [J65].

═══════════════════════════════════════════════════════════════════════
## TASTES.md (new) [J58]
═══════════════════════════════════════════════════════════════════════
1. What a taste is [POP]. 2. Authoring rules (blunt, near-universal,
word-glob bags) [POP]. 3. Curation lifecycle (evidence ages; drop by
commit) [POP]. 4. The starter set install seeds [POP: each taste].

═══════════════════════════════════════════════════════════════════════
## LATER.md (new)
═══════════════════════════════════════════════════════════════════════
1. Never (doctrine): multi-user authority · agent-to-agent messaging.
2. Waiting (each with its evidence trigger) [POP]: northbound ACP
   server · query DSL · plugin API · multi-machine authority (absorbs
   task sync) · adversarial isolation [J19] · Windows · tape pruning ·
   migration bridge (external demand only).
3. The drop ledger of the clean break [POP: what Python had that has
   no successor, each named] [J58].

═══════════════════════════════════════════════════════════════════════
## GENESIS.md (rewrite — wave and leaf skeleton)
═══════════════════════════════════════════════════════════════════════
Root: Raise spice whole from genesis — acceptance amended to one
authority [J18]; canonical `Project:` and `Origin:` fields so every
preview of the package normalizes to one digest [J28]; covenant
paragraphs [POP]; evidence annotation keeps the scars; authority is
the journal's word alone — external surfaces are evidence boundaries.

W0  Seal the doctrine (records, not questions) [J70]:
    pin workspace · confine rim · day-zero hooks · DOCTRINE (incl. the
    five records) · LATER · TASTES leaf [J58] · NAMES carve · config
    tree · trust-quarantine design record [J21] · unit templates
    [J51].
W1  Carve the contracts:
    wire v1 (+hello capability/epoch) · command transaction + witnesses
    [J24] · fact vocabulary (+J27 kinds, chain-root fixed) · fold
    contract (+fold/head sources) · Packed [J63] · provenance grammar ·
    dialect spec · vendor record contract · migration contract [J37] ·
    generated closure gates seeded [J74].
W2  Lay the journal kernel:
    sill: Spec + spawn + flock [J68] · atomic [J69] · resource locks +
    lock verbs [J39] · guard+capabilities [J29] · head tables incl.
    task statements [J18] · lock+fork test [J38] · epoch · seq/notify ·
    bus+timers · timer wheel re-derived at epoch mint [J49] ·
    migration command + drill · crash+restore drills (incl. the
    kill-between-defer-and-due drill) · backup/restore protocol [J35] ·
    claim race proof.
W3  The vertical slice [J73]: fact → append → head → fold →
    measure-only gate → static binary; resolves a wave-0..2 defect as
    its payload.
W4  Fold the board: tasks-as-heads · claim::Active · witnesses ·
    allocator contract then next [J42] · urgency · oops [J43] ·
    depends [J39] · defer/wake · board [FAMILY] [J41] · board graphs +
    handout [J41].
W5  Teach the dialect: classifier · ledger fixed-point · apply-as-one-
    transaction with digest [J28] · refusal corpus.
W6  String the steering: directives · inbox fold · ack scan + command
    evidence + operator cancel [J6] · resend + exhaustion alarm ·
    activation (after an admitted lane [J71]) · run (measured,
    hot-path negative contract) [J81] · degrade-once per lane.
W7  Raise the resident: quarantine boot [J21] · trust verbs [J22] ·
    consent + budgets · up assembly · down state machine [J44: seven
    durable states] · recall product [J48] · renewal [J49] ·
    compaction wiring [J50] · lane table + lives (Steer/Drive/Drain) ·
    lane hold/resume [J27] · occupants (desired vs actual launch
    identity) · tape tee/seal (0600 [J36]) · listen dialects + corpus ·
    canary (absence instrument) · vendor::armed · watch/retro/doctor
    rungs [J39] · version verb [J46].
W8  Wake the tastes: bags (evidence only) · judge · facts ·
    tracked-proposal curation — membership by operator commit alone.
W9  Raise the gate frame: walk (named carve-outs) · measure ·
    shrink-only debt floors [J54] · camp (pile-up audit) · deflaked
    kill via survivor rate [J55] · receipts pinned [J57] ·
    gate::found/cleared facts [J56] · deny from the NAMES Retired
    table [J78] · contract table with battery owners [J39] ·
    Spice-Origin trailer gate at the Git boundary [J79] · commit gate ·
    test placement + test-only-pub check [J59] · corpus + lattice
    conformance gates [J74] · enforcement flip leaf [J60].
W10 Render the canvas: Leptos record leaf (settled, no trial) ·
    socket+state · panes mapped onto the four axes [J61] · magnitude/
    flow/topology/chronology derivations · thread walk · capability
    surfaces + web security [J64] · no-secret-rendered fixtures ·
    packed everywhere · fixtures-only tests incl. rendering smoke.
W11 Prove the appliance: dist payload [J62] · supply contract [J67] ·
    sealed replay per target (container + native runner) · digest-and-
    serve app verification [J65] · bootstrap trust root for the first
    release · post-proof seal composes the one manifest [J66] ·
    publish refusal [--apply=<digest>] · notes from the sealed
    manifest.
W12 Cut release one: skill regenerated [J47] · closure gates green
    [J74] · foreign-repo operation · cutover (external launcher,
    drain, export, backup, empty authority, explicit rollback
    boundary) · restate STABILITY [POP].

Per-leaf skeleton everywhere: Title (imperative, no retired words
[J75]) · After (explicit) · 1–3 behavioral acceptance lines · Flow ·
Priority · Tags · [POP: 2–4 sentence description].
Ordering repairs [J71] and law-dependents [J72] are edges in the lists
above. Any portion offered to the classifier previews clean before it
is applied; the whole is worked incrementally, never bulk-loaded.

═══════════════════════════════════════════════════════════════════════
## README.md (rewrite)
═══════════════════════════════════════════════════════════════════════
Reading order (12 files incl. FRAME, SCAFFOLD, DOCTRINE, TASTES,
LATER) · the architecture in one paragraph (ONE authority) [J18] ·
using the program (one agent starts; leaves are minted into tasks as
they become current; the corpus amends as construction teaches) ·
standing doctrine [POP: tighten to ten lines].

═══════════════════════════════════════════════════════════════════════
## POPULATION ORDER
═══════════════════════════════════════════════════════════════════════
1. DOCTRINE (the records legitimize everything else).
2. NAMES amendments (the words the rest will use).
3. WIRE → TREE → DISK → CLI (contracts before surfaces).
4. CONFIG, CANVAS amendments; TASTES; LATER.
5. GENESIS from the skeleton; retired words out of titles;
   classifier to zero/zero.
6. README last. Archive the gauntlet under docs/record/ [J58].
Every slot is a [POP]; nothing requires a decision. The frame holds.
