# Python study-to-gate ratchet provenance

This note records the ratchet lifecycle recovered from the Python implementation
in `spice-z` and maps it to the Rust gate. It is a forensic handoff, not an
implementation change.

The sources were clean at these revisions when inspected:

- Python: sibling repository `../spice-z` at
  `00c7d6c13c382c6a73e36dc562f6722c96cd46bc`.
- Rust: this repository at
  `1bcbaad986db5d48154792793405df16f00ea6b1`.

Python paths below are relative to the pinned `spice-z` root. Rust paths are
relative to this repository; each section names the implementation it cites.

## Outcome

The Python promotion seam was not a second, cheaper implementation of each
study. The study and pre-commit gate reused the same policy and usually the same
scanner, while their callers supplied different jurisdictions and persistence
rights:

```text
explicit study                         pre-commit
tracked / diff / named paths           one captured staged-path list
        |                                      |
        +---------- shared scanner ------------+
                           |
                 baseline or sticky policy
                           |
             read-only result       pending state
                                      |
                           persist only if all gates pass
```

A repository-wide study established the need and exposed existing debt. Once
the study reached zero, or an explicit ratchet baseline was chosen, the same
check could become a hyper-local gate only if staged paths were a sound
projection of its impact. Expensive analyses without that seam remained
explicit study/doctor work.

Rust retains the correct pure, fail-closed selection model and has stronger
baseline/state types. Its remaining mismatch is downstream: exact changed paths
select policy arms, but selected structural metric arms still acquire the whole
staged tree. Localizing acquisition must preserve prior latch healing and must
not turn a partial observation set into a supposedly complete state inventory.

## Authoritative Python control flow

### One path capture feeds the hook

`spice/hooks/precommit.py:162-187` captures `staged_paths(repo_root)` once,
constructs the normal steps from that exact list, and gives the same list to
post-success steps. It accumulates failures instead of changing the source
inventory between arms.

`spice/studies/walk.py:162-184` implements the capture with:

```text
git diff --cached --find-renames --name-only --diff-filter=ACMR
```

The result is a finite list of added, copied, modified, and renamed paths. The
pre-commit selection path does not walk the repository.

The structural gate adapters keep that boundary:

- `spice/hooks/precommit.py:1210-1243` passes only the captured paths to the
  file-shape ratchet.
- `spice/hooks/precommit.py:1278-1300` passes only the captured paths to the
  complexity ratchet.
- `spice/hooks/precommit.py:1303-1319` passes only the captured paths to the
  magic-number ratchet.

### Studies own broad discovery

`spice/studies/cli.py:489-510` defines the study-side jurisdiction:

- `--staged` selects staged paths.
- Explicit path arguments select exactly those paths.
- `--baseline-ref` selects paths changed from that ref where supported.
- With no narrowing option, the study selects tracked repository paths.

File-shape dispatch at `spice/studies/cli.py:607-659`, complexity dispatch at
`spice/studies/cli.py:662-699`, and magic dispatch at
`spice/studies/cli.py:742-773` all preserve that caller-owned choice. This is
the crucial lifecycle boundary: broad discovery is an explicit study action;
the hook supplies a previously captured local change surface.

### State is accepted with the commit, not with a scan

`spice/hooks/precommit.py:169-187` wraps the complete normal and post-success
hook in `gates.deferred_sticky_writes()`. A scan can compute an in-memory latch
and use it in the current verdict, but `pending_sticky.commit()` runs only after
every step passes. A rejected commit leaves durable ledgers unchanged.

`spice/studies/gates.py:170-215` implements that buffer. It is transaction-like
in intent, although individual Python ledger writes and tolerated I/O errors are
weaker than the Rust state transaction.

## How inherited debt differs from a changed-surface regression

Python uses three complementary mechanisms. They should not be flattened into
one generic baseline rule.

### Zero-first gates

Some scanners can be run repository-wide as studies until the result is zero.
After that cleanup, scanning only staged files is sufficient to reject a newly
introduced violation. No standing per-finding baseline is necessary because
the accepted repository is the zero baseline.

This is the simplest promotion path, but it is sound only when a violation is a
property of an independently scannable changed file. Cross-file or global
properties need a different authority.

### Identity baselines for magic numbers

`spice/studies/magicnums.py:564-616` scans only the paths supplied by its
caller. For each current file it reads the same path from the configured
baseline ref and compares finding identities. The effective inherited-debt
identity is `(path, literal)`: line numbers are diagnostic location, not
baseline membership. A literal already present anywhere in that file is
inherited debt; a current literal absent from the baseline file is a regression.

If the baseline blob does not exist, the baseline finding set is empty. A new
file therefore fails for every current magic finding rather than being skipped.
This is local and fail-closed: unknown history for one changed file expands the
verdict for that file, not the search jurisdiction for the repository.

### Sticky flex latches for bounded metrics

`spice/studies/gates.py:66-76` applies the flex limit to an unlatched key and
the base limit to a latched key. Crossing the flex limit creates a breach;
remaining above base holds an existing latch; returning to base or below heals
it.

The standing ledger does not force every hook run to rescan the repository:

