# Executable succession ledger

This ledger records the Python-to-Rust command succession observed on
2026-08-01. It is an execution ledger, not cutover authorization. Python
remains the scaffold authority until the operator explicitly changes that
fact.

## Current authority

- The ambient command resolves from `command -v spice` to the user-local bin,
  version `0.31.0`.
- That script imports `spice.cli.entry` through the Python environment at
  `~/.local/share/uv/tools/spice-harness`.
- Its editable source resolves to the sibling checkout
  `../spice-z/spice/cli/entry.py`.
- `core.hooksPath` is `.spice/hooks`. Its `pre-commit`, `commit-msg`, and
  `reference-transaction` entries still invoke `spice dev ...`.
- Rust is invoked explicitly with `cargo run --locked -q -p spice -- ...`.
- The tracked `spice.toml` and Git-common `.spice/config.toml` are admitted
  configuration inputs for Rust Doctor. Doctor inspects them without opening
  authority or launching declared tools.
- Rust authority and runtime files live only under the Git-common
  `.spice-rust` directory. The installed Python command continues to own its
  Git-common `.spice` authority.
- `HOOKS-1kJQnmG8` owns the eventual native hook transition. No active hook or
  ambient command path changes before explicit operator cutover.
- Cutover starts a clean Rust authority as required by DOCTRINE.md. It does not
  open, import, migrate, or mutate Python authority bytes.

## Rust coexistence bootstrap

The sole repository-local Rust invocation during construction is Cargo with
the workspace package selected explicitly. From any directory in the target
worktree, run:

```console
cargo run --locked -q -p spice -- doctor
cargo run --locked -q -p spice -- up
cargo run --locked -q -p spice -- version
cargo run --locked -q -p spice -- down
```

`up` remains in the foreground until `down` completes, so the last two commands
run from another terminal in the same repository. `up` creates or admits the
Rust journal at `<git-common>/.spice-rust/data/spicejournal.sqlite3`; linked
worktrees resolve the same Git-common root. `version` reads that finalized Rust
journal, and `down` reads the discovery published at
`<git-common>/.spice-rust/canvas`.

The same root owns resident locks, lane tape, degraded-state evidence, command
locks, and proof receipts. Rust startup does not inspect Python databases below
`<git-common>/.spice`. An unfinalized or incompatible `.spice-rust` journal
returns the typed `journal::schema-refused` outcome naming its state. The
ambient `spice` command and `.spice/hooks` remain Python-owned until the
operator authorizes cutover.

## Rust contracts

The registry contains 63 command leaves. Nineteen have executable dispatch,
so current executable coverage is 19/63, or 30.2 percent. Completed domain
libraries do not count as landed command surfaces.

### Landed

- `watch`
- `retro`
- `doctor`
- `version`
- `gate`
- `gate camp`
- `gate kill`
- `gate receipt`
- `lock`
- `lock run`
- `proof`
- `proof replay`
- `release notes`
- `release publish`
- `up`
- `down`
- `run`
- `config`
- `restore verify`

`doctor`, `version`, and `up` are executable but not yet usable against the
current worktree state; the configuration and bootstrap tasks above own those
operational gaps.

### `LOCAL-1kJQlG6b`

- `install`

### `BOARD-1kJQlDDt`

- `board`
- `board graph`
- `board handout`
- `task next`
- `task add`
- `task done`
- `task review`
- `task note`
- `task defer`
- `task wake`
- `task free`
- `task oops`
- `task depends`
- `doc`
- `doc ledger`

### `COORDIN-1kJQlDjc`

- `lane`
- `lane add`
- `lane drain`
- `lane wake`
- `steer`
- `steer cancel`
- `inbox`
- `recall`
- `ack`

### `POLICY-1kJQlFB6`

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

### `JOURNAL-1kJQlFfS`

- `backup`
- `restore`
- `journal`
- `journal replay`
- `journal reindex`
- `journal verify`
- `fold`
- `thread`

### `PUBLISH-1kJQlGb0`

- `canvas`

`spice/src/cli.rs::landing` carries the same classification. Its closure test
fails for any registered Rust leaf without a disposition. A leaf classified
as landed that reaches the terminal release branch faults as a dispatch defect
instead of presenting unfinished work.

## Installed Python paths

The installed parser reported 123 paths. Namespace rows are included because
the registry exposes them even though argparse requires a child action.

### Agent lifecycle

