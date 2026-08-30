# Claim warning provenance

This note records the claim-expiry warning path recovered from Python Spice in
the sibling `spice-z` repository and maps each responsibility to its Rust Spice
replacement. It is a forensic record, not a compatibility execution path.
Rust never opens predecessor task state to deliver a warning.

The inspected sources were clean at these revisions:

- Python Spice: `8119859e21eaff5058274473eaa8b56a144eea3a`.
- Rust Spice: `a640d7cf637763c65e40ae33043c6b9569994fa4`.

Python citations begin with `spice-z/` and are relative to that pinned sibling
tree. Rust citations are relative to this repository. A citation has the form
`path#anchor`; the anchor is source text or a symbol, never a line number. The
test `spice/tests/claim_warning_provenance.rs#every_current_citation_resolves`
checks Rust citations without requiring a predecessor checkout in CI.

## Outcome

The predecessor combined claim observation and warning delivery in one
command-wrapper working-state meter. Rust keeps the same ownership boundary:
the command rim proves one vendor identity from the process environment, reads
the Rust board, and emits tiered stderr hook feedback while a supervised command
runs. The resident owns claim lapse and automatic renewal deadlines, but it does
not create warning facts or steering.

This path does not import Python state, infer a claim from transcript prose,
write an inbox, or ask the agent to acknowledge a warning. The predecessor
remains evidence for semantics, not a release-specific fallback.

```text
Python task export                         Rust journal
ACTIVE + actor + worktree                  task + claim + lease + residency
          |                                           |
          v                                           v
command working-state meter                         LiveMeter
          |                                           |
          +-- supervisor side channel                 +-- command stderr
                    |
                    +--> command stderr
```

## Responsibility map

| Concern | Python owner | Rust owner |
| --- | --- | --- |
| Claim | Active row plus actor and worktree | Journal heads plus residency |
| Clock | `claim_until` minus wall clock | Live-board lease minus wall clock |
| Tiers | Strict 600, 300, and 60 seconds | Same strict meter tiers |
| Dedupe | Persist handle, phase, other state, and tier | Persist current working-state key and tier |
| Wait | Side-channel `select` deadline | Cancellable meter worker |
| Renew | Renew, wake, then reread expiry | Reread the current lease at the next evaluation |
| Restart | Recreate streams; read display key | Recreate worker; read meter key |
| End or move | No matching active row | No current meter claim; one deduplicated loss notice |
| Deliver | Command stderr | Command stderr hook feedback only |

## Python causal path

### Observation and clock

Every agent shell command enters the wrapper described at
`spice-z/spice/agent/wrap.py#def run_agent_command(`. The working-state collector
at `spice-z/spice/agent/wrap.py#def collect_working_state_snapshot(` delegates
claim selection to `spice-z/spice/agent/wrap.py#def _working_state_claim(`.

That selector obtains the ambient vendor thread, exports `+ACTIVE`, and retains
only rows whose `claim_by` equals that thread and whose `claim_worktree` resolves
to the command's repository root. If several rows survive, the newest
`claim_at` or `start` wins. It parses `claim_at` and `claim_until`, subtracts the
wall clock, and clamps elapsed and remaining seconds at zero. Missing identity,
failed export, no active row, actor mismatch, worktree mismatch, or malformed
timestamps yield an empty or partially timed observation instead of authority
manufactured from transcript text.

### Thresholds and rendering

`spice-z/spice/agent/wrap.py#def _claim_lease_warning_level(` applies strict
boundaries. At 600 seconds there is no warning; 599 enters `WARNING`; 299 enters
`URGENT`; and 59 enters `CRITICAL`. The repeat cadence comes from
`spice-z/spice/agent/wrap.py#def _claim_lease_warning_repeat_seconds(`: 120, 60,
and 15 seconds respectively.

`spice-z/spice/agent/wrap.py#def render_working_state_snapshot(` appends the
tier, remaining seconds, and the exact action
`spice task reclaim HANDLE` to one stderr working-state sentence. The tier and
message shape are pinned by
`spice-z/tests/test_agentcli.py#def test_working_state_claim_lease_warnings_cross_thresholds_and_repeat`.

### Suppression and deadline wake-up

`spice-z/spice/agent/wrap.py#class AgentWorkingStateInjector` keeps a process
display key and also reads the durable key written by
`spice-z/spice/agent/wrap.py#def write_working_state_state(`. The key built by
`spice-z/spice/agent/wrap.py#def working_state_key(` includes inbox count,
claim handle, phase, last maxim, and warning tier. Remaining seconds are not
identity. A same-tier renewal therefore stays suppressed until its cadence,
while a tier transition is immediately visible.

The injector estimates remaining time between board reads. Its next deadline is
the earlier of a tier boundary and a repeat. The boundary calculation at
`spice-z/spice/agent/wrap.py#def _seconds_until_claim_lease_boundary(` targets
599, 299, and 59 seconds and enforces a 50 millisecond minimum, preventing a
zero-delay loop.