- `spice/studies/gates.py:218-237` remaps loaded keys across renames, intersects
  them with still-live keys, and unions current flex breaches.
- `spice/studies/fileloc.py:194-225` remeasures only paths named by existing
  sticky entries so unchanged debt can heal or remain held.
- `spice/studies/fileloc.py:339-447` derives new breach candidates and blocking
  findings only from the caller's paths, while consulting that finite prior
  latch set.
- `spice/studies/complexity.py:275-356` does the same with `(path, routine)`
  keys; `spice/studies/complexity.py:442-471` limits healing remeasurement to
  paths named by loaded keys.
- `spice/studies/fileloc.py:228-245` keeps study scans read-only by default;
  the pre-commit adapter explicitly requests persistence.

The resulting jurisdiction is `changed keys + previously latched keys`, not
`all repository keys`. That finite union is the mechanism that distinguishes a
changed regression from inherited global debt without a repository walk.

Peer ownership is a verdict concern, not a discovery expansion.
`spice/hooks/precommit.py:1254-1275` renders peer-held flex-slice claims as
informational redirects and fails only unclaimed blocking findings.

## Unknown impact without repository search

The Python hook's staged-path capture is local, but Python does not contain a
general compiled classifier that maps an unknown path class to an exhaustive
battery. Unsupported suffixes commonly no-op. That is a gap in the Python
design, not behavior for the Rust port to preserve.

Rust already supplies the stronger rule in `gate/src/change_selection.rs`:

- `select_change_gates` is pure and has no repository parameter.
- It sorts and deduplicates exact changed paths.
- Known path classes select their canonical arms.
- Policy authority paths and unknown or unsupported roots add an escalation.
- Any escalation selects the complete canonical battery.

The selector's own documentation at `gate/src/change_selection.rs:218-222`
states the invariant directly: an unprovable path class produces a complete
battery plan rather than omission or success with unknown evidence. The tests
named `policy_authority_and_unknown_paths_escalate_to_every_battery_arm`
demonstrate that behavior.

This is the desired interpretation of fail-closed selection. Unknown impact
widens the finite gate plan; it does not make the selector discover the whole
repository. An exhaustive selected arm may use a repository-wide authority,
but that work is an explicit result of escalation, outside the selector.

## Evidence for study-to-ratchet promotion

The available Git history begins with a fairly mature dual-surface design, so
it does not contain the original study measurements or cleanup campaign. The
operator record supplies the promotion rule: checks began as studies; after a
study reached zero, or after an explicit ratchet baseline was chosen, a check
could become a pre-commit gate once it had a hyper-local impact projection. The
code and later commits show how that rule was hardened:

| Revision | Date | Recovered step |
| --- | --- | --- |
| `8407dfa2` | 2026-06-12 | Initial available tree already exposes tracked/staged study modes, staged pre-commit paths, and the magic baseline comparison. |
| `34cf96d7` | 2026-06-25 | Makes flex/sticky study scans pure unless persistence is explicitly requested. |
| `4c227f17` | 2026-07-02 | Adds actor/path flex jitter without broadening scan jurisdiction. |
| `d17f974a` | 2026-07-02 | Adds peer flex-slice claims so parallel work does not trigger duplicate repair. |
| `4cbf1766` | 2026-07-07 | Adds file-shape latch self-healing and informational peer findings. |
| `d7e1d496` | 2026-07-07 | Extends self-healing behavior to complexity and repository documents. |
| `5975b5c0` | 2026-07-15 | Adds the standing mutation ratchet to doctor, explicitly outside the hook. |
| `f00d8fbd` | 2026-07-17 | Extracts the shared bounded-gate kernel. |
| `242fd38c` | 2026-07-26 | Defers sticky persistence until the whole commit is accepted. |

The mutation study is the negative case that clarifies the boundary.
`spice/studies/mutations.py:112-166` identifies changed Python modules, creates
a disposable checkout, establishes a baseline test result, and runs mutation
reports. `spice/studies/mutations.py:169-185` maintains its standing ratchet,
and `spice/studies/mutations.py:533-565` rejects score decreases against the
per-module baseline. `docs/overview.md:105-121` deliberately places it under
`spice dev doctor`, not pre-commit, to keep commit feedback fast. The versioned
`tests/mutation-ratchet.json` is study/doctor standing state, not evidence that
the mutation search became hook-local.

Therefore the proven promotion sequence is:

1. Run the broad study to establish the problem and inventory existing debt.
2. Reach zero, or deliberately choose a baseline identity/standing latch.
3. Prove a local projection: changed paths alone for zero-first and identity
   checks, or changed keys plus the finite prior latch set for sticky bounds.
4. Reuse the scanner and policy with the hook's one staged-path capture.
5. Keep study mode broad and read-only by default.
6. Buffer proposed state and publish it only after the full commit gate passes.
7. Leave analyses with no sound and fast local projection in explicit
   study/doctor/CI surfaces.

## Rust port map

### Retain exactly

