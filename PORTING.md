# Executable succession ledger

Captured on 2026-08-01. This is an execution ledger, not cutover authority.
Python remains the installed scaffold until the operator explicitly changes it.

## Authority and readiness

The ambient `~/.local/bin/spice` is Python `0.31.0`; Rust runs explicitly via
Cargo. Active `.spice/hooks` and legacy tracked `.githooks` wrappers still
invoke Python. Python owns Git-common `.spice`; Rust owns `.spice-rust` and
never reads Python authority bytes.

| Measure | Result |
| --- | --- |
| Installed Python registry | 123/123 paths have one disposition; 4 task-owned gaps; no unsupported deletions |
| Rust contract registry | 82/82 leaves landed; no gaps or unaccounted leaves |
| Rust process sweep | 164 text/JSON invocations; matching exits and streams; no placeholders |
| Functional smoke | 48/48 process tests across lifecycle, control, evidence, and recovery |
| Coexistence | Two Rust generations; Python command, state, hooks, and hook path unchanged |

The smoke set is `succession`, `coexistence`, `resident`, `steering`,
`gate_cli`, `proof_cli`, `journal_cli`, `session_cli`, and `task_artifact_cli`.
It covers restart, control, evidence, session forensics, and recovery. Python
activation, agent/task status, and the live claim were exercised in the same
worktree with separate authority.

Cutover remains blocked on three classes of risk:

1. `RECOVER-1kJWSHkc`, `DEMO-1kJWSLwL`, `PARITY-1kJWSQ7W`, and
   `PARITY-1kJWSV53` own retained predecessor behavior
   that is not yet executable in Rust. Until they land, the active Python
   hooks and command remain unchanged and the Python policy gate runs last in
   `cargo xtask verify`.
2. Predecessor state transfer and recovery must satisfy the parity tasks; an
   empty Rust authority is not a substitute for retained capability.
3. Installed entrypoint selection and rollback still require an operator-run
   cutover rehearsal and explicit authorization.

The version-three Rust install plan publishes executable `pre-commit`,
`commit-msg`, and `reference-transaction` artifacts beneath
`.spice/rust/git-hooks`. The first invokes only the canonical verifier; the
second preserves current commit-message hygiene and additionally enforces the
frozen `Spice-Origin` v1 grammar; the third preserves the independent guard
against abandoning commits already merged into the current branch's upstream.
Git remains on its Python-owned `.spice/hooks` or `.githooks` path until the
operator explicitly runs `scripts/install-hooks.sh`, which activates the
complete Rust namespace.

## Rust coexistence

From the target worktree, run `doctor`, then foreground `up`. Run `version` and
`down` from another terminal. `up` creates or admits
`<git-common>/.spice-rust/data/spicejournal.sqlite3` and publishes owner-only
discovery at `<git-common>/.spice-rust/canvas`. State links, malformed state,
and incompatible journals refuse by name. No command changes the ambient
entrypoint, active hooks, or Python authority.

## Rust contracts

The closed registry contains 82 leaves and all 82 have executable dispatch.
`spice/src/cli.rs::landing` carries the same classification; its tests reject
registry drift, and the process sweep rejects terminal release or resident
placeholder outcomes.

### Landed

- `watch`
- `retro`
- `session`
- `session briefing`
- `session commands`
- `session commits`
- `session compactions`
- `session messages`
- `session phases`
- `session slices`
- `session summary`
- `session sweep`
- `session thread`
- `session timeline`
- `session tokens`
- `session turns`
- `doctor`
- `version`
- `install`
- `lock`
- `lock run`
- `restore verify`
- `gate`
- `gate camp`
- `gate kill`
- `gate receipt`
- `proof`
- `proof replay`
- `release notes`
- `up`
- `down`
- `board`
- `board graph`
- `board handout`
- `task next`
- `task add`
- `task capture`
- `task done`
- `task review`
- `task note`
- `task defer`
- `task wake`
- `task free`
- `task oops`
- `task depends`
- `task artifact add`
- `task artifact list`
- `task artifact prune`
- `task artifact show`
- `doc`
- `doc ledger`
- `lane`
- `lane add`
- `lane drain`
- `lane wake`
- `steer`
- `steer cancel`
- `inbox`
- `recall`
- `consent`
- `consent allow`
- `consent deny`
- `consent policy`
- `taste`
- `taste add`
- `taste drop`
- `trust`
- `trust grant`
- `trust revoke`
- `backup`
- `restore`
- `journal`
- `journal replay`
- `journal reindex`
- `journal verify`
- `fold`
- `thread`
- `canvas`
- `release publish`
- `run`
- `ack`
- `config`

