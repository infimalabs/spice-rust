# Gate throughput audit

Task: `THROUGH-1kKR1m2q`

This is a historical evidence report and removal/deferral proposal. It changes
no gate. The governing operator direction at the time was steering `1kKR1gmK`:
focus on speed and throughput, identify overzealous checks, and harden and
polish after cutover. The later load warning `1kKRGGHk` specifically called out
repetitive work in high-volume hooks.

## Current disposition (2026-08-11)

The measurements remain useful; the cutover recommendations are superseded and
authorize no present removal. The current local hook selects one deterministic
affected battery, while authority or unknown paths select all 27 canonical
arms. Full verification executes those 27 arms once and retains one compact,
source-bound success summary. `README.md`, `DEVELOPMENT.md`, and
`SHAPE_CONFORMANCE.md` own those current paths.

Installation is reversible coexistence preparation. Only an exact,
operator-authorized authority-switch plan changes the bare command and all
three common hooks. Retiring the predecessor is a separate decision; indefinite
dual execution is not a qualified runtime mode.

## Measurement method

Measurements were taken on 2026-08-06 at repository commit `44f505a`, on an
Apple M4 Pro running arm64 macOS 26.5.2. `/usr/bin/time -p` measured the seven
canonical `cargo xtask verify` actions. Temporary `Instant` probes measured the
27 compiled battery arms and their fused acquisition seams; the probes were
then removed and the clean battery was repeated. The measured staged index was
identical to `HEAD`, so changed-path scanners show their clean no-op floor.
Cargo caches were warm except where the named command required a different
target or test artifact.

The battery eagerly acquires evidence by family. A row timer therefore cannot
honestly own all of its setup. The tables report direct arm time, while these
shared observations remain separate:

| Shared battery work | Wall time |
| --- | ---: |
| Materialize staged index | 694.743 ms |
| Load tracked contract | 9.366 ms |
| Core capture total | 884.724 ms |
| Core direct rows within that total | 52.986 ms |
| Core shared walk, digest, and baselines | 831.738 ms |
| Metrics capture total | 8.252 s |
| Toolchain family total | 61.861 s |
| Hygiene family total | 1.938 s |
| End-to-end battery, instrumented | 77.79 s |
| End-to-end battery, probes removed | **78.77 s** |

The direct family totals account for about 73.65 seconds. Roughly 4.14 seconds
remain in orchestration, validation, and serialization. The emitted report was
approximately 23 MB in this run, so success-path report construction and output
are themselves material hook work.

## 2026-08-09 canonical-hook regression

Task `THROUGH-1kLJJQ8K` profiled the installed Rust pre-commit command itself:
`cargo run --locked --package spice -- gate pre-commit`. The runner had 12
physical/logical cores and 10 active lanes. Every complete experiment used
`/usr/bin/time -p`; aggregate CPU below is user plus system time. The historical
one-lane arm profile above remains the stage-level baseline: the 78.77-second
battery was dominated by `rust-typecheck` at 22.662 seconds,
`canvas-web-typecheck` at 35.773 seconds, metrics capture at 8.252 seconds, and
about 4.14 seconds of orchestration, validation, and report serialization.

| Run | Concurrent allocation at launch | Wall | Aggregate CPU | Result |
| --- | --- | ---: | ---: | --- |
| Before, direct canonical gate | Four gates across the 12-lane runner | 195.61 s | 122.68 s | Clean battery |
| After, cold persistent native cache | Fourth gate; 10 active lanes | 131.67 s | 118.15 s | All compiler arms passed; unrelated `file-shape` refusal |
| After, warm diagnostic rendering | Two peer gates and three Cargo processes | 80.08 s | 96.86 s | Native arm passed in 15.990 s; unrelated `file-shape` refusal |
| After, warm repeat | Two peer gates and two Cargo processes | 131.94 s | 103.13 s | All compiler arms passed; unrelated `file-shape` refusal |
| After, ordinary-target reuse | One peer gate and three Cargo processes | 131.30 s | 108.96 s | All compiler arms passed; unrelated `file-shape` refusal |
| Clean exact gate, cold launcher | One peer gate and one Cargo process | 164.42 s | 110.64 s | Clean battery |
| Clean exact gate, warm repeat | One peer gate and one Cargo process | **104.45 s** | 108.12 s | Clean battery |
| Before stable source cache, high load | Four peer gates and seven Cargo processes | 196.10 s | 120.08 s | Clean battery, but over the unchanged 180-second limit |
| Stable source cache, queue-bound | One peer gate and two Cargo processes | 166.58 s | 83.03 s | Clean battery; native arm passed in 0.864 s |
| Stable source cache, immediate repeat | One peer gate and no Cargo processes | **99.23 s** | 75.11 s | Clean battery; native arm passed in 0.551 s |

