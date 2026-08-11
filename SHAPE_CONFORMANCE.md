# Rust self-hosting shape conformance

Task: `SELF_HO-1kLc5XkF`

This governed source audit compares product shape revision
`0c5bab269cbb1c4ca60edccf01444edf2b7b91cc` with Rust revision
`cc25c7b8b52ec9f722c51aea71660768f6d0eb6e`, before this record and its
topology regression. It does not authorize copying predecessor code.

## Decision

**NO-GO for declaring the Rust appliance fully qualified for self-hosting.**
The semantic port is broad and the strict verifier is green, but three live
correctness paths remain:

1. `STATE-1kLg3Pyn`: production gates read typed state but do not publish latch
   and peer-claim movement at the accepted-run boundary.
2. `SESSION-1kLg3XMy`: recovery may treat a transcript thread as the live
   caller when projecting steering and held work.
3. `COMPATI-1kLcFlbD`: the installed Canvas has a repeated, unreproduced
   undecodable-payload failure.

The parent `SELF_HO-1kLbzBGX` depends on these repairs, this audit, and the
existing verifier, fleet, development-loop, locality, ratchet, and remediation
evidence. Rust is suitable for controlled development and evidence collection;
the final self-hosting declaration waits.

## Authority and method

The authoritative sibling `spice-shape` checkout was read in full at the pinned
revision: `README.md`, `INVARIANTS.md`, `CASES.md`, `00-value-models.md`,
`01-loop-specifications.md`, `02-product-shape-i.md`,
`03-product-shape-ii.md`, `04-product-shape-iii.md`,
`05-stories-and-failures.md`, `06-surface-mechanics.md`, and
`07-constitution.md`.

Precedence is current operator direction and proven Rust invariants, fixed
shape semantics, free implementation choices, then historical mechanics.
**Match** preserves meaningful behavior; **stronger** adds fail-closed proof;
**deviation** changes behavior with rationale and validation below;
**obsolete** names a non-authoritative predecessor mechanism; **gap** names
missing live behavior with an owner.

## Contract map

