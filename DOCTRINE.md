# DOCTRINE

## The kernel

**Transcript.** The transcript is the exact record of an occupant's exchange
with its vendor. Spice captures vendor bytes verbatim before classification,
preserves missing fields and unknown forms, and binds every interpretation to
the bytes that support it. Facts may describe that record; folds may arrange
it; neither may revise it. When interpretation and sediment disagree, the
sediment wins as evidence and the interpretation becomes a named defect.

**Board.** The board is the surface through which work steers work. It reads
authoritative task statements, workflow heads, claims, and coordination heads
from the journal, ranks them once, and packs them with explicit overflow.
Operators and occupants receive the same current answer at a named cursor.
Selection remains the allocator's act: an occupant does not choose work by
reading a convenient row, and a stale rendering never grants possession.

**Stigmergy.** Coordination occurs through durable changes to the shared work
surface, not private messages between occupants. Daemonless commands observe
without appending. The resident alone recognizes eligible disk evidence and
commits the corresponding fact. Every occupant can therefore explain its next
act from the board and journal, while every hermetic rung remains usable
without quietly changing the system it examines.

**Authority.** One foreground resident holds the kernel lock before opening
the sole authority database. Within it, one journal transaction commits the
command id, immutable facts, authoritative heads, coordination heads,
result, and wake cursor. No command, fold, canvas, hook, or offline path owns a
second writer. An epoch rejects stale clients; only loss of the kernel lock
ends ownership.

**Break.** Release one starts clean. Cutover drains the old fleet, exports only
wanted work as a task document, takes a verified backup, and opens empty
authorities under the new binary. It is executed from an operator shell
outside the fleet being drained, and its rollback boundary is explicit:
restoring the predecessor is legal only until the new journal admits its
first intent, after which the exchange is forward-only. Old handles, schema,
and executable state do not cross. Historical material remains evidence
outside the new authority; it never becomes an import path or a
compatibility arm.

**Refusal.** A withdrawn or inadmissible act refuses loudly with the attempted
surface, the release that withdrew or constrained it, and the available
remedy. A refusal is not a fact and cannot pretend the world changed. It
returns the refusal result class and exit 2; malformed execution or broken
invariants fault and exit 1; completed requests reply and exit 0. Silence is
reserved for channels whose contract explicitly permits silence.

**Wake.** Signals are hints; cursors are truth. Discovery never polls. A
waiter reads the durable condition, arms its notification, reads again, then
blocks within a named bound. After any wake it consumes the durable gap rather
than trusting the signal count. Lost, duplicated, or reordered hints can delay
a bounded recheck, but they cannot lose committed work or create authority.
A durable deadline is not scheduled until something recomputes the wake from
it: the timer wheel is a derivation over deadline-bearing heads, rebuilt at
every epoch mint and after every commit that touches a deadline, so a
restarted resident wakes every deferral and lapses every dead lease without
a sweeper. Deadlines persist as UTC instants, waits run on monotonic clocks,
and an observed wall-clock regression faults by name.

**Channels.** Every channel is either blocking or silent by contract. A
blocking channel owns its capacity, deadline, cancellation, child cleanup, and
restart disposition. A silent channel may discard hints but never durable
state, authorization, or a result that the caller was promised. No queue
silently changes posture when pressure rises.

**Outcomes.** The operator reads outcomes, never code. Replies, refusals, and
faults name the subject, cursor, artifact identity, and any bounded next action
needed to continue. Generated contract tables connect each command leaf to its
principal, authority, result class, and facts, while receipts bind proof to the
tree and policy measured. Code structure matters only where these observable
contracts require it.

**Evidence.** Evidence precedes every surface that cites it. Transcript bytes
are durable before semantic recognition; a target manifest exists before
proof names an artifact; receipts exist before a finding is presented as
settled. Release one carries only evidence needed by a shipped story. A
possible surface without present evidence belongs in `LATER.md` with one
observable trigger, never as dormant configuration, schema, or code.

## The decision records

### One authority

The journal is the sole authority. Task statements, workflow state, claims,
leases, trust, coordination, command identities, results, and the wake cursor
commit in one SQLite transaction under the resident's guard. The board reads
the authoritative head tables directly; folds are disposable canvas
materializations and never stand between a command and its truth.

This resolution follows the clean break. Release one retains no external task
writer, synchronization protocol, recurrence engine, imported state, working
set numbering, or undo facility whose value could repay a second durable
store. The old split therefore imposed failure modes without preserving a
needed capability: commands could cross stores, reads needed composite
cursors, and recovery had to infer which half of an act had landed.

The resolution deletes the second task database, cross-store plans,
before-and-after fingerprints, divergence classifications, serialized board
reads, paired backup, dual migration, and composite cursors. Crash recovery is
the journal transaction matrix only. The measured claim race and transaction
latency remain in the proof corpus as scars; none remains as product
architecture.

### Principals & cooperative threat model

Release one assumes cooperative occupants running as one Unix user.
Capabilities provide attribution and workflow interlocks, not adversarial
isolation. The operator capability is created by `up` and held through the
canvas discovery file. Each lane capability is minted at seating and bound to
the lane, occupant, and occupant epoch. The resident derives actor and rights
when it accepts the connection; caller fields may describe provenance but
cannot grant it.

