# Coexistence record

Every integration point between the Python predecessor and this Rust port,
enumerated. The port runs beside the predecessor rather than after it, so the
question this record answers is not "what is left to port" but "where do the two
running systems touch, and which of those touches is deliberate".

`spice/tests/coexistence.rs` enforces the byte-identity half of the policy that
`spice/src/repository.rs` states. This file is the map that enforcement was
written against, and `spice/tests/coexistence_record.rs` keeps its citations
honest.

## Reading a citation

A citation is written `` `path#anchor` ``: a repository-relative path, then a
literal string that must occur in that file. The anchor is a symbol name where
the sentence names a symbol, and a configuration key or filename where it names
one of those, because the port declares some of these through macros that no
symbol table would list.

Anchors rather than line numbers because line numbers rot silently. Three
generations of this record cited by line, and by the time it moved into the tree
`spice/src/repository.rs#admit_state` had drifted from line 457 to 468 with
nothing to notice. An anchor moves with the code it names, and a line citation
added here is refused rather than merely discouraged.

A path beginning `spice-z/` names the predecessor tree, which is a separate
checkout and not a path in this repository. Those citations are deliberately
unresolvable from here; the test asserts they stay that way, so a predecessor
path that ever becomes a real file here is reported rather than silently
resolved against the wrong tree.

## The two measured negatives

These determine the shape of everything else: there is no direct call seam in
either direction between this port and the predecessor. Every seam below is
shared state, shared configuration, or Git's hook dispatch.

**No Python code invokes this port.** The predecessor does invoke a Rust binary
-- `spice-z/spice/tasks/ops.py#run_tool_command` runs `rtk gain --project -f
json`, where the executable comes from
`spice-z/spice/config/values.py#configured_rtk_executable` under
`spice-z/spice/config/values.py#RTK_KEY`, and `rtk` is Rust, installed by
`cargo install --git` per `spice-z/tests/test_rtkdocs.py#cargo install`. So the
negative is narrowed to this port and carries its counterexample.

The original search terms -- `cargo`, `target/debug`, `target/release` -- could
not have found that call, and the durable lesson is the miss rather than the
correction. Those three strings match how a Rust program is built or located
inside a build tree. That is sufficient evidence of an invocation and not
necessary for one: a binary installed and then invoked under its own name,
resolved from configuration or `PATH`, exhibits none of them. It is exactly the
shape a predecessor-to-port call would take, since the port installs as `spice`
under `<common>/.spice-install` with no build path involved. A negative about
invocation is a claim about call sites, so it has to be proved by enumerating
call sites -- `run_tool_command`, `subprocess`, `Command::new` -- and
classifying what each runs, not by searching for build paths. Re-swept on those
terms, no predecessor call site runs this port.

**No Rust code launches the predecessor.** The only `Command::new("spice")` in
the port is `spice/src/cli.rs#Command::new("spice")`, which is clap's parser
name, identified from the surrounding `.about` and `.subcommand_required`
builder chain rather than from the call alone.

Two attempts to break these negatives by omission also failed, and are recorded
so they are not re-run. The two sides share no `SPICE_` environment name at all
-- 20 in the port against 29 in the predecessor, empty intersection, checked
non-vacuously. And the binary-name convergence is not a seam: both build a
program named `spice`, but the port installs under `<common>/.spice-install`
while the live `core.hooksPath` is `.spice/hooks`, and `spice-install` appears
nowhere in the predecessor tree.

## Seam A -- the shared state root

**Deliberate, tested.** Both sides anchor at the Git common directory: the live
value is that directory's `.spice`, which for a fleet of linked worktrees is one
shared path rather than one per lane.

- Predecessor: `spice-z/spice/tasks/config.py#backend_root`,
  `spice-z/spice/paths.py#shared_state_root`.
- Port: `spice/src/repository.rs#admit_state`,
  `gate/src/state.rs#current_state_path` taking a common Git directory,
  `spice/src/config/load.rs#locations`.

Policed by `spice/tests/coexistence.rs#this_runtime_claims_no_path_the_predecessor_owns`
and coexistence policy point 2. The rationale is that the port anchors here
because the worktree is unpredictable and can be force committed.

## Seam B -- disjoint stores in one data directory

**Deliberate, tested one-directionally.** Both sides keep their databases in
`<common>/.spice/data/`.

Predecessor declarations, each a named constant:

- `spice-z/spice/tasks/opslog.py#OPERATIONS_DB_FILENAME` -- `taskchampion.sqlite3`
- `spice-z/spice/mail/ackstate.py#ACK_STATE_DATABASE_FILENAME` -- `spiceacks.sqlite3`
- `spice-z/spice/agent/maximmetrics.py#MAXIM_METRICS_DATABASE_FILENAME` -- `spicemaxims.sqlite3`
- `spice-z/spice/serve/team/projection.py#PROJECTION_DATABASE_FILENAME` -- `spiceprojections.sqlite3`
- `spice-z/spice/serve/team/schema.py#TEAM_DATABASE_FILENAME` -- `spiceteams.sqlite3`

Port: `spice/src/repository.rs#spicejournal.sqlite3` and
`spice/src/repository.rs#spicefolds.sqlite3` declare the two stores under
`data/`, and `spice/src/journal_command.rs#AUTHORITY_MEMBER` is the same journal
name. The port owns two stores, not one.

Live, all six predecessor-and-port files sit in that one data directory.
`spicefolds.sqlite3` is declared and pinned but not yet materialized in this
fleet, which is a fact about what has run here rather than a gap in the record.

The port pins the boundary: `spice/tests/resident/vendor_process.rs#assert_unchanged`
seeds the five predecessor names with sentinel bytes and is driven by
`spice/tests/resident/vendor_process.rs#installed_spice_lifecycle_leaves_the_predecessor_system_untouched`
across a full install, upgrade, allocation, steering and shutdown.

The reciprocal is absent **for stores specifically**, and that scope is
load-bearing: nothing in the predecessor tree asserts anything about
`spicejournal.sqlite3`, which is unreferenced anywhere in that tree rather than
merely in its `.py` files. The exact non-vacuity searches and their measured
file counts are:

| Predecessor search | Files |
| --- | ---: |
| `git grep -l spicemaxims` | 8 |
| `git grep -l spicemaxims -- tests/` | 3 |
| `git grep -l spicejournal` | 0 |

The zero is therefore the predecessor's silence about the port's store and not
a search that cannot find anything. All three figures are measurements of a
moving tree rather than constants; `spice/tests/coexistence_record.rs` reads
them from this table and runs the named searches against the predecessor
checkout so drift fails. The tree-wide maxim result holds for the bare token and
the full `spicemaxims.sqlite3` filename alike, over the tracked set or over the
working tree with `__pycache__` excluded. The tests-only result is a defensible
search for a sentence about what the predecessor asserts, but not the search the
preceding sentence describes; an earlier generation of this record confused
the two. This is not a general claim that the predecessor tests nothing about
the port. It does, about configuration vocabulary, which is Seam F.

## Seam C -- the agent gitconfig

**Was an undocumented convergence, now closed.** Both sides write
`<git-dir>/agent.gitconfig`.

- Predecessor: `spice-z/spice/agent/shadow.py#AGENT_GITCONFIG_NAME`,
  `spice-z/spice/agent/shadow.py#config_path`, and a
  `rev-parse --absolute-git-dir` probe in the same file.
- Port: `spice/src/git_shadow.rs#AGENT_GITCONFIG_NAME`, the same probe, and
  `spice/src/git_shadow.rs#publish_shadow`.

Both write the whole file and neither contributes an entry; it is the only
shared path outside any `.spice`. Closed under EFFORT-1kKp195G in `a263f4e3`:
the decision record in `spice/src/repository.rs#agent.gitconfig` names it, and
`spice/tests/git_shadow.rs` pins the anchor and filename against a linked
worktree, where `--absolute-git-dir` and `--git-common-dir` differ.

## Seam D -- the port's own spice.toml is read by the predecessor

**Deliberate, and the largest live seam.**

- Predecessor: `spice-z/spice/config/layers.py#effective_commands` builds the
  system, repository and worktree layers; for a port repository root the
  repository layer resolves to the port's own tracked `spice.toml`. It consumes
  `policy.languages` at `spice-z/spice/policyconfig.py#languages` and
  `policy.file_shape` at `spice-z/spice/policyconfig.py#file_shape`, with
  `spice-z/spice/policy.py#file_shape` pairing to them.
- Port: `spice.toml#[policy.file_shape]` declares `.rs` in its
  `source_suffixes`, and `spice.toml#[policy.languages]` declares the
  `complexity` and `magic` language sets.

Net effect: the predecessor classifies and measures Rust source using policy the
Rust repository declares about itself. Reproduced live rather than read from the
schema -- a layered load over a lane root returns `.rs` in
`policy.file_shape.source_suffixes` and in `policy.languages.magic`.