| Shape commitment | Rust authority | Class and evidence |
| --- | --- | --- |
| One vocabulary; unknown variants fail | `wire/src/lib.rs`, `wire/src/fact.rs`, `wire/src/intent/binding.rs` | **Stronger:** closed enums, canonical encoding, admission, and `deny_unknown_fields`. |
| Task identity is its inception stamp; handles are projections | `wire/src/witness.rs`, `wire/src/body/task.rs`, `board/src/task.rs` | **Match:** `TaskStamp` is stored; public handles are derived. |
| Journal is authoritative; projections rebuild | `journal/src/guard/commit.rs`, `journal/src/replay.rs`, `journal/src/migrate.rs` | **Stronger:** facts and heads commit together; replay checks derived thread/steering indexes. |
| A lane is a durable seat, not a process | `lane/src/table.rs`, `lane/src/renew.rs`, `spice/src/resident/lane_surface.rs` | **Match:** seat, lineage, occupant, epoch, capability, worktree, and session are distinct. |
| Read/render paths never launch agents | `lane/src/lib.rs`, `spice/src/resident/lifecycle.rs` | **Match:** lifecycle alone owns wake decisions. |
| Language, storage, serialization, and visual spelling are free | `Cargo.toml`, `journal/src/lib.rs`, `wire/src/lib.rs`, `app/src/lib.rs` | **Match:** semantic owners survive; Python/Pydantic/JSONL/DOM spellings are **obsolete**. |
| One registry generates CLI syntax/help | `spice/src/cli.rs`, `spice/src/cli/contracts.rs`, `spice/src/cli/syntax.rs` | **Match:** each row binds syntax, principal, access, guards, tools, outcomes, and battery. |
| Every visible CLI leaf executes | `spice/src/cli.rs::landing`, `spice/src/cli/tests.rs` | **Stronger:** a second closed table proves all 97 leaves land. |
| Success, refusal, and fault differ | `wire/src/reply.rs::Outcome`, `spice/src/render.rs`, `spice/src/cli/tests.rs` | **Deviation:** one outer `Reply`/`Refusal`/`Fault` union prevents disagreement with an inner `ok`; tests prove exits 0/2/1. |
| Built-ins cannot be shadowed; aliases retire | `spice/src/cli/mounts.rs`, `spice/src/cli/contracts.rs`, `spice/src/cli/tests.rs` | **Match:** collision is admitted before dispatch; retired leaves are absent. |
| Reply position states its authority | `spice/src/cli.rs::ReplyPositionLaw`, `wire/src/reply.rs` | **Stronger:** registry law and typed answer carry the position. |
| Inception and origin are durable | `wire/src/head.rs`, `wire/src/envelope.rs`, `spice/src/board_command/tests/origin.rs` | **Match:** statements store `born` and provenance. |
| Bare capture prepends plan; accepted work does not | `board/src/task.rs`, `spice/src/board_command/tests/resolution.rs` | **Match:** executable shape cases 415–418. |
| One active claim and lease prove possession | `board/src/claim.rs`, `board/src/allocate.rs`, `journal/src/guard/claim_index.rs` | **Stronger:** `Active` requires coherent heads; allocation mints both atomically. |
| Lapse re-proves expiry; takeover is immediate | `board/src/lapse.rs`, `board/src/next.rs`, `spice/src/board_command/tests.rs` | **Match:** witnesses recheck in transaction and the displaced claimant is told. |
| Allocation uses the fixed priority ladder | `board/src/next.rs` | **Match:** the sole selector is deterministic. |
| Non-clean review owns follow-up work | `spice/src/board_command/tests/resolution.rs` | **Stronger:** missing tracking refuses atomically. |
| Task views project one head set | `board/src/handout.rs`, `board/src/graph.rs`, `spice/src/board_command/read_guard.rs` | **Match:** reads do not maintain parallel stores. |
| One resident owns lifecycle | `spice/src/resident/lifecycle.rs`, `lane/src/lib.rs` | **Match:** every lifecycle invariant has one owner. |
| Inbox outranks held claim, then work | `spice/src/resident/lifecycle.rs::Runtime::reconcile` | **Match:** one serialized decision and closed triggers. |
| Two-task, three-minute, and three-second thresholds | `spice/src/resident/lifecycle.rs`, `spice/src/resident/timers.rs` | **Match:** durable transitions and timers, not polling. |
| Held claims resume; retries are shared and observable | `spice/src/resident/lifecycle.rs`, `spice/src/resident/lane_surface/automatic.rs` | **Match:** one per-lane retry bucket does not block siblings. |
| Rapid deaths hold automatic seating; wake resumes | `lane/src/hold.rs`, `spice/src/resident/lifecycle.rs` | **Match:** replayable reduction plus observed process events. |
| Lane outlives occupant; server owns topology revision | `lane/src/table.rs`, `spice/src/resident/arrangement/concurrency.rs` | **Stronger:** every topology mutation shares one revision predicate. |
| Fusion retains independent members and attribution | `spice/src/resident/lane_surface/lineage.rs`, `spice/src/resident/arrangement/boundary.rs` | **Stronger:** complete admission and source boundaries prevent loss. |
| Canvas close is presentation-only | `app/src/browser/watchband/controls.rs`, `app/src/browser/view/lineage_menu.rs` | **Match:** it does not originate topology or terminate processes. |
| A team holds at most six agents | `spice/src/resident/lane_surface/lineage.rs`, `app/src/watchband.rs`, `spice/src/resident/arrangement_tests.rs` | **Deviation:** no server cap; six is only an accent palette. Lane identity and address carry attribution; the named seven-member regression proves none is lost. |
| Git writes only at claim, completion, launch | `spice/src/task_git.rs`, `xtask/tests/git_boundaries.rs` | **Stronger:** one module plus a tree-wide network-command audit. |
| Claim advances only a clean non-diverged lane | `spice/src/task_git.rs::prepare_claim` | **Match:** dirt, loose commits, divergence, and missing recovery refuse. |
| Completion publishes before phase movement | `spice/src/task_git.rs::publish`, `spice/src/board_command/mutation.rs` | **Match:** the committing task fact carries the Git boundary. |
| Publication retry is narrow; conflict recoverable | `spice/src/task_git.rs::publish_with_retry`, `spice/src/task_git/transaction.rs` | **Stronger:** bounded CAS retry and write-ahead conflict state. |
| Only the task footprint lands | `spice/src/task_git/landing.rs`, `spice/src/task_git/landing_tests.rs` | **Stronger:** drift is classified with an executable remedy. |
| Launch advance never raises but is visible | `spice/src/task_git.rs::advance_at_launch` | **Match:** typed notes reach immediate and durable launch identities. |
| One collect-all gate roster | `gate/src/battery/manifest.toml`, `gate/src/battery.rs` | **Match:** 27 canonical arms publish one result vector. |
| Passed, failed, and N/A remain distinct | `gate/src/verdict.rs`, `gate/src/metrics/evidence.rs` | **Match:** unavailable evidence never becomes success. |
| Base, flex, latch, actor, jitter share one rule | `gate/src/state/bound.rs`, `gate/src/metrics/state_binding.rs`, `gate/src/core/acquire.rs` | **Stronger:** `BoundDecision` drives evidence and pending state. |
| Peer claims redirect flex debt | `gate/src/state/transaction.rs`, `gate/src/battery/authorities.rs` | **Gap:** types exist; production publication does not. `STATE-1kLg3Pyn`. |
| Studies select jurisdiction; filing is explicit | `spice/src/gate_command.rs`, `spice/src/board_command/mutation/gate_findings.rs` | **Match:** filing is explicit and idempotent. |
| Pre-commit is bounded | `gate/src/change_selection.rs`, `spice/src/change_gate.rs`, `gate/src/workspace_tests/selection.rs` | **Deviation:** exact staged paths select policy/test evidence; unknown authority escalates exhaustively. Tests and `SELECTI-1kLc3tXp` validate locality. |
| One run reads stable selected content | `gate/src/tree.rs`, `gate/src/battery/repository_executor.rs`, `spice/src/change_gate.rs` | **Deviation:** an immutable staged tree and digest replace GOV-36's selector/content split; tree/change-gate tests prove stability and fail closed on movement. |
| Missing and empty magic baselines differ | `gate/src/metrics/acquire.rs::magic_state`, `gate/src/battery/tests/magic.rs` | **Match:** never-baselined files are N/A; explicit empty baselines find debt. Treating a new file as empty is **obsolete**. |
| Durable gate state moves only on acceptance | `gate/src/state/transaction.rs`, `gate/src/battery/authorities.rs` | **Gap:** transaction tests pass, but production reads `StateLedger` without finishing/publishing. `STATE-1kLg3Pyn`. |
| Install plan is read-only and apply digest-pinned | `spice/src/install.rs`, `spice/src/install/progress.rs` | **Stronger:** apply re-plans and binds destination parent/generation. |
| Install interruption resumes and reversal is exact | `spice/src/install/record.rs`, `spice/src/install/progress.rs`, `spice/src/install/files.rs`, safety tests | **Match:** repeated apply/unapply converges; drift becomes residue. |
| Install and authority switch coincide | `INSTALL.md`, `spice/src/install.rs`, `spice/tests/coexistence.rs` | **Deviation:** common-Git assets install beside the predecessor; only operator authorization changes command/hook authority. Tests prove both boundaries. |
| Corrupt journal refuses; history migrates once | `journal/src/replay.rs`, `journal/src/guard/commit.rs`, `journal/src/migrate.rs`, tests | **Deviation:** an ordered one-way chain replaces one historical step because shipped Rust schemas must recover. Exclusive transaction and byte-exact rollback tests validate it. |
| Restore authenticates before authority changes | `journal/src/restore.rs`, `spice/src/restore_command.rs` | **Stronger:** owned bytes, manifest/digest/schema/replay checks, exact plan digest, and replacement rollback. |
| Demo failure is reproducible | `spice/src/demo_command/recovery.rs` | **Match:** preserved root plus shell-safe retry/removal. |
| Briefing takes intent from steering and evidence from transcript | `spice/src/session_command/authority.rs`, `spice/src/session_command/dataset.rs`, `spice/src/session_command/project.rs` | **Match:** asks and held work precede transcript detail; unread probes stay unknown. |
| Current identity comes only from live environment | `spice/src/session_command/authority.rs::exact_thread` | **Gap:** transcript sources currently win before live vendor identities. `SESSION-1kLg3XMy`. |