- `agent`: namespace only; Rust uses resident and lane surfaces.
- `agent activation`: `lane` and canvas discovery; `COORDIN-1kJQlDjc`.
- `agent ensure`: `lane add`; `COORDIN-1kJQlDjc`.
- `agent import`: removed by the LATER.md drop ledger.
- `agent post-tool-hook`: `run`, `inbox`, and `ack`; `COORDIN-1kJQlDjc`.
- `agent reply`: `ack`; `COORDIN-1kJQlDjc`.
- `agent requeue-deadletter`: removed by the LATER.md drop ledger.
- `agent run`: landed `run`.
- `agent show`: `lane` and `recall`; `COORDIN-1kJQlDjc`.
- `agent supervise`: landed `up` plus `lane`; `COORDIN-1kJQlDjc`.

### Configuration

- `config`: namespace in Python; landed Rust leaf `config`.
- `config agent`: `config` plus vendor launch; `LOCAL-1kJQlG6b`.
- `config defaults`: landed typed `config` provenance.
- `config judge`: tracked `taste` policy; `POLICY-1kJQlFB6`.
- `config personality`: typed `config` and vendor launch; `LOCAL-1kJQlG6b`.
- `config say`: typed canvas speech; `SPEECH-1kJQnmkd`.
- `config set`: landed `config` refuses writes until a digest-bound mutation
  contract is admitted.
- `config show`: landed Rust `config`.
- `config system`: landed typed `config` provenance.
- `config trust`: namespace; Rust `trust`; `POLICY-1kJQlFB6`.
- `config trust grant`: `trust grant`; `POLICY-1kJQlFB6`.
- `config trust revoke`: `trust revoke`; `POLICY-1kJQlFB6`.
- `config trust show`: `trust`; `POLICY-1kJQlFB6`.

### Installation, locks, and health

- `deinit`: withdrawn in Python; Rust `install --unapply` and `down`;
  `LOCAL-1kJQlG6b`.
- `doctor`: landed Rust `doctor`; current config work is `INTEGRA-1kJQlCHl`.
- `init`: Rust `install` and `up`; `LOCAL-1kJQlG6b` and
  `COEXIST-1kJQlCm5`.
- `lock`: landed Rust `lock`.
- `lock run`: landed Rust `lock run`.
- `lock status`: landed Rust `lock`.
- `watch`: landed Rust `watch`.

`demo` is removed by the LATER.md drop ledger.

### Development and repository gates

- `dev`: namespace only; Rust uses `gate`, `proof`, and `cargo xtask verify`.
- `dev commit-msg`: native hook work; `HOOKS-1kJQnmG8`.
- `dev doctor`: landed Rust `doctor` plus `INTEGRA-1kJQlCHl`.
- `dev install-hooks`: Rust `install` plus `HOOKS-1kJQnmG8`.
- `dev pre-commit`: `gate` and canonical verification; `VERIFY-1kJQlH5b`.
- `dev pytest`: workspace tests in canonical verification; `VERIFY-1kJQlH5b`.
- `dev python-typecheck`: ends with the Python implementation; Rust Clippy and
  tests are owned by `VERIFY-1kJQlH5b`.
- `dev reference-transaction`: its Git publication behavior is removed by the
  LATER.md drop ledger; hook withdrawal is `HOOKS-1kJQnmG8`.
- `dev serve-web-typecheck`: Wasm checks in canonical verification;
  `VERIFY-1kJQlH5b`.
- `dev serve-web-types`: shared Rust wire types replace emitted JavaScript;
  `VERIFY-1kJQlH5b`.

Every path below is development evidence for `gate` or canonical verification,
not a shipped one-for-one product verb. `VERIFY-1kJQlH5b` owns policy parity.

- `study`
- `study assertion-free-tests`
- `study complexity`
- `study complexity-hotspots`
- `study csharp-members`
- `study csharp-unused-candidates`
- `study env-name-ledger`
- `study env-policy`
- `study file-loc`
- `study javascript-unused`
- `study magic-numbers`
- `study markdown-links`
- `study mutations`
- `study private-internals`
- `study python-unused`
- `study reachability`
- `study shape`
- `study subsumption`
- `study suite-seam-reach`
- `study symbol-reachability`
- `study taste`

### Taste curation

- `maxim`: namespace; Rust uses `taste`; `POLICY-1kJQlFB6`.
- `maxim agree`: taste judgment and canvas curation; `POLICY-1kJQlFB6`.
- `maxim disable`: tracked taste deletion proposal; `POLICY-1kJQlFB6`.
- `maxim disabled`: `taste` membership view; `POLICY-1kJQlFB6`.
- `maxim disagree`: taste judgment and canvas curation; `POLICY-1kJQlFB6`.
- `maxim enable`: tracked taste admission proposal; `POLICY-1kJQlFB6`.
- `maxim file-proposals`: `taste add` plus `task add`; policy and board tasks.
- `maxim proposals`: `taste add`; `POLICY-1kJQlFB6`.
- `maxim propose`: `taste add` plus `task add`; policy and board tasks.
- `maxim report`: `taste` and canvas curation; policy and publish tasks.
- `maxim show`: `taste`; `POLICY-1kJQlFB6`.
- `maxim sources`: taste loci in `taste` and canvas; policy and publish tasks.

