# Product model — supporting planes — Metrics and hooks

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Metrics

Transcript accounting is stated in the
[metrics register](../invariants/session.md#metrics).

## Hooks

Git gate hooks (`pre-commit`, `commit-msg`, `reference-transaction`) refuse or
allow a repository mutation. Agent shell hooks (the wrapper and the supervisor)
shape and steer the agent's own commands. They have different owners and
different failure semantics, and are never conflated.

Each installed Git hook contains only its shell header and one `exec` of the
repository's installed Spice with the hook name and original arguments. The
launcher owns no environment policy. Native `gate pre-commit` removes Git's
hook-only repository routing, repeated exec-path prefixes, and only the default
developer-root variables Git injected before it captures proof authority or
launches a child.

The wrapper receives each complete top-level command string
[exactly once](../invariants/identity.md#steering-shell-instrumentation-is-one-shot-at-the-agent-s-own-command-boundary).
That is the last point at which the command still exists as one string rather
than a running process, which is why the
[command optimizer](../invariants/session.md#extension-the-command-optimizer-is-optional)
rewrites there.

### Supervised host environment

A lane inherits only admitted host variables. On Windows, its session home is
explicit `HOME`, then `USERPROFILE`, then the complete `HOMEDRIVE`/`HOMEPATH`
pair. Its temporary root is explicit `TMPDIR`, then `TEMP`, then `TMP`.
The supervisor exports the resolved home as both `HOME` and `USERPROFILE`,
and the temporary root as `TMPDIR`, `TEMP`, and `TMP`, so Spice's pinned
session and OMP's native Windows discovery agree. Session discovery outside
the occupant uses the same resolver. Missing home or temp sources refuse.

Explicit `SHELL` selects the native interpreter behind interception. Without
it on Windows, the supervisor uses installer and gate Git selection—`PATH`
first, then the warned Program Files default—and locates `bin/bash.exe` beside
that exact `cmd/git.exe`; both must be regular files. Discovery inspects files
and the search path but never executes Git or substitutes `ComSpec` or
PowerShell for a POSIX interpreter. Windows also admits `HOMEDRIVE`,
`HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `SystemRoot`, `PATHEXT`, and `ComSpec`
for the vendor and its tools; unrelated variables remain excluded. Unix home,
temp, shell, and ambient admission rules are unchanged.

Session publication and discovery share OMP's directory encoder. On Windows,
temp scope precedes home scope when the temporary directory nests beneath the
profile. Absolute directory names omit Rust's verbatim-path namespace marker
and retain OMP's ordinary drive or UNC spelling.

Supervisor change notifications prompt a durable-state read; they are not
startup authority. At the start horizon the resident reads `state.json` again,
so an unannounced published failure or proven start wins over a timeout. A
genuine timeout names the supervisor directory containing `state.json` and
`supervisor.log`; directive recovery and dead-letter policy are unchanged.

### Shell results

Advice is read-only. A leading `cd` to the literal absolute directory the
command already runs in, and a final `head` or `tail` pipeline stage, each earn
one hint on that command's initial result. Advice never changes the command, its
permission decision, its output, or its exit status, and carries no
acknowledgment or reminder.

Shell and job results show a bounded preview. When bytes are omitted the result
names the complete keeper-owned log, which retains the command's original
captured bytes; `read` it with a line range.

Fresh steering published after an OMP step starts makes that step stale. A shell
or blocking `job wait` emitted by the stale step is registered and immediately
released through its ordinary job result. The first released boundary replays
the complete current unanswered inbox; concurrent calls from the same stale
generation do not repeat it. Another fresh publication permits one new replay.
A later OMP step clears the marker; delivery-cache movement does not. Atomic job
operations and work from the later step keep their normal foreground behavior.

### Active-work narration

The resident keeps one narration streak per occupant. Assistant prose resets it;
tool results, an outstanding command, and `job wait` do not. The first reminder
is due at the configured tool-call bound (`lane.narration`) or three minutes,
and the second at the next call bound or five minutes total. Further reminders
are call-bound only. Each due reminder records `narration::prodded`, interrupts
the active host-tool boundary so healthy work continues as a kept job, and
enters steering immediately. No acknowledgment key is minted, and a moved or
exited occupant never receives a successor's prod.

### Cadence is reachability

[Pending steering](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary)
reaches non-shell work because an unheard key interrupts outstanding tool waits.

An OMP turn that settles with an exact active claim leaves its occupant idle;
without a claim it exits and vacates the lane. A backgrounded command survives
under its detached keeper; a command whose keeper cannot launch fails its call
and leaves no kept job. If its ending, or an event of a command started with
`events`, stays unheard through the delivery grace, the keeper wakes the
matching retained occupant when its claim remains admitted, or requests a lane
wake for a successor. `job wait` is for a finite job whose result blocks
current work. While it blocks, the lane's current activity remains that wait:
the exact job and its latest bounded output replace any earlier tool as the
operator-visible status. That output refreshes within a few seconds of the job
printing, at a bounded rate however fast it prints, and the wait is restated
whenever another tool call settles while it blocks. The job's final result
alone settles the tool call.

A [service](../invariants/lifecycle.md#lifecycle-services-belong-to-one-claim-win)
never ends on its own and never keeps a lane idle; `job wait` answers at once
with its output so far. A task-associated service follows its exact claim win;
an unanchored service stops with the occupant turn that launched it. Every seat
start names the lane's kept commands that still run, marking services, so a
forgotten task service surfaces to the next seat.

A job's ending is told once. An answer that reports how a job ended records it
as heard, so no completion notice follows and no successor repeats it. The
record is claimed atomically before a notice is written, so a racing answer and
notice deliver exactly one; a notice that fails to write gives the claim back.

### Acknowledgment corrections

[Assistant text on the lane tape](../invariants/identity.md#steering-acknowledgment-evidence-is-assistant-prose-on-the-lane-tape)
is the acknowledgment evidence: visible prose, and the declared intent of every
tool call in the same message. Reasoning, tool arguments and quoted
examples do not qualify. `ACK KEY [KEY ...]: response` can retire several keys;
`NAK KEY: reason` declines one. Recognition happens on landing, not at turn end.

Every reminder of an already-delivered directive replaces its body with a
correction, even when the original is shorter. It names the pending key, says
whether replies followed, and supplies the same grammar as reasoning-only,
wrong-key and compaction corrections. First deliveries remain complete.

Once an ordinary, reasoning-only, wrong-key or compaction correction actually
lands—not merely enters the aside queue—the first later complete visible
assistant response that leaves its key pending queues an activity correction.
Newly due keys join any older keys still queued. Further ignored responses
repeat it every third complete response. This cadence
has no clock: silence, a blocked or tool-only call, and streamed fragments do
not advance it, and its queued aside does not interrupt a running tool. An ACK
in visible prose or declared tool intent removes its keys from the queued
correction before journal catch-up; partial ACKs leave the other keys named.
Compaction and occupant replacement start a new activity window.

A wrong-key correction names unrecognized tokens and lists pending keys, once
per compaction. Delivered keys stay recognized for that occupant's lifetime,
so repeating a previously acknowledged key causes no false correction.
