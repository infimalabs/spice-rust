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
it on Windows, the supervisor locates `bin/bash.exe` beside the installer's
admitted Program Files Git `cmd/git.exe`; both must be regular files.
Discovery inspects files, never executes Git or searches `PATH`, and never
substitutes `ComSpec` or PowerShell for a POSIX interpreter. Windows also admits
`HOMEDRIVE`, `HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `SystemRoot`, `PATHEXT`, and
`ComSpec` for the vendor and its tools; unrelated variables remain excluded.
Unix home, temp, shell, and ambient admission rules are unchanged.

Session publication and discovery share OMP's directory encoder. On Windows,
temp scope precedes home scope when the temporary directory nests beneath the
profile. Absolute directory names omit Rust's verbatim-path namespace marker
and retain OMP's ordinary drive or UNC spelling.

### Shell results

Advice is read-only. A leading `cd` to the literal absolute directory the
command already runs in, and a final `head` or `tail` pipeline stage, each earn
one hint on that command's initial result. Advice never changes the command, its
permission decision, its output, or its exit status, and carries no
acknowledgment or reminder.

Shell and job results show a bounded preview. When bytes are omitted the result
names the complete keeper-owned log, which retains the command's original
captured bytes; `read` it with a line range.

### Active-work narration

The resident keeps one narration streak per occupant. Assistant prose resets it;
tool results, an outstanding command, and `job wait` do not. When the streak
reaches its tool-call bound (`lane.narration`) or its time deadline, the
resident records `narration::prodded` and the supervisor interjects a progress
notice that interrupts outstanding tool waits, never their commands. No
acknowledgment key is minted, and a moved or exited occupant never receives a
successor's prod.

### Cadence is reachability

[Pending steering](../invariants/identity.md#steering-pending-steering-reaches-each-admitted-agent-boundary)
reaches non-shell work because an unheard key interrupts outstanding tool waits.

Every turn exit vacates the lane, including exit code zero. A backgrounded
command survives under its detached keeper; a command whose keeper cannot
launch fails its call and leaves no kept job. If its ending, or an event of a
command started with `events`, stays unheard through the delivery grace, the
keeper requests a lane wake, and the successor's startup catch-up delivers the
unheard events and then the ending. An agent holding a claim may therefore end
its turn while a deadline-bounded job runs; it is woken with its claim when the
job ends or emits an event. `job wait` is for a finite job whose result blocks
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

[Assistant prose on the lane tape](../invariants/identity.md#steering-acknowledgment-evidence-is-assistant-prose-on-the-lane-tape)
is the only acknowledgment evidence. Within one prose the placed header decides
its key; a later mention in backticks or mid-sentence is talk about the header.
Prose that only quotes or buries a live key is refused as `steer::ack-missed`
with the repair: lead a line with the header.

A placed header naming a token the occupant does not hold retires nothing. The
supervisor privately names that token, says it acknowledged nothing, and lists
the pending keys, once per compaction. A key successfully delivered to an
occupant stays recognized for that occupant's lifetime, so a closing reply may
repeat its header without a false correction.
