# DISK

Every byte at rest has one owner, life, and recovery contract. The
appliance is identified by its target manifest. The operated repository holds
one authority database, disposable materializations, captured vendor bytes,
and explicit local surfaces. Vendor files remain outside that contract and
serve only as forensic sediment.

## The appliance

One `spice` executable ships for each release target:
`x86_64-unknown-linux-musl` and `aarch64-apple-darwin`. A canonical target
manifest binds the target tuple to the executable digest, embedded app digest,
toolchain and container identity, and proof receipt. Copies with the same
manifest identity are the same appliance; identities are compared only within
their target tuple.

The executable embeds the canvas app and every builtin. Installation copies
the executable; it does not create a shared install tree, defaults file, XDG
state, or user dotfile. Defaults are compiled values covered by the executable
and proof digests. Repository policy may select among exposed behavior, but no
file outside the executable can silently redefine a builtin.

## Tracked

The operated repository has three tracked surfaces:

| Path | Contract |
| --- | --- |
| `spice.toml` | Operator-authored policy and repository configuration. Changes are reviewed and versioned with the code they govern. |
| `baseline.toml` | Versioned shrink-only floors for gate measures, including mutation kill rates admitted only after N-run confirmation. A measured improvement moves a floor down; moving one up requires an operator commit. Sealed proof re-derives the floors, and the file carries an explicit version. |
| `.agents/skills/spice/` | The generated worktree skill derived from the current clap tree. Generation and staleness checks make it an artifact of the executable surface, never a separately maintained command manual. |

Tracked files express portable operator intent. They contain no live claim,
lease, cursor, capability, receipt, or resident state.

## Untracked topology

Repository-common state lives under `<git-common>/.spice/`, where
`<git-common>` is resolved by Git rather than inferred from a worktree path.
The complete release-one topology is:

```text
<git-common>/.spice/
├── data/
│   ├── spicejournal.sqlite3
│   └── spicefolds.sqlite3
├── locks/
├── resident
├── seq
├── canvas
├── inbox/
├── degraded/
├── install
├── proof/
│   └── <rev>/
├── tape/
├── hooks/
│   └── static/
└── config.toml
```

**`data/spicejournal.sqlite3`.** The sole authority. One SQLite transaction
commits a command id, immutable facts, authoritative task statement and
workflow heads, coordination heads, command results, and the wake cursor.
The resident opens it only after acquiring the kernel lock. Its schema version
row names the one current at-rest format.

**`data/spicefolds.sqlite3`.** The disposable fold store. Each registered fold
publishes a complete generation transactionally from declared journal, head,
fold, tracked-config, or tape sources. Authority never reads it. Deleting it
costs replay, not truth — with one stated exception: the bounded
`bus::refusals` telemetry ring lives here, is derived from nothing, and does
not survive deletion, replay, or restore. That loss is accepted; it is the
only argument this store holds that replay cannot return.

**`locks/`.** The resource-lock floor. Each configured lock or pool shard
is one flock target under `locks/<name>/<shard>`, held by `spice lock run`
for the life of one child command, with holder metadata (pid, cwd,
started instant) written inside the held file and cleared on release.
Contention never blocks: it exits with the configured busy, taken, or
full code. The kernel-held flock is the ownership fact; the metadata is
diagnosis. Deleting an unheld file is harmless.

**`resident`.** The process-life writer-lock target and diagnostic record.
The resident acquires its advisory `flock` before opening the journal, retains
the descriptor with `CLOEXEC`, and spawns children only through the exec path,
so no child inherits ownership. The recorded pid and epoch aid diagnosis; the
kernel-held lock is the ownership fact. Release one has no live takeover.

**`seq`.** The wake marker for the journal's monotone cursor. A commit advances
the durable cursor in the journal and publishes this marker atomically.
Watchers treat its notification as a hint, then read the journal cursor as
truth. Loss or duplication of a notification cannot lose a committed fact.

**`canvas`.** The mode-`0600` loopback discovery file. It carries the resident
endpoint, operator capability, and resident epoch created by `spice up` as
one atomically replaced tuple, so a client can never read a fresh endpoint
with a stale epoch or the inverse. It is the only capability transport: the
capability never appears in a URL, query string, or cookie. Possession of
copied attribution data does not substitute for the capability.

**`inbox/`.** Per-lane renderings of pending durable directives. They are
regenerated when the journal advances and are safe for the agent hook to read
without opening authority. A rendered file can be deleted or stale; directive
disposition remains in the journal.

**`degraded/`.** Per-lane atomic disk evidence, `degraded/<lane>`, left when
an agent run cannot reach the resident. The run continues under its measured
contract. The next resident recognizes and consumes each lane's marker once,
committing the corresponding observation rather than trusting the file as
authority. Markers are per lane so concurrent outages cannot overwrite one
another's only evidence.

**`install`.** The durable install record `--unapply` reverses from: the
prior Git hook setting, every path install wrote, the generated-skill and
unit-template destinations, and the digests of the seeded tracked files.
Written atomically by `spice install --apply`; without it, unapply refuses
rather than guessing.