This boundary states what the machine can honestly enforce. Processes sharing
an unrestricted user can inspect one another's files and memory through the
ordinary operating environment, so stronger claims would turn token names
into security theater. A second Unix identity running a lane is the evidence
trigger for adversarial isolation in `LATER.md`.

The resolution deletes ambient actor claims, caller-selected authority,
bearer strings treated as a user boundary, and policy paths that imply one
occupant is protected from another. It retains exact principal binding for
command ids, operator-only trust grants, and lane-scoped command rights
because those controls make attribution and orderly operation deterministic.

### Three clocks

Socket, journal, and browser compatibility advance independently. A socket
admits only the current hello containing wire version, capability, and
resident epoch; mismatch refuses while naming both artifacts. The journal
stores one current schema and performs one forward migration at open before
the resident mints its epoch. The browser is built per worktree into the one
ignored lane stage; an atomic pointer selects one complete immutable
generation, and resident startup refuses unless that generation exists.

The split matches three different replacement events: a client can be stale
while durable data remains current; durable data can require migration while
the command grammar is unchanged; and a lane can rebuild its browser without
recompiling the resident. One version number would obscure these
cases and make a successful comparison meaningless.

The resolution deletes old socket arms, permanent historical upcasters,
browser negotiation, and any inference that a current client authorizes an
unmigrated store. Migration instead has prepare and finalize states, an
external progress witness, a crash drill, and a schema version row inside the
journal.

### The target manifest

The release object is one canonical sealed manifest containing a separate
entry for each target tuple, and the pipeline that produces it is acyclic.
`dist` verifies pins, builds the Wasm app, builds the native canvas, and emits
a receipt-free target payload binding the tuple, native binary digest, app
digest, and toolchain and container
identity. Proof consumes that payload digest and emits a detached receipt —
in the sealed Linux container for `x86_64-unknown-linux-musl`, on the named
native sealed runner for `aarch64-apple-darwin`. A post-proof seal composes
the manifest from both target entries and their receipts, and publication
consumes only that sealed object. Identity and the claim that any copy is
every copy hold within one tuple, never across tuples.

This object makes distribution part of proof rather than an unmeasured step
after it: proof cannot certify a binary or app different from the artifact an
operator copies, and nothing consumes an object that already contains its own
verdict.

The resolution deletes a universal cross-platform digest, an app assembled
after native identity, ordinary builds presented as releases, and proof that
omits its target environment. The supply path is one committed lockfile,
vendored crates inside the proof container, and a network-disabled
empty-cache build.

### Resident life

The resident is the foreground process started by `spice up`. It holds the
writer lock, owns command admission, and remains alive until `spice down`
advances the durable states Seated, Quiescing, Draining, Terminating, Killing,
Verifying, and Dark. `systemd-user` and `launchd` templates are installation
artifacts that may own restart policy outside the process.

Foreground operation gives the process one observable owner and leaves
restarts to operating-system mechanisms already built for that purpose. The
down sequence can preserve unresolved directives, seal tape, bound child
termination, classify residue, and release the lock without confusing
process detachment with successful shutdown.

The resolution deletes self-daemonization, a second startup path, implicit
restart policy, and process exit treated as semantic acknowledgement. There is
one start, one graceful end, and one externally visible state machine between
them.

## The why-Rust ledger

Rust collapses the dependency ledger into the product. The workspace graph
states which crate may know each authority, effect, and vocabulary type;
private constructors keep journal commit inside the bus; the app can depend on
wire types without reaching the journal, filesystem, environment, or process
rim. Cargo features, the committed lockfile, and the target graph become
reviewable inputs rather than ambient packages discovered at execution.

The compiler carries laws that prose alone cannot keep current. Capability
types prevent domains from constructing powers they were not given. Exhaustive
enums close the fact and result vocabularies. Deny-by-default lints and
reasoned `expect` attributes make exceptions visible. Generated checks bind
the clap tree, fact admission, folds, waits, and artifact graph back to the
same compiled surface.

Quality floors are measured against the repository's own history. Ratchets
shrink when the fleet improves and rise only through an operator commit;
mutation outcomes are confirmed before they latch; every metric declares its
files, language, parser, target, and exclusions. Rust does not make those
measurements true by reputation. It makes their subjects enumerable and their
regressions enforceable at the gate.

Distribution collapses into one checked path. `dist` pins tools, builds and
tests the app, embeds the exact assets, builds each native target, and emits
the manifest consumed by proof. The same typed contracts cross CLI, resident,
canvas, and app, while one binary contains the command surface and its browser
surface. The result is not merely a port in Rust; it is an appliance whose
dependency graph, policy checks, and copied artifact can be proved together.

## The tastes as repo truth

The standing preferences that judge work live in [`TASTES.md`](TASTES.md).
That file owns their blunt wording, near-universal word-glob bags, seed set,
evidence aging, and commit-governed curation. Runtime facts record confirmed
and cleared findings; they do not redefine membership or create a second
policy register.
