# Coexistence record

Python Spice and Rust Spice may run in the same repository, but they do not
share writable routing state. They share only the tracked repository and the
operator's per-worktree choice of Git hook implementation. Routing identity,
pending direction, acknowledgments, resident discovery, sockets, configuration,
artifacts, receipts, and Git shadow files have distinct physical names.

`spice/tests/coexistence.rs` snapshots Python's complete state tree around real
Rust commands. The concurrent process proof is
`spice/tests/resident/vendor_process/coexistence.rs#installed_spice_lifecycle_requires_explicit_predecessor_hook_handoff`.
This record maps the policy those tests enforce; `spice/tests/coexistence_record.rs`
keeps every local citation resolvable.

## Reading a citation

A citation is `` `path#anchor` ``: a repository-relative path followed by text
that must still occur in that file. Paths beginning `spice-z/` name the separate
Python checkout and are intentionally not resolved by this repository's test.
The Python measurements below were last checked at revision
`7380d9a09b86637ef923f3ee0c5360b8413b8319`.

## Direct-call boundary

Neither implementation directly invokes the other. Python's tool subprocess at
`spice-z/spice/tasks/ops.py#run_tool_command` launches the configured `rtk`
from `spice-z/spice/config/values.py#configured_rtk_executable`, not this port.
Rust's `spice/src/cli.rs#Command::new("spice")` is a clap command name, not a
process construction. The executable-name convergence still mattered through
an inherited shell `PATH`; the routing root cause and closure are recorded
below.

## Physical routing ledger

| Plane | Python Spice | Rust Spice |
| --- | --- | --- |
| Git-common authority root | `<common>/.spice` | `<common>/.spice-rs` |
| Lane-local context root | `<git-dir>/.spice` and `<worktree>/.spice` | Per-lane facts below `<common>/.spice-rs` |
| Tracked repository config | `<worktree>/spice.toml` | `<worktree>/spice-rust.toml` |
| Machine-local config | `<git-dir>/.spice/config/spice.toml` | `<common>/.spice-rs/config.toml` |
| Task and message authority | `<common>/.spice/data/*.sqlite3` | `<common>/.spice-rs/data/spicejournal.sqlite3` |
| Pending direction | `<worktree>/.spice/inbox/*.txt` | Rust journal facts and `spice/src/repository.rs#pub fn inbox` |
| ACK authority | `<common>/.spice/data/spiceacks.sqlite3` | Rust journal facts under `.spice-rs` |
| Resident discovery | Python state and process-unique side channel | `<common>/.spice-rs/canvas` and `<common>/.spice-rs/resident` |
| Git shadow | `<git-dir>/agent.gitconfig` | `<git-dir>/spice-rs-agent.gitconfig` |
| Install receipt | Python-owned namespace, if any | `<common>/.spice-rs/install.json` |
| Published Rust appliance | none | `<common>/.spice-install` |

The Rust root is declared once at
`spice/src/repository.rs#pub const STATE_DIRECTORY`; its whole subtree is the
single exclusive directory returned by
`spice/src/repository.rs#pub fn exclusive_directories`. Gate capture uses the
same root at `gate/src/state.rs#CURRENT_STATE_DIRECTORY`, and local configuration
uses it at `spice/src/config/load.rs#const LOCAL_FILE`. No per-file subtraction
from Python's root remains.

### State stores

Python declares its shared root at `spice-z/spice/paths.py#shared_state_root`,
its durable lane-local root at `spice-z/spice/paths.py#worktree_state_root`, and
its visible live root at
`spice-z/spice/paths.py#worktree_runtime_state_root`.
Its authorities include:

- `spice-z/spice/tasks/opslog.py#OPERATIONS_DB_FILENAME` for tasks;
- `spice-z/spice/mail/ackstate.py#ACK_STATE_DATABASE_FILENAME` for ACKs;
- `spice-z/spice/agent/maximmetrics.py#MAXIM_METRICS_DATABASE_FILENAME` for
  maxim metrics;
- `spice-z/spice/serve/team/projection.py#PROJECTION_DATABASE_FILENAME` for
  projections; and
- `spice-z/spice/serve/team/schema.py#TEAM_DATABASE_FILENAME` for teams.

Rust declares its journal through
`spice/src/repository.rs#spicejournal.sqlite3` and its fold projection through
`spice/src/repository.rs#spicefolds.sqlite3`. A coincident leaf filename in the
future cannot collide because the parent roots differ. The test
`spice/tests/coexistence.rs#state_roots_are_siblings_and_database_names_cannot_cross`
places the same relative database name in both roots and reads both identities
back independently.