The first persistent-output repair reduced the original four-gate wall time by
63.94 seconds, or 32.7%, while aggregate CPU changed by only 4.53 seconds. A
later clean run with four peer gates and seven Cargo processes still crossed the
hard limit, however, because fresh source mtimes continued to invalidate local
workspace fingerprints. After stabilizing those inputs, the two exact repeats
used 83.03 and 75.11 seconds of aggregate CPU. Their 67.35-second wall spread is
queueing rather than a second validation path: native compilation took less
than one second in both. An earlier incomplete attempt likewise spent 28.97
seconds waiting for and rebuilding the `cargo run` launcher, then correctly
refused an index that moved during selection at 33.02 seconds wall and only
1.18 seconds aggregate CPU. More compute or fewer simultaneous Cargo jobs can
reduce that wait; more agent lanes cannot.

The regression came from `rust-typecheck` creating a nonce-derived scratch
workspace with fresh source mtimes, `cargo-target`, and `cargo-build`
directories, then deleting the output pair after every
`cargo check --locked --workspace --all-targets`. That forced a cold dependency
build first and continued to invalidate local workspace crates after output was
made persistent. The repaired path keeps the immutable selected tree and exact
command. It synchronizes selected source into one worktree-Git-directory cache,
preserves bytes and mtimes for unchanged files, verifies the complete mirror,
and seals it while Cargo reads it. Native artifacts reuse the live worktree's
already-required ordinary `target`, while build-script output uses one stable
gate-owned subdirectory. A same-worktree cache possession explicitly refuses a
concurrent mirror instead of selecting a fallback path. Cargo remains the one
concurrency authority for overlapping worktree builds. The canvas arm retains
its separate persistent wasm/release pair because those artifacts do not
overlap the native launcher or all-target check.

The intermediate post-change refusals were not runtime or compiler failures.
They named latched debt in `fold/src/task_effort.rs` (1,504 lines against 1,500)
and `wire/src/intent.rs` (1,502 against 1,495). The final staged tree repaired
both through cohesive submodules without changing either floor. Exact clean
gates then completed under concurrent load without changing the 180-second
limit. The stable-source queue-bound case stayed below that limit, and its
immediate repeat met the sub-120-second target with lower aggregate CPU than
the earlier persistent-output runs.

## 2026-08-11 isolated fleet saturation

Task `PERFORM-1kLbsp1T` measured one canonical full verifier beside cold,
agent-equivalent Rust builds on the same 12-core, 24 GiB host. Each peer ran
`cargo test --locked -p wire --no-run` with `CARGO_BUILD_JOBS=12` and distinct
`CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` roots. `wire` retained proc-macro,
C, Rust, and linker work without multiplying the repository's approximately
30 GiB warm output footprint. One excluded cold warmup completed in 7.89
seconds and occupied 342,896 KiB. Each reported run used fresh peer roots and one
250 ms descendant-tree sampler; the disk column is the measurement-root delta.