`install`, `config`, `restore`, and `release publish` are digest-bound plans.
Rust installation remains namespaced and never replaces the ambient command or
active hooks. Recovery is offline and lock-held; resident reads are
authenticated. `backup` and digest-pinned `restore` carry the complete
manifest-referenced task-artifact generation with the journal authority and
roll both planes back together on failed finalization. `doctor`, `version`,
`up`, and `down` execute end to end against isolated Rust state in both forms.

## Installed Python paths

The parser has 123 paths. `R` means a landed behavioral replacement, `D` a
landed regrouping, `P` a retained capability owned by the named public task,
and `X` an operator-confirmed deletion. `X` is valid only when the exact path
and acknowledgment evidence appear in the structured deletion registry in
LATER.md. Under `ack:1kJWMcnL`, cutover authorization does not authorize
capability deletion, so that registry is empty. A pending owner is a cutover
risk, not a fallback.

### Agent lifecycle

- `agent`: D -> resident and lane surfaces.
- `agent activation`: R -> `lane` and canvas discovery.
- `agent ensure`: R -> `lane add`.
- `agent import`: P -> `RECOVER-1kJWSHkc` for team, lineage, session, and claim transfer.
- `agent post-tool-hook`: R -> `run`, `inbox`, and `ack`.
- `agent reply`: R -> `ack`.
- `agent requeue-deadletter`: P -> `RECOVER-1kJWSHkc` for exact-once directive recovery.
- `agent run`: R -> `run`.
- `agent show`: R -> `lane` and `recall`.
- `agent supervise`: R -> `up` and `lane`.

### Configuration

- `config`: D -> Rust `config` is a leaf, not a namespace.
- `config agent`: D -> `config` plus vendor launch.
- `config defaults`: R -> `config` provenance.
- `config judge`: R -> `taste`.
- `config personality`: D -> `config` plus vendor launch.
- `config say`: D -> typed `[say]` configuration and canvas speech playback.
- `config set`: R -> digest-bound `config` apply.
- `config show`: R -> `config`.
- `config system`: R -> `config` provenance.
- `config trust`: D -> Rust `trust` family.
- `config trust grant`: R -> `trust grant`.
- `config trust revoke`: R -> `trust revoke`.
- `config trust show`: R -> `trust`.

### Installation, locks, and health

- `deinit`: D -> `install --unapply` plus `down`.
- `demo`: P -> `DEMO-1kJWSLwL` for the zero-setup seeded canvas environment.
- `doctor`: R -> `doctor`.
- `init`: D -> `install` plus `up`.
- `lock`: D -> Rust `lock` family.
- `lock run`: R -> `lock run`.
- `lock status`: R -> `lock`.
- `watch`: R -> `watch`.

### Development gates

- `dev`: D -> `gate`, `proof`, and `cargo xtask verify`.
- `dev commit-msg`: D -> installer-owned `cargo xtask commit-msg` hook.
- `dev doctor`: R -> `doctor`.
- `dev install-hooks`: D -> `install` plus operator-run `scripts/install-hooks.sh` cutover.
- `dev pre-commit`: D -> `gate` plus the canonical verifier.
- `dev pytest`: D -> workspace tests in the canonical verifier.
- `dev python-typecheck`: R -> strict Rust linting and workspace tests in the canonical verifier.
- `dev reference-transaction`: D -> installer-owned `cargo xtask reference-transaction` hook.
- `dev serve-web-typecheck`: D -> verifier Wasm check.
- `dev serve-web-types`: R -> shared Rust wire types plus verifier app-drift and Wasm checks.

Each `study` path is D -> gate or verifier evidence, not a shipped verb.

- `study`: D.
- `study assertion-free-tests`: D.
- `study complexity`: D.
- `study complexity-hotspots`: D.
- `study csharp-members`: D.
- `study csharp-unused-candidates`: D.
- `study env-name-ledger`: D.
- `study env-policy`: D.
- `study file-loc`: D.
- `study javascript-unused`: D.
- `study magic-numbers`: D.
- `study markdown-links`: D.
- `study mutations`: D.
- `study private-internals`: D.
- `study python-unused`: D.
- `study reachability`: D.
- `study shape`: D.
- `study subsumption`: D.
- `study suite-seam-reach`: D.
- `study symbol-reachability`: D.
- `study taste`: D.