### Agent identity, inbox, ACK, and live delivery

Python resolves a worktree-local thread pointer at
`spice-z/spice/agent/paths.py#THREAD_ID_FILENAME`, pending files through
`spice-z/spice/paths.py#worktree_inbox_dir`, ACKs through
`spice-z/spice/mail/ackstate.py#ACK_STATE_DATABASE_FILENAME`, and its live side
channel through `spice-z/spice/agent/sidechannel.py#spice-agent-side-`.

Rust binds a lane capability with `spice/src/agent_launch.rs#SPICE_RS_LANE`, stores
directions and ACK facts in its own journal, and publishes resident discovery
below the root exposed by `spice/src/repository.rs#pub fn runtime`. A Python and
Rust agent may carry the exact same UUID: UUID equality is data equality, not a
filesystem or queue identity.

The concurrent proof seeds Python task authority and a Python
`.spice/inbox/<uuid>.txt`, starts a Rust lane with that same UUID, and routes a
Rust directive to it. Rust's inbox never lists the Python item; Rust refuses an
ACK for the Python key; Python's inbox, ACK database, thread pointer, socket
marker, executable, hooks, and process remain unchanged. Two other live Rust
lanes also prove that each lane sees and ACKs only its own directions.

### Git shadow

Python owns `<git-dir>/agent.gitconfig` through
`spice-z/spice/agent/shadow.py#AGENT_GITCONFIG_NAME`. Rust owns
`<git-dir>/spice-rs-agent.gitconfig` through
`spice/src/git_shadow.rs#AGENT_GITCONFIG_NAME` and
`spice/src/git_shadow.rs#publish_shadow`. The regression
`spice/tests/git_shadow.rs#RUST_GITCONFIG_NAME` asserts both the positive Rust
path and the untouched Python path in ordinary and linked worktrees.

### Installation and cutover from the former shared root

Rust publishes executable, skill, and hooks under `.spice-install`, while its
receipt is below `.spice-rs`. `spice/src/install.rs#const RECORD_PATH` and
`spice/src/install.rs#const GIT_PRE_COMMIT_PATH` pin those common-Git
namespaces. Rust therefore owns no worktree exclusion stanza and never reads or
writes `info/exclude`. A same-version Rust receipt left at the former
`.spice/install.json` location is not an alternate input: the new binary never
inspects a path now owned by Python. An operator must reverse that old
installation with its owning old installed binary before applying the renamed
build. This is an explicit one-time namespace cutover, not an implicit bridge
or fallback.

## The executable routing defect and closure

The operator attachment for ROUTING-1kLGWDWl showed one agent reporting its
context unavailable while Python and Rust Spice were running together. The
attachment localized the investigation; the causal trace came from code and the
concurrent reproduction:

1. Python's console entry at `spice-z/spice/cli/entry.py#def main` and Rust's
   installed skill at `spice/src/install/skill.md#spice recall` both answer to
   the shell word `spice`.
2. Rust already embedded its exact executable in hook and MCP launch arguments,
   but the resident passed ambient `PATH` to the occupant. If Python's executable
   appeared first, prose-driven Rust skill commands reached Python.
3. Python correctly consulted its own thread, inbox, and ACK authorities. To the
   Rust lane, correct service by the wrong runtime appeared as missing context.

The terminal is a **root cause**: inherited executable lookup let a Rust-launched
agent change routing identity after launch. The closure is
`spice/src/resident/lane_surface/launch.rs#fn appliance_path`, which prepends the
directory of the running Rust executable to the occupant's `PATH`. The
concurrent proof deliberately places a predecessor `spice` earlier in the
resident's ambient path; bare agent-shell commands still reach the installed
Rust appliance, and the predecessor invocation log stays absent.

## Tracked configuration

Tracked policy is physically separate. Python builds its repository layer from
`<worktree>/spice.toml` at
`spice-z/spice/config/layers.py#effective_commands`; Rust loads only
`<worktree>/spice-rust.toml` through
`spice/src/config/load.rs#const TRACKED_FILE`, and Gate uses that same Rust path
through `gate/src/input.rs#POLICY_PATH`. Neither runtime falls back to the
other's file when its own is absent or malformed.

