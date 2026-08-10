# Coexistence record

Python Spice and Rust Spice may run in the same repository, but they do not
share writable routing state. They share only the tracked repository and the
operator's per-worktree choice of Git hook implementation. Routing identity,
pending direction, acknowledgments, resident discovery, sockets, configuration,
artifacts, receipts, and Git shadow files have distinct physical names.

`spice/tests/coexistence.rs` snapshots Python's complete state tree around real
Rust commands. The concurrent process proof is
`spice/tests/resident/vendor_process/coexistence.rs#installed_spice_lifecycle_leaves_the_predecessor_system_untouched`.
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

The Python document retains the one deliberate execution bridge: its sole
`policy.pre_commit` entry invokes the current Rust `spice gate pre-commit`
command while `policy.pre_commit_builtins` is false. That is a command owned by
the predecessor hook, not a shared configuration read. Rust's document omits
both predecessor-only bridge keys. Machine-local configuration is also
separate; Rust reads only `.spice-rs/config.toml`.

## Git hook selection

Git exposes one effective `core.hooksPath` per worktree, so the two hook systems
are mutually exclusive there. Python generates its hooks from
`spice-z/spice/hooks/initplanning.py#HOOK_ARGS` and installs them through
`spice-z/spice/hooks/install.py#install_hooks_for_repo`. Rust publishes its
separate hook assets from `spice/src/install.rs#GIT_PRE_COMMIT_PATH` and resolves
them through `spice/src/install.rs#pub fn installed_git_hooks`.

Activation is guarded: `xtask/src/hooks.rs#fn preflight_worktree_hooks` checks
every worktree before writing, and `xtask/src/hooks.rs#ForeignHookPath` refuses
to seize a worktree configured for Python. Hook choice is therefore an operator
selection, not a cross-delivery path.

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