## Seam E -- Git's hook dispatch

**Deliberate, mutually exclusive by design.**

Live scope: `core.hooksPath` is set at `--worktree` scope to the relative
`.spice/hooks`; `--local` and `--global` are both unset. Relative means Git
resolves it per worktree, and `/.spice/` is excluded in the common directory's
`info/exclude`. So the three hook scripts are per-lane untracked state, not one
shared installation.

The three are five-line files: shebang, blank, `set -eu`, blank, and `exec spice
dev pre-commit` / `commit-msg "$1"` / `reference-transaction "$1"`.

- Predecessor: `spice-z/spice/hooks/initplanning.py#HOOK_ARGS` is the template
  those five-line files are generated from, verbatim;
  `spice-z/spice/hooks/install.py#install_hooks_for_repo` writes them and points
  `core.hooksPath` at them; `spice-z/spice/hooks/cli.py#configure_dev_parser`
  registers the three verbs and `spice-z/spice/hooks/cli.py#handle_dev`
  dispatches them.
- Port: `spice/src/main.rs#gate_command::pre_commit` routes `gate pre-commit` to
  `spice/src/gate_command.rs#pre_commit`, and `spice/src/cli.rs#landing` admits
  the route.

The port also ships its own complete hook set:
`spice/src/install.rs#GIT_PRE_COMMIT_PATH`,
`spice/src/install.rs#GIT_COMMIT_MESSAGE_PATH` and
`spice/src/install.rs#GIT_REFERENCE_TRANSACTION_PATH` declare it under
`.spice-install/git-hooks/`, `spice/src/install.rs#asset_contract` maps each to
the common Git directory, and `spice/src/install.rs#installed_git_hooks`
resolves them there. That directory does not exist live, so this set has never
been installed in this fleet.

Two things about it are worth the record. Its pre-commit body does not run the
Rust gate at all -- it execs `cargo xtask verify` -- so activating it would give
a weaker formatter check than the gate itself, since `cargo xtask verify` still
formats through `cargo fmt` and is therefore blind to `include!` targets. And
activation is guarded rather than eager: `xtask/src/hooks.rs` states that
same-worktree coexistence is unsupported because `core.hooksPath` is
single-valued per scope, that per-repository coexistence is real because
`extensions.worktreeConfig` makes per-worktree selection work, and that one path
dispatching to both is rejected because `reference-transaction` runs while the
repository-global `packed-refs` lock is held.
`xtask/src/hooks.rs#preflight_worktree_hooks` reads every worktree's effective
value before any write, worktree overrides are cleared only after that passes,
`--local` is then set, and the `xtask/src/hooks.rs#ForeignHookPath` refusal says
to keep Rust and predecessor hooks in separate worktrees.

So the accurate statement is not that the predecessor's hooks run on Rust
commits. It is that two complete hook systems exist, they are mutually exclusive
per worktree by explicit design, the port refuses to seize a foreign path, and
only the predecessor's is activated here.

## Seam F -- keys the predecessor schema permits but never reads

**Deliberate, load-bearing.**

`policy.admission` is declared once, at
`spice-z/spice/config/schema.py#policy.admission`, and no production Python
reads the value. That narrow claim is the one the evidence supports; the key is
not confined to the schema table.

The predecessor carries a regression test for it:
`spice-z/tests/test_configlayer.py#test_repository_configuration_admits_the_rust_publisher_admission_argv`.
It writes a `spice.toml` declaring `policy.admission` as this port's own
publisher argv -- `cargo metadata --no-deps --offline --format-version 1` --
runs the real layered load, asserts the effective value is that six-element
argv, and asserts its source is the repository layer specifically rather than
any layer. Its own comment says why it is not a schema-table test: the Rust
cutover tree sets this argv and every runtime reads the same repository
configuration, so admitting the key has to survive the real layered load.
Reproduced live here rather than read.

That test is the reciprocal Seam B lacks for stores. The predecessor is silent
about the port's databases and deliberately not silent about the port's
configuration vocabulary, and the difference is the interesting part of both
seams.

The port consumes the key at `spice/src/config.rs#policy.admission`,
`gate/src/core/config_keys.rs#policy.admission` and
`spice/src/task_git/admission.rs#policy.admission`.