| Agent-equivalent scale | Cold peer wall range | Initial verifier budget (workers/jobs/peers/workspace lanes) | Wall and result | Verifier progress | Peak processes / Cargo / rustc / linkers | Peak sampled RSS | Disk delta |
| ---: | ---: | --- | --- | --- | --- | ---: | ---: |
| 4 | 12.07–13.44 s | 3 / 4 / 0 / 4 | 217.69 s; cleanup fixture refusal | 9/13 actions; 16/32 shards | 109 / 8 / 51 / 6 | 4,513,760 KiB | 1,028,784 KiB |
| 8 | 18.42–36.59 s | 1 / 4 / 2 / 1 | 298.63 s; hard-supervisor interruption | 9/13 actions; 2/32 shards | 121 / 8 / 63 / 27 | 6,606,144 KiB | 2,401,396 KiB |
| 10 | 37.52–39.22 s | 3 / 4 / 0 / 4 | 238.87 s; cleanup fixture refusal | 9/13 actions; 16/32 shards | 191 / 10 / 99 / 20 | 10,644,288 KiB | 3,118,964 KiB |
| 8, focused comparison | 24.54–30.51 s | 3 / 4 / 0 / 4; unchanged after setup | 299.33 s; hard-supervisor interruption | 12/13 actions; 32/32 shards | 181 / 9 / 85 / 57 | 9,474,976 KiB | 2,436,264 KiB |

Follow-up task `PERFORM-1kLhqWQL` isolated the post-prebuild decision. It built
the verifier executable at the exact pre-rebalance revision `9ebe5631` and the
rebalance revision `dd52ab23`, then ran both executables from the same current
worktree and warm selected-root cache. Each arm launched seven fresh isolated
peer builds and withheld verifier launch until all seven Cargo processes were
observable. Both verifier probes independently saw exactly one remaining peer
and therefore started with the identical two-worker, four-job, two-lane budget.

| Implementation | Cold peer wall range | Initial budget (workers/jobs/peers/lanes) | Post-prebuild pressure and budget | Wall and result | Verifier progress | Peak processes / Cargo / rustc / linkers | Peak sampled RSS | Disk delta |
| --- | ---: | --- | --- | --- | --- | --- | ---: | ---: |
| Pre-rebalance `9ebe5631` | 22.60–33.17 s | 2 / 4 / 1 / 2 | zero peers by boundary timing; no second probe, budget stayed 2 / 4 / 1 / 2 | 296.42 s; hard-supervisor interruption | 9/13 actions; 13/32 shards | 153 / 7 / 83 / 18 | 7,031,872 KiB | 2,422,540 KiB |
| Rebalance `dd52ab23` | 22.90–32.96 s | 2 / 4 / 1 / 2 | published 0 peers and 3 / 4 / 0 / 4 | 289.28 s; success | 13/13 actions; 32/32 shards | 145 / 8 / 68 / 29 | 8,070,864 KiB | 2,455,452 KiB |

The boundary was pressure-free in both arms independently of the second probe.
The seven serialized prerequisite actions took 65.07 and 64.63 seconds before
workspace-test setup began; setup completed at approximately 106.26 and 108.06
seconds. Every peer had already exited by 33.17 and 32.96 seconds respectively.
The arms therefore match at launch and at the stable cache boundary. The only
production-path scheduling difference is that the rebalance arm observes and
publishes the recovered capacity.

Four agents are the first observed saturation point for this deliberately cold,
isolated mix. Independent Cargo jobservers granted up to 12 jobs apiece, so the
host saw 51 `rustc` processes at four agents and 99 at ten. Required output-root
isolation made duplicate compilation and disk growth roughly linear, but
neither the 108 GiB available disk nor 24 GiB memory capacity was exhausted.
Linker and overall process peaks increased sharply, yet the direct failures
were the verifier's scheduling and cleanup observations: four and ten exposed
an immediate PID-absence assertion while recently killed descendants were
still observable, and eight captured two peers at startup, retained one
workspace lane after every peer had exited, and reached the outer supervisor
after only two shards. The inconsistent initial peer counts of 0, 2, and 0 also
prove that one instantaneous process-list sample is not a stable fleet census.

The narrow repair leaves every command, action, target, isolated output root,
180-second test-phase deadline, and 300-second hard envelope intact. Cleanup
fixtures now allow the same bounded 500 ms post-reap observation already used
by adjacent process tests. The full verifier observes peer pressure again after
its serialized workspace-test prebuild and publishes that budget before Canvas
and shards start. Two or more peers that remain at that boundary use the single
degraded scheduling shape: one outer worker and one workspace lane, with the
Cargo job grant still divided from the observed CPU/participant count. A failed
observation fails all three dimensions closed to one. No alternate resolver or
unbounded retry is used.