### Operator canvas

- `serve`: Rust `canvas`; `PUBLISH-1kJQlGb0`.
- `serve browser-artifact-path`: embedded canvas discovery;
  `PUBLISH-1kJQlGb0`.
- `serve rebuild-projections`: `journal replay` and `fold`;
  `JOURNAL-1kJQlFfS`.
- `serve teams`: `lane` and `board`; coordination and board tasks.

### Session forensics

The complete Python `session` family is removed by the LATER.md drop ledger.
Its retained questions are answered by landed `watch` and `retro`, plus
`recall` and `thread` under their owning tasks.

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

### Task plane

- `task`: namespace; Rust uses `board`, `task`, and `doc`;
  `BOARD-1kJQlDDt`.
- `task add`: Rust `task add`; `BOARD-1kJQlDDt`.
- `task artifact`: task evidence successor; `BOARD-1kJQlDDt`.
- `task artifact add`: task evidence successor; `BOARD-1kJQlDDt`.
- `task artifact list`: task evidence successor; `BOARD-1kJQlDDt`.
- `task artifact prune`: no second authority or retention store;
  `BOARD-1kJQlDDt` must prove the path or record its release boundary.
- `task artifact show`: task evidence successor; `BOARD-1kJQlDDt`.
- `task capture`: automatic `commit::landed` recognition plus `task add`;
  `BOARD-1kJQlDDt`.
- `task claim`: allocator-owned `task next`; direct row choice does not cross.
- `task delete`: `doc` task editing; settled correction remains trigger-bound
  in LATER.md; `BOARD-1kJQlDDt`.
- `task depends`: Rust `task depends`; `BOARD-1kJQlDDt`.
- `task doctor`: Rust `doctor` and `board`; `BOARD-1kJQlDDt`.
- `task done`: Rust `task done`; `BOARD-1kJQlDDt`.
- `task graph`: Rust `board graph`; `BOARD-1kJQlDDt`.
- `task handout`: Rust `board handout`; `BOARD-1kJQlDDt`.
- `task ingest`: Rust `doc`; `BOARD-1kJQlDDt`.
- `task ledger`: Rust `doc ledger`; `BOARD-1kJQlDDt`.
- `task list`: Rust `board`; `BOARD-1kJQlDDt`.
- `task modify`: Rust `doc` and task-edit intent; `BOARD-1kJQlDDt`.
- `task next`: Rust `task next`; `BOARD-1kJQlDDt`.
- `task note`: Rust `task note`; `BOARD-1kJQlDDt`.
- `task oops`: Rust `task oops`; `BOARD-1kJQlDDt`.
- `task reclaim`: resident claim renewal has no manual command.
- `task review`: Rust `task review`; `BOARD-1kJQlDDt`.
- `task reword`: active statement correction uses `doc`; settled correction is
  trigger-bound in LATER.md; `BOARD-1kJQlDDt`.
- `task show`: Rust `board`; `BOARD-1kJQlDDt`.
- `task sizing`: Rust `board` and `board graph`; `BOARD-1kJQlDDt`.
- `task stale`: Rust `board` and `lane`; board and coordination tasks.
- `task status`: Rust `board`; `BOARD-1kJQlDDt`.
- `task unclaim`: Rust `task free`; `BOARD-1kJQlDDt`.
- `task wake`: Rust `task wake`; `BOARD-1kJQlDDt`.

Mounted repository commands are absent from the installed 123-path registry.
They remain behind the evidence trigger in LATER.md and are not release-one
work.

## Reproduce the audit

Capture the installed Python registry without importing it into Rust:

```sh
~/.local/share/uv/tools/spice-harness/bin/python -c \
  'from spice.cli.parser import command_path_registry as r; \
print("\\n".join(" ".join(p) for p in sorted(r())))'
```

Inspect Rust registration and executable classification with:

```sh
cargo test --locked -p spice \
  every_contract_has_an_executable_succession_disposition
cargo run --locked -q -p spice -- --help
```

`VERIFY-1kJQlH5b` owns the final machine-readable coverage report and the
95-percent pre-cutover threshold. Passing that threshold still does not grant
cutover authority.