**`proof/<rev>/`.** The sealed evidence bundle for one revision. Proof creates
it from the manifest emitted by `dist`; publication reads the bundle named by
the target manifest. The revision qualifies the path, so evidence from one
tree cannot answer for another.

**`tape/`.** Spice-owned vendor bytes, and confidential evidence: the
directory is mode `0700` and every segment and sealed member is mode `0600`,
because verbatim transcripts can carry secrets, source, and tool arguments.
A lane appends verbatim frames to an active plaintext segment before
interpretation. Rotation seals that segment atomically as an indexed
immutable gzip member. A citation locus is `(member, offset, length)`:
compression, archive, and later reads preserve the same address without
referring back to vendor files. Tape bytes leave this directory only inside
an operator-held backup bundle, and render only behind the operator
capability; no unauthenticated surface serves them.

**`hooks/`.** Installed Git and shell entrypoints. Installation records the
previous Git hook setting before selecting this directory. Reinstallation is
idempotent; unapply restores the recorded setting rather than blindly
clearing it.

**`hooks/static/`.** The static shell-hook stage used after the first wrapped
shell enters the agent environment. It carries recursion guards and fixed
handoff code so descendants do not bootstrap the resident boundary again.

**`config.toml`.** Repository-local, untracked overrides for exposed
configuration domains. Command-line intent wins over it, and tracked
`spice.toml` remains the portable policy surface. Removing the file restores
compiled and tracked resolution; it does not delete authority.

Nothing else may appear as release-one spice state. A new path requires an
explicit owner, durability class, recovery operation, and lifecycle before it
can enter this inventory.

## RTK worktree scope

RTK is an optional external tool with a named, doctor-visible configuration
seam. Its context follows a worktree and thread, not repository authority:

```text
<worktree-git-dir>/.spice/
└── data/
    └── <thread-uuid>/
        └── rtkhistory.sqlite3
```

`<worktree-git-dir>` is resolved with `git rev-parse --git-dir`, never
`--git-common-dir`. The two may physically coincide for the main worktree, but
their scopes do not. `<thread-uuid>` uses lower-case `8-4-4-4-12` spelling.
The database is durable for that thread and disposable with explicit thread
or worktree cleanup. It is context owned by RTK, not a fact, head, fold,
receipt, or source of repository truth. No dormant RTK branch exists when the
tool is unavailable.

## Ownership law

The journal is the sole authority and the resident is its sole writer. The
head tables and immutable facts inside the same transaction answer current
state; no filesystem marker, fold, tape member, hook, tracked file, or vendor
file can overrule them.

Durability follows recoverability:

- The journal is durable authority. Loss requires restore from a verified
  backup.
- Tape members are durable forensic evidence. They are append-only while
  active and immutable after sealing.
- Tracked files are durable operator intent under Git.
- `config.toml` and RTK history are owner-authored local state with their own
  explicit lives, never repository authority.
- Folds, `seq`, `canvas`, `inbox/`, `degraded/`, `locks/`, and installed
  hooks are disposable or life-bound renderings. Their named producer
  recreates them; only the refusal ring inside the folds store is lost with
  them, and that loss is accepted above.
- Proof bundles are reproducible evidence for a named revision, but a release
  may cite only the exact bundle bound into its target manifest.

`spice backup OUTPUT` quiesces the resident and snapshots the single journal
database through SQLite's consistent backup path. It writes a manifest that
enumerates tape members and their identities. Recovery code names every input
filename; it never recursively globs `data/`, `tape/`, or the worktree-local
RTK tree. The folds database is excluded because replay heals it.

## Operations discipline

**Verify.** `spice journal verify` checks envelope decoding, sequence and
thread continuity, head agreement, command results, and schema identity.
`spice restore verify BUNDLE` checks a bundle and its tape manifest without
mutating repository state. A successful check names the exact artifact and
version it examined.

**Backup and restore.** `spice backup OUTPUT` first quiesces authority writes,
takes the one-database snapshot, records the tape manifest, recognizes
`journal::backed`, and then releases the quiescence boundary. The bundle is
operator-held secret material — it contains verbatim tape.

Restore is the one operation outside the ordinary command door, because it
replaces the store that would record it. `spice restore BUNDLE` previews the
exact recovery and prints its plan digest; `spice restore BUNDLE
--apply=<digest>` then runs the lock-held protocol: verify the digest-pinned
plan, stop admission, close authority, exchange staged bytes atomically,
reopen and verify, mint a new epoch, and recognize `journal::restored` in
the reopened journal. A changed bundle or stale digest refuses before any
byte moves. Restore never adopts ambient files or discovers inputs by glob.

**Crash drills.** The battery injects failure at journal seams: before and
after the authority transaction, between commit and `seq` publication, and
between publication and command reply. An uncommitted transaction disappears;
a committed command remains authoritative; a missing marker is reconstructed
from the journal cursor; and retrying a committed command id replays its
recorded result. Fold publication likewise exposes either the old complete
generation or the new complete generation, never a partial answer.

