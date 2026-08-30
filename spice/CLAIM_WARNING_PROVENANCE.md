# Claim warning provenance

This note records the claim-expiry warning path recovered from Python Spice in
the sibling `spice-z` repository and maps each responsibility to its Rust Spice
replacement. It is a forensic record, not a compatibility execution path.
Rust never opens predecessor task state to deliver a warning.

The inspected sources were clean at these revisions:

- Python Spice: `8119859e21eaff5058274473eaa8b56a144eea3a`.
- Rust Spice: `8037bd652e2cb40f2323f51332a8ccb5cef8d404`.

Python citations begin with `spice-z/` and are relative to that pinned sibling
tree. Rust citations are relative to this repository. A citation has the form
`path#anchor`; the anchor is source text or a symbol, never a line number. The
test `spice/tests/claim_warning_provenance.rs#every_current_citation_resolves`
checks Rust citations without requiring a predecessor checkout in CI.

## Outcome

The predecessor combined two concerns in one command-wrapper working-state
meter. Rust separates them by authority:

1. The resident owns a durable warning for a manual claim. It derives a timer
   from journaled claim and lease authority, re-proves the complete generation
   when the timer fires, records `claim::warned`, and publishes steering only to
   the still-matching lane.
2. The command rim owns current-process working-state decoration. It proves one
   vendor identity from the process environment, reads the Rust board, and emits
   tiered stderr notices while a supervised command runs.

Neither path imports Python state, infers a claim from transcript prose, or asks
the agent to poll. There is one current Rust path for each responsibility. The
predecessor remains evidence for semantics, not a release-specific fallback.

```text
Python task export                         Rust journal
ACTIVE + actor + worktree                  task + claim + lease + residency
          |                                           |
          v                                           v
command working-state meter              resident timer     LiveMeter
          |                                  |                 |
          +-- supervisor side channel        |                 +-- command stderr
                    |                        v
                    +--> command stderr   claim::warned --> exact lane inbox
```

## Responsibility map

| Concern | Python owner | Rust owner |
| --- | --- | --- |
| Claim | Active row plus actor and worktree | Journal heads plus residency |
| Clock | `claim_until` minus wall clock | Journal timer and live-board clock |
| Tiers | Strict 600, 300, and 60 seconds | Same meter tiers; resident at expiry minus 299 |
| Dedupe | Persist handle, phase, other state, and tier | Fact generation plus persisted meter key |
| Wait | Side-channel `select` deadline | Cancellable meter worker |
| Renew | Renew, wake, then reread expiry | New lease head re-arms both observations |
| Restart | Recreate streams; read display key | Fold heads; read meter key |
| End or move | No matching active row | Stale timer; no old meter claim |
| Deliver | Command stderr | Exact-lane steering and command stderr |

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
`spice-z/tests/test_agentcli.py#def test_working_state_claim_lease_warnings`.

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
is `spice-z/tests/test_sidechannel.py#def test_side_channel_repeats_critical_claim_warning`.

`spice-z/spice/agent/supervisorwatch.py#def renew_supervised_claim(` performs
best-effort renewal for the supervised thread. Success clears prior failure
state and notifies contract streams; their next collection observes the new
`claim_until`. Completion, unclaim, expiry, actor or worktree movement, and
terminal renewal results remove the matching active observation. A wrapper or
supervisor restart creates new process-local injectors, but the persisted key
continues repeat suppression. Collection and persistence failures decorate
nothing and never change the child command's output or exit status.

## Rust resident warning

### Durable authority and timer

The resident warning authority is defined by
`spice/src/resident/timers.rs#struct WarningAuthority`. Selection at
`spice/src/resident/timers.rs#fn desired_warnings(` admits only an active manual
claim in the seated resident epoch with the current lane occupant and a coherent
lease. It captures the task stamp, positioned claim and lease heads, lane,
occupant, epoch, deadline, and expiry. Automatic claims are excluded because
their resident heartbeat renews a much shorter rolling lease.

`spice/src/resident/timers.rs#fn synchronize_warnings(` cancels process-local
timers whose captured authority no longer equals the desired fold and arms the
new generation. Resident startup runs the same synchronization over durable
heads, so restart is recovery from authority rather than persistence of an old
sleep.

The deadline at `spice/src/resident/timers.rs#fn warning_deadline(` is expiry
minus 299 seconds. When it fires,
`spice/src/resident/timers.rs#fn consume_warning(` rejects an early or expired
event and commits `wire/src/body/task.rs#pub struct ClaimLeaseWarned` under a
journal guard. `spice/src/resident/timers.rs#fn warning_guard(` re-reads the
exact task, claim, lease, residency epoch, lane occupant, horizon, and prior
warning generation atomically. A renewal, reclaim, completion, free, lapse,
movement, or resident replacement makes the old proposal stale.

The fact kind is fixed at `wire/src/fact.rs#ClaimLeaseWarned = "claim::warned" by BusTimer;`.
`spice/src/resident/timers.rs#fn warning_recorded(` deduplicates by task stamp,
claim head, lease head, and expiry. A genuinely new lease generation is
warnable; replaying the same generation is not.

### Exact-lane publication

After commit, `spice/src/resident/timers.rs#pub(super) struct WarningDelivery`
retains the complete generation. The runtime branch at
`spice/src/resident/runtime/surface.rs#timers::Fired::ClaimWarning(delivery) => {`
re-proves current residency before publishing the lane inbox. The steering
projection recognizes the warning fact at
`spice/src/resident/lane_surface/reply.rs#wire::FactKind::ClaimLeaseWarned => {`.

Durable pending steering must remain generation-aware after this immediate
publication check as well. Authority retirement is owned by the resident
warning path, not by this documentation and not by a predecessor fallback. Its
invariant is that restart or an unrelated inbox publication cannot resurrect a
warning whose claim, lease, expiry, lane occupant, or epoch has moved.

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
second repeats. Its persisted key excludes remaining seconds. The scheduler at
`spice/src/session_meter.rs#fn next_refresh(` selects the next tier or repeat
deadline and applies the same 50 millisecond floor.

`spice/src/run_meter.rs#pub(super) struct Runtime` evaluates once before the
child command, emits any announcement to stderr, and starts a cancellable worker
only when another refresh is due. Collection, clock, state-read, or persistence
failure becomes silence. Completion of the child cancels and joins the worker,
so the meter cannot outlive the command it decorates.

## Compatibility boundary

The retained data is Rust journal authority and Rust meter suppression state.
Python task exports, side-channel sockets, working-state JSON, and thread
pointers are never migrated or consulted. If a future release cannot interpret
retained Rust state, it must migrate that state once or refuse while naming the
owning release. It must not add a predecessor reader, a release-specific warning
arm, transcript inference, or a second executable reclaim spelling.