The journal beside Git is an intentional addition, not a competing board:
immutable journal facts make receipts, claims, topology, steering, and review
atomic and replayable; Git attests source publication. `spice/src/task_git/tests.rs`
proves baseline-first publication and trailers; `spice/src/release_history.rs` reads first-parent
landings, and journal resolution tests prove restart survival.

Shape GOV-20 retires a healed latch even if a run fails, while GOV-24 defers
writes until the whole run passes. The Python implementation buffered and
discarded retirement on failure. Rust governs the contradiction as an
accepted-run transaction: rejection cannot mutate durable quality state. The
gap is production wiring, not the discarded contradictory sentence.

## Risk ledger and exit

| Risk | Owner and consequence |
| --- | --- |
| Gate state is not published | `STATE-1kLg3Pyn`; **blocking**, because later runs read stale quality state. |
| Transcript can impersonate current authority | `SESSION-1kLg3XMy`; **blocking**, because live work can attach to the wrong session. |
| Installed Canvas rejects a payload | `COMPATI-1kLcFlbD`; **blocking**, because the operator path is not clean. |
| Local selection and full studies await review | `SELECTI-1kLc3tXp`; evidence risk despite green implementation. |
| Ratchet provenance and remediation await review | `SELECTI-1kLc51jD`, `REMEDIA-1kLc40qB`; state repair must retain their locality contracts. |
| Development and fleet evidence await review | `DEVELOP-1kLbrBsW`, `PERFORM-1kLbsp1T`, `PERFORM-1kLbfC2X`; qualification risk. |
| Teams exceed six | Governed, non-blocking deviation with seven-member regression. |
| New magic files have no delta | Governed, non-blocking GOV-35 match; applicability stays explicit. |

At the pinned Rust revision, `cargo xtask verify-action selection` and all 13
actions of `cargo xtask verify` passed. This audit's final checks belong in its
task completion evidence.

The parent may declare GO only when all repair tasks complete review; every
evidence dependency settles; the unchanged verifier passes clean at the final
revision; installed entry, authority switch, rollback, journal restore, and
Canvas load work without a predecessor checkout; and fleet evidence supports
the four-agent default with loud degradation at eight to ten.

Until then: **NO-GO, recoverable, with no unowned shape gap.** Every live gap
has a public task, and every retained deviation has rationale and executable
validation.