### Side-channel delivery and renewal

`spice-z/spice/agent/sidechannel.py#class AgentSideChannelServer` creates one
injector per connected command. It emits once after the hello, emits again when
claim or inbox activity wakes the socket pair, and passes the injector's next
deadline to `select`. A quiet long-running command therefore receives tier
crossings and repeats without running another shell command. The focused proof
is
`spice-z/tests/test_sidechannel.py#def test_side_channel_repeats_critical_claim_warning_on_lease_timer`.

`spice-z/spice/agent/supervisorwatch.py#def renew_supervised_claim(` performs
best-effort renewal for the supervised thread. Success clears prior failure
state and notifies contract streams; their next collection observes the new
`claim_until`. Completion, unclaim, expiry, actor or worktree movement, and
terminal renewal results remove the matching active observation. A wrapper or
supervisor restart creates new process-local injectors, but the persisted key
continues repeat suppression.

Failure ordering is narrower than a blanket failure-to-silence rule. Snapshot
collection is attempted first and its exceptions are caught by
`spice-z/spice/agent/wrap.py#class AgentWorkingStateInjector`, before any current
working-state row is rendered. The suppression check then calls
`spice-z/spice/agent/wrap.py#def read_working_state_state(`: a missing file or
malformed JSON is an empty key and permits normal delivery, while another read
error escapes before the current row is written. After a successful read,
rendering writes and flushes stderr before
`spice-z/spice/agent/wrap.py#def write_working_state_state(` runs; the injector
also updates its process-local displayed key and timestamp before that durable
write. A persisted-key write failure can therefore occur after streaming output
is already visible and local suppression has advanced. The side-channel server
catches that `OSError` and closes that delivery stream. These meter failures do
not replace the supervised child command's output or exit status, but a
state-write failure is not retroactive silence.

## Legacy Rust warning facts

Rust journals may retain warning facts created by earlier releases. Their wire
shape remains defined at
`wire/src/body/task.rs#pub struct ClaimLeaseWarned`, and the fixed vocabulary
remains at
`wire/src/fact.rs#ClaimLeaseWarned = "claim::warned" by BusTimer;` so those
journals remain readable. The steering ledger and lane reply fold may still
interpret such retained facts, but no current timer schedules or appends one.

The focused resident proof at
`spice/src/resident/timers/tests/mod.rs#fn manual_claims_never_create_resident_warning_protocol()`
synchronizes and renews a manual claim, then verifies that no warning fact,
steering head, or pending lane inbox is created. This preserves forward reading
without keeping the old implementation as an executable arm.

## Rust command meter

`spice/src/session_meter/live.rs#pub struct LiveMeter` is the read-only current
command collector. `spice/src/session_meter/live.rs#pub fn open(` accepts only
one non-empty vendor identity captured at the binary rim. It discovers the Rust
repository and Rust journal; it has no predecessor database input.

`spice/src/session_meter/live.rs#fn collect(` joins that process identity to one
seated lane session and then reads only the lane's active claim and coherent
lease. Ambiguous identities, missing residency, a moved occupant, an inactive
claim, or an epoch mismatch produce no current claim. The live collector also
persists the last exact authority key so ownership movement can produce one
deduplicated `CLAIM LOST` notice rather than continuing to describe the old
claim.

The pure policy in `spice/src/session_meter.rs#enum LeaseTier` preserves the
predecessor's strict 600, 300, and 60 second boundaries and 120, 60, and 15
second repeats. `spice/src/session_meter.rs#pub struct Meter` persists the
announcement key, and
`spice/src/session_meter.rs#pub fn evaluate(&self, snapshot: &WorkingState, now: Duration)`
suppresses same-tier repeats while allowing tier transitions immediately. The
key excludes remaining seconds. `spice/src/session_meter.rs#fn render(` emits
the tier, live remaining seconds, and exact reclaim command. The scheduler at
`spice/src/session_meter.rs#fn next_refresh(` selects the next tier or repeat
deadline and applies the same 50 millisecond floor.

`spice/src/run_meter.rs#pub(super) struct Runtime` evaluates once before the
child command, emits any announcement to stderr, and starts a cancellable worker
only when another refresh is due. Collection, clock, state-read, or persistence
failure becomes silence. Completion of the child cancels and joins the worker,
so the meter cannot outlive the command it decorates.

## Compatibility boundary

The retained data is Rust journal authority, including readable legacy warning
facts, and Rust meter suppression state. Legacy facts are not a current warning
producer.

Python task exports, side-channel sockets, working-state JSON, and thread
pointers are never migrated or consulted. If a future release cannot interpret
retained Rust state, it must migrate that state once or refuse while naming the
owning release. It must not add a predecessor reader, a release-specific warning
arm, transcript inference, or a second executable reclaim spelling.