The top-level taste rows are the same shape, and
`spice-z/spice/config/schema.py#policy.taste.words` says so outright:
`policy.taste.words` stays the predecessor's lexical map while the named rows
are evaluated by the Rust taste judge. This class is load-bearing rather than
cosmetic, because an unknown `spice.toml` key makes the predecessor refuse
outright; its schema had to learn the port's vocabulary for the port to be
usable at all.

## Seam G -- maxim bags fire on Rust lanes

**Incidental.** This is the seam where one side legitimately admits no line.

- Predecessor: six bags from `spice-z/spice/spice.toml#[maxims.polling]`.
- Port: none, and that absence is the evidence, so there is no port-side line to
  give.

Fleet-wide from the shared `spicemaxims.sqlite3` at one reading: 1945 fires, 702
gate-suppressed, 1243 published, judged-confirmed and judged-rejected both 0 on
every row. Predecessor behaviour reaching Rust lanes' prose while exercising no
Rust path. Those counts are a live reading of a shared store that every lane
writes; they move between one measurement and the next and are a sample, not a
constant.

## The open discovery

The port declares six `[taste.NAME]` rows in `spice.toml#[taste.` and no
`policy.taste.words`. `taste/src/config.rs` parses those rows first-class:
`taste/src/config.rs#pub struct Config`, `taste/src/config.rs#struct Row` with
`deny_unknown_fields`, and `taste/src/config.rs#pub fn configured`. The
predecessor's pre-commit taste step at
`spice-z/spice/hooks/precommit.py#taste` is the lexical map the port does not
declare. So at commit time those six rows are read by nobody, and whether the
resident evaluates them live remains the open question;
`wire/tests/vocabulary.rs#taste::confirmed` and
`app/tests/runtime/invalidation_and_chronology.rs#taste::cleared` show the
vocabulary exists.

## Candidate new trigger points

Cheapest first. Every one of these changes the port's own tracked
configuration, its own lane-local hook directory, or adds new code on the
predecessor side of a store it already owns. Nothing requiring a change to
operator-owned predecessor configuration was applied, and none is proposed.

1. **A `[commands]` entry in the port's `spice.toml`.**
   `spice-z/spice/config/layers.py#effective_commands` exposes named commands;
   the port declares no `[commands]` table while the predecessor declares one.
   Zero automatic cost, nothing runs unless invoked.
2. **A `policy.pre_commit` row in the port's `spice.toml`** running the Rust
   gate, scoped by paths. The mechanism is proven in the predecessor's own
   `spice.toml`, which declares `run` plus `scopes.paths`; the port declares
   zero rows. This is the better route than activating the port's own hook set,
   because it adds the Rust gate to the live predecessor hook rather than
   contesting `core.hooksPath`, and because the port's hook would run `cargo
   xtask verify` instead of the gate. Cost: the debug binary must exist and be
   current, a stale one refuses rather than fails open, and it adds the gate's
   wall time to a commit already paying the predecessor's. Scope to `gate/**`
   first.
3. **A read-only predecessor projection over `spicejournal.sqlite3`**, the only
   port-owned store in a directory the predecessor already uses. Strongest
   available parity evidence short of a cutover; cost is a new reader that must
   tolerate schema drift, since journal schema 2 has no migration from 1.
4. **Activating the port's hook set in a dedicated worktree**, which
   `xtask/src/hooks.rs#extensions.worktreeConfig` explicitly supports. Highest
   exercise value and highest risk, because `reference-transaction` runs under
   the `packed-refs` lock; propose it last and only behind a bounded timeout.

## Provenance

This record was written three times as task validations before it landed here,
and each generation superseded the last **whole**, because a completed task's
validation cannot be amended:

- EFFORT-1kKp2Hjr wrote it. **Superseded, do not read as current.**
- EFFORT-1kKsXcb8 restated it whole to correct four citations. **Superseded, do
  not read as current.**
- EFFORT-1kKssls7 restated it whole again to correct two false claims, one scope
  error and five anchors. It was the newest generation, and this file supersedes
  it.

The corrections that history records are folded in above rather than listed:
`policy.admission` is stated in its narrow form with the predecessor's own test
cited, the invocation negative is narrowed and carries its counterexample, Seam
B's missing reciprocal is scoped to stores, and every anchor names a symbol
instead of a line.

EFFORT-1kKtbwdd moved the record here so the next correction is a diff rather
than a fourth restatement. That is the whole point of the move: from here on,
the commit that falsifies a sentence can amend it, and `git log` carries the
generations that used to have to be carried in prose.