The earlier focused eight-agent comparison remains non-causal because its
startup probe observed zero peers rather than the baseline's two. The matched
follow-up resolves that gap: with identical pressured startup budgets and
pressure-free prebuild boundaries, the fixed budget was interrupted after 13
shards while the rebalanced budget completed all 32 shards, all 13 actions, and
contract landing. The 7.14-second observed wall difference understates the
effect because the pre-rebalance arm did not complete. This is a material
envelope-level improvement consistent with the deterministic scheduler tests,
so no contradictory repeat was required and the production rebalance remains.

The broader cold isolated workload still has no GO at the original 4, 8, or 10
concurrent-agent matrix points. The measured efficient ceiling is below four;
this matrix did not identify the highest admitted lower count. Warm ordinary
work may behave better, but these samples do not establish a higher ceiling.

## Verify battery

“No catch found” means SPICE transcript forensics found no instance in this
port where the check stopped a defect. It does not claim the check could never
find one.

| Verify action | Measured wall | Defect actually caught in this port | Proposal for the speed-first cutover |
| --- | ---: | --- | --- |
| `cargo fmt --all -- --check` | 1.18 s | No catch found; formatting drift only | Remove from blocking verify and leave to editor/after-cutover polish. |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 12.43 s | A shared readiness fixture became dead code in suites that did not import it; strict lint stopped it | Keep in verify, not pre-commit. This is a demonstrated compile/lint catch. |
| `cargo check --locked --workspace --target wasm32-unknown-unknown` | 10.29 s | No distinct catch found. The observed WASM refusal came from the selected-tree canvas arm while this standalone check passed | Defer this duplicate class until after cutover unless it becomes the sole WASM check. |
| `cargo test --locked --workspace --all-targets` | **232.13 s to first failure** | On this exact audit run, `wire/tests/environment_vocabulary.rs` caught `SPICE_APP_SOURCE_IDENTITY` being removed as foreign even though Doctor owns it | Keep as the high-value integration battery, but run outside commit hooks. Shard or target by affected closure for feedback speed. |
| `cargo test --locked --workspace --doc` | 3.42 s | No catch found | Defer to post-cutover or scheduled CI. |
| Canonical Rust gate battery | **78.77 s** | See all 27 rows below | Replace in pre-commit with a deliberately small selected battery. Run retained expensive arms in verify/CI. |
| Contract landing inspection | 5.839 ms | No catch found; measured current report admitted all registered dispatch | Defer under the no-catch rule, although its savings are negligible and it is the cheapest check here. |

The workspace test command stopped at its first failure, so 232.13 seconds is a
lower bound rather than a complete-suite time. Measuring the later actions
independently gives a lower-bound full verify cost of about **338.22 seconds**.
A real `cargo xtask verify` on this snapshot stops after about 256.03 seconds at
the integration failure and never reaches doc tests, the battery, or landing.

## Workspace test sharding follow-up

Task `PERFORM-1kL5YDZ9` replaced that monolithic verifier action with a native
coordinator. On 2026-08-08, `/usr/bin/time -lp` measured both commands over the
same staged tree and warm Cargo cache. The final monolithic run rebuilt `app` in
2.54 seconds; that small rebuild is included in its wall and CPU totals. Both
commands completed the same current inventory: 138 test targets across 23
workspace packages.

| Run | Wall | User CPU | System CPU | Total CPU | Peak shards | Slowest shard |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Monolithic workspace | 241.40 s | 267.74 s | 74.95 s | 342.69 s | 1 | workspace, 241.40 s |
| 12-target shards | 115.83 s | 275.12 s | 84.83 s | 359.95 s | 4 | `spice`, 103.02 s |

The sharded result reduces wall time by 125.57 seconds, or 52.0%, while spending
5.0% more aggregate CPU. The coordinator's own elapsed time was 115.82 seconds,
below both the two-minute feedback goal and its hard 180-second bound. Its
slowest shard was also below two minutes. An immediately preceding successful
run completed in 96.16 seconds with an 87.42-second slowest shard, so the table
uses the slower repeat as the conservative result.