| Mechanism | Rust authority | Required retained behavior |
| --- | --- | --- |
| Single immutable staged source | `spice/src/change_gate.rs:223-342` | Prepare the staging arm once; feed its exact changes and digest into test and policy selection; refuse source movement. |
| Pure fail-closed policy selection | `gate/src/change_selection.rs` | Keep selection repository-free; unknown/authority paths select every canonical arm. |
| Local versus exhaustive test reach | `spice/src/change_gate.rs:272-310` | Continue selecting local workspace tests from exact changes and exhaustive reach only after escalation. |
| Merge-aware baseline authority | `gate/src/metrics/baseline.rs` | Keep capture of parents, merge base, and clean-merge authorship; it is stronger than the Python single-ref convention. |
| Typed ratchet policy | `gate/src/ratchet.rs`, `baseline.toml` | Keep shrink-only global floors and typed evidence. The only current standing floor is `rust.mutation-survivors = 0`; do not replace this authority with ad hoc Python JSON. |
| Typed state and claims | `gate/src/state` | Keep explicit bounds, latches, claims, and accept/reject publication semantics. |
| Durable full-study repair | canonical battery reporting | Preserve repository-wide repair generation on full runs; local gate selection must not erase that confidence surface. |

### Missing behavior to implement

1. **Bind exact paths to structural acquisition.**
   `spice/src/change_gate.rs:233-303` sends exact changes into selection, but
   `gate/src/battery/repository_executor.rs:148-236` captures focused arms by
   `GateId` alone. Its metric arm calls `MetricSource::capture`, and
   `gate/src/metrics/acquire.rs:294,482,502,647,687` iterates the full
   `source.measurable()` inventory. The policy plan is local; structural
   acquisition is not. A local arm needs a digest-bound candidate-path set.

2. **Preserve a separate exhaustive study path.**
   Narrowing the local arm must not narrow full battery/verify collection.
   Global repair findings and audits still need the complete selected tree.
   The existing throughput audit measured metric capture at 8.252 seconds and
   a full report at roughly 23 MB (`gate/THROUGHPUT_AUDIT.md:38-46`), which is
   direct evidence that reusing full acquisition in pre-commit is material.

3. **Fail closed for new-file magic evidence.**
   `gate/src/metrics/acquire.rs:873-937` currently returns
   `EvidenceState::NotApplicable` when no committed baseline contains the file.
   Python instead compares against an empty finding set. The Rust contract must
   either measure every new-file finding as new or explicitly escalate/refuse;
   it must not admit the path as not applicable.

4. **Production state publication (implemented).**
   `gate/src/battery/state_run.rs` now binds focused captures to one fixed-clock
   `CurrentState`, accumulates their exact observations, and publishes through
   the typed transaction only at the enclosing accepted boundary.

5. **Partial reconciliation (implemented).**
   `CurrentState::reconcile_partial` updates the supplied sticky keys while
   preserving unrelated live entries; exhaustive reconciliation retains its
   complete-inventory pruning contract. Production regressions cover both
   preservation and accepted latch creation, hold, and healing.

6. **Classify inherently global arms honestly.**
   Repository-document target inventory, reachability, and similar relational
   policies may require a cached/global authority or explicit exhaustive
   escalation. Do not label them local merely because their arm was selected by
   one changed path.

### Do not copy

- Python AST, lizard, tree-sitter subprocess, and language-runtime adapters;
  Rust already owns native or vendored parsers and typed evidence.
- Per-family untyped JSON ledgers and `ContextVar` buffering. Wire the Rust
  state transaction instead.
- Python mutation scratch-checkout and `PYTHONPYCACHEPREFIX` mechanics. Rust's
  native kill system and `baseline.toml` are the current authorities.
- `tests/mutation-ratchet.json` as a pre-commit mechanism. It intentionally
  belongs to the Python doctor workflow.
- Python peer-claim serialization details. Retain coordination semantics through
  Rust's typed claims.
- Silent ledger load/write tolerance such as caught-and-ignored I/O errors.
  Rust publication failures must remain observable refusals.
- Python's silent unsupported-suffix behavior. Rust's exhaustive escalation is
  the stronger invariant.

## Implementation contract

A future implementation can be considered faithful only when all of these are
true:

1. The staging snapshot and exact normalized changed paths are captured once.
2. Selection remains a pure mapping from that evidence to canonical arms.
3. Local structural arms receive a digest-bound candidate set and do not walk
   unrelated source classes.
4. Sticky checks add the finite prior-latch jurisdiction needed for correct
   hold/heal decisions.
5. A partial run cannot prune state it did not observe.
6. New files and unsupported classes fail closed by measurement, refusal, or
   explicit exhaustive escalation.
7. Full battery/study execution retains repository-wide acquisition and durable
   global repair evidence.
8. No proposed state becomes durable unless all selected policy and test arms
   accept the same staged source.
9. Benchmarks demonstrate hook-local cost by changed surface, while full-study
   cost remains visible and separately measured.

## Evidence boundary

The available Python repository proves the settled control flow, baseline and
latch identities, locality mechanism, persistence boundary, and the later
hardening sequence. It does not preserve the pre-history measurements by which
each original study was cleaned to zero. Claims about that earliest promotion
step come from the task's operator record and are intentionally distinguished
from code and Git provenance above.