**Migration.** Migration is a command executed by `spice up` before it mints a
resident epoch or admits the fleet. It prepares one forward transition,
records the schema version in the journal, finalizes the current path, and is
covered by a crash drill at both boundaries; the seated resident then
recognizes `journal::migrated` so recovery appears on chronology. `baseline.toml` carries its own
version so gate floors cannot be interpreted under an unnamed schema. There
is no permanent upcast arm and no second at-rest clock.

**Retention.** Active tape rotates into immutable compressed members; release
one archives and does not prune them automatically. Tape pruning remains
absent until its recorded evidence trigger is met. Fold generations may be
discarded after a replacement publishes. Discovery files, markers, renderings,
and proof bundles are removed only according to the lifecycle table below;
none is retained as hidden authority.

## The clean cutover

Python spice and Rust spice never open the same authority state, either
concurrently or in sequence. Cutover drains the old fleet, exports wanted work
as a task document, stops every old process, verifies a backup, and moves the
old state out of the release-one topology. The Rust resident then acquires the
new lock and creates an empty journal; disposable folds derive from that empty
authority.

Legacy authority files cause a refusal naming the clean-break release and the
required backup-and-empty remedy. No schema bridge, import, handle
translation, or compatibility writer exists. Applying an exported task
document expresses new intent through the current document command; it is not
state migration.

Cutover is executed from an operator shell outside the fleet being drained —
never as a task inside it — and every boundary of the exchange is covered by
a kill-and-restart drill. Its rollback boundary is explicit: restoring the
predecessor backup is legal only until the Rust journal admits its first
intent; after that first command the exchange is forward-only, and a
predecessor restore would destroy new truth.

## Vendor sediment

```text
~/.claude/projects/…/*.jsonl
~/.codex/sessions/…
```

These paths belong to their vendors. Spice may read them for daemonless
`watch` and `retro` forensics, preserving absence, unknown fields, malformed
residue, and canary observations, but it never treats them as authority or
promises their retention. Supervised live bytes are written verbatim to
`tape/` before typed decoding. Durable citations therefore name a spice-owned
tape locus, never a vendor path.

## Lifecycles table

| Path | Born | Grows or changes by | Dies or recovers |
| --- | --- | --- | --- |
| `spice` | `dist` seals a target build and manifest | Never in place; a new build has a new digest | Replaced by another manifest-identified target artifact |
| `spice.toml` | Operator's first tracked policy commit | Operator commits | With the repository |
| `baseline.toml` | Install seeds a versioned floor set | Confirmed improvements shrink floors; operator commits alone may raise them | With the repository; Git restores it |
| `.agents/skills/spice/` | Install generates it from the clap tree | Regeneration under the staleness gate | Unapply removes the generated artifact |
| `data/spicejournal.sqlite3` | First `spice up` creates or migrates the current schema before epoch mint | One guarded journal transaction per accepted command | Never by policy; a verified backup restores loss |
| `data/spicefolds.sqlite3` | First requested fold materialization | Transactional generation publication | Freely; replay reconstructs every fold, and the refusal ring alone is accepted loss |
| `locks/` | First `spice lock run` against a configured name | Holder metadata written while held, cleared on release | Freely when unheld; the kernel flock is the ownership fact |
| `install` | `spice install --apply` records what it wrote | Rewritten atomically by each apply | `--unapply` consumes it to reverse the exact install |
| `resident` | `spice up` acquires the kernel lock before authority opens | Diagnostic pid and epoch may be atomically replaced | The file may persist; process death releases the kernel lock |
| `seq` | The journal establishes its first cursor | Each committed cursor publishes a marker | Freely; journal maximum reconstructs it |
| `canvas` | `spice up` creates the loopback endpoint and operator capability | Atomically replaced at each launch | With the resident; the next launch recreates it |
| `inbox/` | A lane first has a pending directive | Journal wakes regenerate lane renderings | Freely; the journal disposition regenerates it |
| `degraded/` | A lane's agent run cannot reach the resident | Never; one marker per lane represents pending disk evidence | The next resident consumes each lane's marker once |
| `proof/<rev>/` | Proof seals evidence for `<rev>` from the `dist` manifest | Never for that revision | Superseded by proof for another revision; reproducible from its pinned inputs |
| `tape/` | Supervision receives a lane's first vendor frame | Verbatim frames append to the active segment; rotation indexes and compresses it into an immutable member | Sealed members are retained in release one; there is no automatic pruning |
| `hooks/` | `spice install --apply` records prior hook state and installs entrypoints | Idempotent reinstall | `--unapply` restores the recorded prior hook state |
| `hooks/static/` | Hook installation emits the static stage | Regeneration with the installed hook artifact | With `hooks/` |
| `config.toml` | First repository-local override | Operator edits | Freely; removing it restores tracked and compiled resolution |
| `data/<thread-uuid>/rtkhistory.sqlite3` under the worktree Git dir | RTK first records context for the thread | RTK writes in that worktree | Explicitly with the thread or worktree |
| `~/.claude/projects/…/*.jsonl` | Claude creates vendor history | Claude writes and rotates it | By vendor policy; spice provides no recovery contract |
| `~/.codex/sessions/…` | Codex creates vendor history | Codex writes and rotates it | By vendor policy; spice provides no recovery contract |