The Python document retains one deliberate execution bridge: its
`policy.pre_commit` entry invokes the current Rust `spice gate pre-commit`
command while its own `policy.pre_commit_builtins` setting is false. That
command is owned by the predecessor hook and enters Rust from outside the Rust
configured-command recursion boundary; it is not a shared configuration read.
The Rust document carries the distinct self-hosting override: its built-ins are
disabled and its sole `Spice source gate` command builds and invokes exactly
`spice gate selection` through Cargo. A Rust configured command receives
`SPICE_RS_PRE_COMMIT_SOURCE`, so pointing that edge back at `spice gate
pre-commit` would be recursive and is refused. The product selection command is
the direct current-source leaf and does not redispatch. Machine-local
configuration is also separate; Rust reads only `.spice-rs/config.toml`.

The installed pre-commit dispatcher resolves that native policy once. It runs
the packaged selected-change gate exactly once when built-ins are enabled,
then runs configured commands in authored order; disabling built-ins runs only
the configured list, and disabling them with an empty list refuses before any
work begins. The native configured-command executor runs each admitted `run`
array as exact argv, never through a shell, from the canonical repository root.
One retained staged seal supplies every step with the same
`SPICE_RS_PRE_COMMIT_SOURCE`
digest and a `SPICE_RS_PRE_COMMIT_PATHS` JSON array containing only the staged
paths selected by that step's authored scopes. The source variable is also the
recursion tripwire: a configured command cannot redispatch `spice gate
pre-commit`. The tracked hook enters that installed dispatcher directly; its
configured Cargo delegate builds and invokes the public `gate selection` leaf,
so it is permitted under that source override without re-entering the
dispatcher. The child environment
is captured once, strips Git's local
repository variables and indexed `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*`
overrides, then replaces both Spice-owned values with the captured authority.
Standard input is null, output capture is bounded, and all configured steps
share one deadline and process-group cleanup budget. No configured command can
start until the current executable configuration digest has a durable operator
grant; index movement, policy movement, or trust revocation stops the battery.
The repository does not auto-grant its self-hosting command: changing the
tracked Rust document produces the complete digest/path/argv refusal and waits
for an explicit operator decision about those exact bytes.

Packaged change selection is repository-neutral. It compiles an applicability
profile from the immutable staged path inventory and the typed tracked
`spice-rust.toml`; no installed selector knows this source tree's crate names,
frontend directories, or source-only authority files. An absent tracked document supplies
no file-shape or language classes. Within a present document those three class
lists are required, repository-document omission retains Gate's generic
`AGENTS.md` default, absent package roots do not narrow Python selection,
absent generated paths exclude nothing, and absent test paths conservatively
leave every Python path eligible for test policy. Unknown paths and changes to
the tracked policy refuse before any selected arm or workspace test begins.
The richer Spice-workspace selector remains the separate public current-source
entrypoint `spice gate selection`; completion evidence remains the full
verifier.

## Git hook selection

Git exposes one effective `core.hooksPath` per worktree, so the two hook systems
are mutually exclusive there. Python generates its hooks from
`spice-z/spice/hooks/initplanning.py#HOOK_ARGS` and installs them through
`spice-z/spice/hooks/install.py#install_hooks_for_repo`. Rust publishes its
separate hook assets from `spice/src/install.rs#GIT_PRE_COMMIT_PATH` and resolves
them through `spice/src/install.rs#pub fn installed_git_hooks`.

Activation is part of the installed appliance's digest-bound plan.
`spice/src/install/hook_activation.rs#pub(super) fn plan` records every
worktree's effective and worktree-local selection before any asset write, and
`spice/src/install/error.rs#ForeignHookPath` refuses to seize a worktree
configured for Python. Apply publishes the executable hook assets first and
switches the common absolute `core.hooksPath` last; unapply restores the exact
prior values from the same receipt. No second activation command or Git
configuration implementation remains.
Hook choice is therefore an operator selection, not a cross-delivery path.

## Validation boundary

The tests cover both static and running coexistence:

- `spice/tests/coexistence.rs#this_runtime_claims_no_path_the_predecessor_owns`
  proves the whole Rust root is exclusive and outside Python's root.
- `spice/tests/coexistence.rs#running_this_port_leaves_predecessor_state_byte_identical`
  and
  `spice/tests/coexistence.rs#installing_this_port_leaves_predecessor_state_byte_identical`
  snapshot Python's entire tree around real commands.
- `spice/tests/resident/vendor_process/coexistence.rs#assert_coexistence_steering`
  proves same-UUID and cross-lane inbox/ACK isolation with both systems live.
- `spice/tests/resident/vendor_process/coexistence.rs#fn agent_shell_command`
  reproduces bare-shell lookup through the occupant environment.

The contract is intentionally asymmetric only where the repository cannot
execute the separate Python checkout. Rust proves it never touches Python
state. Python-side citations are dated measurements and must be rechecked in
that checkout whenever its paths change.