### Taste curation

- `maxim`: D -> Rust `taste` family.
- `maxim agree`: D -> `taste` judgment and canvas curation.
- `maxim disable`: D -> tracked `taste drop` proposal.
- `maxim disabled`: R -> `taste` membership.
- `maxim disagree`: D -> `taste` judgment and canvas curation.
- `maxim enable`: D -> tracked `taste add` proposal.
- `maxim file-proposals`: D -> `taste add` plus `task add`.
- `maxim proposals`: R -> `taste add`.
- `maxim propose`: D -> `taste add` plus `task add`.
- `maxim report`: D -> `taste` plus canvas curation.
- `maxim show`: R -> `taste`.
- `maxim sources`: D -> `taste` provenance plus canvas.

### Operator canvas

- `serve`: R -> `canvas`.
- `serve browser-artifact-path`: D -> embedded canvas artifact.
- `serve rebuild-projections`: R -> `journal replay` and `fold`.
- `serve teams`: R -> `lane` and `board`.

### Session forensics

The Rust rim now reads immutable Codex and Claude transcript evidence directly,
including gzip, ambient/thread discovery, compaction reconstruction, usage and
tool accounting, deterministic residue, and bounded filtered projections.

- `session`: R -> `rim::session_briefing`.
- `session briefing`: R -> `rim::session_briefing`.
- `session commands`: R -> `rim::session_commands`.
- `session commits`: R -> `rim::session_commits`.
- `session compactions`: R -> `rim::session_compactions`.
- `session messages`: R -> `rim::session_messages`.
- `session phases`: R -> `rim::session_phases`.
- `session slices`: R -> `rim::session_slices`.
- `session summary`: R -> `rim::session_summary`.
- `session sweep`: R -> `rim::session_sweep`.
- `session thread`: R -> `rim::session_thread`.
- `session timeline`: R -> `rim::session_timeline`.
- `session tokens`: R -> `rim::session_tokens`.
- `session turns`: R -> `rim::session_turns`.

### Task plane

- `task`: D -> Rust board/task/doc families.
- `task add`: R -> `task add`.
- `task artifact`: D -> Rust `task artifact` command family.
- `task artifact add`: R -> `task artifact add`.
- `task artifact list`: R -> `task artifact list`.
- `task artifact prune`: R -> `task artifact prune`.
- `task artifact show`: R -> `task artifact show`.
- `task capture`: R -> `task capture` with loose-commit capture and boundary publication.
- `task claim`: R -> allocator-owned `task next`.
- `task delete`: D -> authored `doc` correction.
- `task depends`: R -> `task depends`.
- `task doctor`: D -> `doctor` plus `board`.
- `task done`: R -> `task done`.
- `task graph`: R -> `board graph`.
- `task handout`: R -> `board handout`.
- `task ingest`: R -> `doc`.
- `task ledger`: R -> `doc ledger`.
- `task list`: R -> `board`.
- `task modify`: D -> `doc`.
- `task next`: R -> `task next`.
- `task note`: R -> `task note`.
- `task oops`: R -> `task oops`.
- `task reclaim`: P -> `RECOVER-1kJWSHkc` for bounded manual claim recovery.
- `task review`: R -> `task review`.
- `task reword`: D -> active correction uses `doc`.
- `task show`: R -> `board`.
- `task sizing`: R -> `board graph`.
- `task stale`: D -> `board` plus `lane`.
- `task status`: R -> `board`.
- `task unclaim`: R -> `task free`.
- `task wake`: R -> `task wake`.

Mounted repository commands are absent from the installed registry and remain
behind their LATER.md evidence trigger.

## Reproduce the audit

```sh
~/.local/share/uv/tools/spice-harness/bin/python -c \
  'from spice.cli.parser import command_path_registry as r; \
print("\\n".join(" ".join(p) for p in sorted(r())))'
cargo test --locked -p spice \
  installed_python_registry_and_ledger_dispositions_are_bijective
cargo test --locked -p spice \
  every_contract_has_an_executable_succession_disposition
cargo test --locked -p spice --test succession
cargo test --locked -p xtask \
  live_python_deletion_requires_exact_operator_confirmation
```

`cargo xtask verify` emits the versioned succession-readiness report, enforces
the 95 percent landed threshold and both registry closures, rejects malformed
pending owners and unsupported or stale deletion claims, then runs the
installed Python `spice dev pre-commit` policy gate. Passing proves an
accounted ledger; reported pending paths still block parity and it does not
grant cutover authority.