The partition is deterministic: packages and targets are metadata-sorted, then
split into chunks of at most 12 targets. Each chunk uses exact Cargo selectors
(`--lib`, `--bin`, and `--test`) so binary unit tests remain in the inventory.
Four process groups may run at once. Each group has its own 180-second deadline,
and the coordinator has the same overall bound. Start records print the complete
target inventory; 30-second progress records name the last observed test; and a
terminal record names the package, binary, disposition, and elapsed time.

## Pre-commit battery: all 27 arms

Core rows share 831.738 ms of eager family setup. Metrics rows share 31.374 ms.
Toolchain rows share 0.181 ms. Hygiene rows share 579.068 ms. Those values are
not repeated in each row and must be added once per retained family, not once
per arm.

| Arm | Direct wall | Defect actually caught in this port | Proposal |
| --- | ---: | --- | --- |
| `merge-integrity` | 8.567 ms | No catch found | Defer to post-cutover/landing validation. |
| `plan-phase` | 0.001 ms | No catch found | Remove from blocking hook. |
| `repo-shape` | 0.232 ms | No catch found | Defer to post-cutover. |
| `staging` | 20.780 ms | No catch found for partial staging | Defer to post-cutover. |
| `repo-docs` | 8.898 ms | No defect catch found; only bound-policy work was observed | Defer bound enforcement to post-cutover. |
| `config-key-validity` | 0.114 ms | No catch found | Defer; typed configuration tests already exercise this class. |
| `config-false-disable` | 12.643 ms | No catch found | Defer to post-cutover. |
| `config-tracked-trust` | 1.751 ms | No catch found | Defer to post-cutover. |
| `formatters` | 1.980 s | No catch found; duplicates top-level formatting | Remove from pre-commit. |
| `local-paths` | 0.009 ms | No catch found; clean changed-path floor | Defer to post-cutover. |
| `taste` | 0.002 ms | No catch found; clean changed-path floor | Defer to post-cutover. |
| `rust-typecheck` | 22.662 s | No distinct catch found; overlaps workspace Clippy/check | Remove from pre-commit. |
| `canvas-web-typecheck` | **35.773 s** | Caught a selected-tree native WASM check exiting 101 while standalone checks passed | Retain the class in verify/CI, but remove it from every commit. |
| `javascript-unused` | 65.377 ms | No catch found | Defer to post-cutover. |
| `csharp-unused-candidates` | 4.868 ms | No C# input; no catch found | Remove until C# exists in the port. |
| `python-typecheck` | 0.004 ms | No Python input; no catch found | Remove until Python exists in the port. |
| `env-policy` | 0.006 ms | No distinct catch found; the reported three-name event was an `env-name-ledger` refusal, not this arm | Defer to post-cutover. Reconsider only after an applicable-path benchmark and a demonstrated catch. |
| `env-name-ledger` | 1.375 s | Caught `GIT_DIR`, `GIT_WORK_TREE`, and `GIT_INDEX_FILE` missing from `spice-rust.toml`, but also blocked every lane on `SPICE_123`, a task-handle negative fixture mistaken for an env read | Move out of pre-commit until its jurisdiction and false-positive policy are repaired. Run in verify/CI meanwhile. |
| `file-shape` | 196.425 ms | Caught a new bound breach and caused work to split files; no functional defect was demonstrated | Defer to post-cutover. It is a structure preference with cross-lane coupling. |
| `complexity` | **3.544 s** | Caught an 80-line test routine, but also mis-measured a bodyless `extern "C"` declaration through EOF | Defer to post-cutover. The observed catches were shape debt, not behavior defects. |
| `magic-numbers` | 1.137 s | No catch found | Defer to post-cutover. |
| `markdown-links` | 3.342 s | No catch found | Defer to post-cutover or scheduled docs CI. |
| `reachability` | 1.359 s | Not applicable: selected Python files = 0 | Remove until Python returns. |
| `symbol-reachability` | 0.001 ms | Not applicable: selected Python files = 0 | Remove until Python returns. |
| `python-unused` | 0.001 ms | Not applicable: selected Python files = 0 | Remove until Python returns. |
| `assertion-free-tests` | <0.001 ms | Not applicable: selected Python files = 0 | Remove until Python returns. |
| `private-internals` | <0.001 ms | Not applicable: selected Python files = 0 | Remove until Python returns. |

`VERIFY-1kKFmwQ8` records the three-Git-name event precisely: each access still
carried an `env-policy: allow` marker that the environment-access arm honored,
while the separate environment-name ledger has no waiver concept and refused
because `spice-rust.toml` did not account `GIT_DIR`, `GIT_WORK_TREE`, or
`GIT_INDEX_FILE`. The event is therefore evidence for `env-name-ledger`, not an
`env-policy` catch.

## Complexity, file shape, and the flex slice

These arms have caught policy breaches, but no observed breach was a functional
defect. Their mechanics also turn local shape preferences into fleet-wide
coordination:

1. File shape measures every selected source as both lines and bytes.
   Complexity parses every selected routine for cyclomatic complexity and
   length. The current run paid 3.741 seconds for these two arms before shared
   metrics setup.
2. A fresh path receives actor-varied elastic headroom. The predecessor limits
   are a base routine length of 80 and a 1.5 elastic ratio, with five-percent
   actor variation; a prior breach latches the path/routine and collapses its
   active limit back to the base.
3. A breach is also represented by a Git-common, path-owned peer claim: earliest
   claim wins, same actor refreshes it, and another actor receives `PeerHeld`
   until expiry. The state fixture fixes the TTL at 21,600 seconds (six hours).
4. That is the “flex slice”: one lane spends elastic headroom and holds the path
   while other linked worktrees inherit the restriction. The resulting advice
   is to remain append-only or move seams, even when the blocked lane did not
   author the debt it is being asked to reorganize.

Two field events demonstrate the throughput cost. A test rename crossed the
80-line complexity bound and had to be split before an unrelated task could
commit. Later, a merge was byte-identical to its peer parent in three flagged
files but was charged against first parent; the peer flex claims did not expire
for hours, so the merge required a hook bypass and a separate defect task. The
current code now exempts a file inherited byte-for-byte from either merge
parent, which repairs that exact attribution case, but it does not remove the
shared latch/claim coupling for ordinary concurrent edits. The complexity
parser's bodyless-`extern` EOF behavior is a second example where unrelated
tail append work changes a routine measurement.

The audit's own final battery validation reproduced that coupling: the only
failure was `spice/src/board_command/client.rs` at 1,504 lines against a 1,485
line active flex limit. This audit neither edits nor stages that file; a peer's
shared-tree growth therefore blocked this documentation-only lane.

The historical speed-first proposal was to defer both arms and freeze their
ledgers, then restore them only with changed-path jurisdiction, no cross-actor
publication block, and parser tests proving that append-only tail work cannot
inflate an unrelated routine. That proposal is retained as evidence, not as a
current instruction.

## Historical cutover proposal (superseded)

The proposed pre-commit contract at the time was intentionally small:

- Retire the blocking battery from pre-commit for cutover. None of its 27 arms
  has both a demonstrated functional-defect catch and a cost/jurisdiction
  profile strong enough to retain on every commit.
- Do not acquire core, toolchain, metrics, or hygiene families for a hook
  profile with no retained arms. The current eager capture would otherwise pay
  most of their cost even after rows were hidden.
- Do not emit the complete success report. Print a compact success receipt and
  retain detailed evidence only for failures or an explicit audit command.
- Run Clippy, the selected-tree canvas check, the environment ledger, and the
  workspace integration suite in verify/CI, never once per commit.
- Park all never-caught and non-applicable checks until post-cutover, exactly as
  the operator's speed-first direction requests.

Removing only the two nested typechecks from pre-commit recovers about 58.44
seconds. Removing structural/docs scanners recovers another 8.22 seconds.
Avoiding unused family capture and full success serialization is required to
turn those row-level savings into a genuinely fast hook.
