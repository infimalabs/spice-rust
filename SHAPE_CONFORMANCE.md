# Rust self-hosting shape conformance

This audit compares shape `0c5bab269cbb1c4ca60edccf01444edf2b7b91cc`
with Rust `cc25c7b8b52ec9f722c51aea71660768f6d0eb6e` plus later reviewed repairs.

## Decision

**GO for Rust self-hosting under the bounded fleet contract.** The pinned
audit's three correctness blockers are closed and reviewed:

1. `STATE-1kLg3Pyn` publishes only accepted runs; `STATE-1kLkPtDG` makes
   historical sources read-only and carries sealed-index renames.
2. `SESSION-1kLg3XMy` binds authority only to an admitted live vendor identity.
3. `COMPATI-1kLcFlbD` versions Canvas separately from durable Wire and passes
   the installed load path with strict frame decoding intact.

Four agents are the isolated heavy mix's first saturation point; the 4/8/10
samples received no throughput GO. Those are measurements, not admission
thresholds. Repository-wide verification is now retired; package feedback runs
its Cargo children serially with one build job, and staged selection stays
bounded to the changed slice.

## Authority and method

The authoritative sibling `spice-shape` corpus was read in full at its pinned
revision: overview, invariants, cases, value models, loop specifications,
product shape, stories/failures, surface mechanics, and constitution.

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
| Journal is authoritative; projections rebuild | `journal/src/guard/commit.rs`, `journal/src/replay.rs`, `journal/src/migrate.rs` | **Stronger:** facts and heads commit together; replay checks derived thread/steering indexes. |
| A lane is a durable seat, not a process | `lane/src/table.rs`, `lane/src/renew.rs`, `spice/src/resident/lane_surface.rs` | **Match:** seat, lineage, occupant, epoch, capability, worktree, and session are distinct. |
| Read/render paths never launch agents | `lane/src/lib.rs`, `spice/src/resident/lifecycle.rs` | **Match:** lifecycle alone owns wake decisions. |
| One registry generates CLI syntax/help | `spice/src/cli.rs`, `spice/src/cli/contracts.rs`, `spice/src/cli/syntax.rs` | **Match:** each row binds syntax, principal, access, guards, tools, outcomes, and battery. |
| One active claim and lease prove possession | `board/src/claim.rs`, `board/src/allocate.rs`, `journal/src/guard/claim_index.rs` | **Stronger:** `Active` requires coherent heads; allocation mints both atomically. |
| Lapse re-proves expiry; takeover is immediate | `board/src/lapse.rs`, `board/src/next.rs`, `spice/src/board_command/tests.rs` | **Match:** witnesses recheck in transaction and the displaced claimant is told. |
| Allocation uses the fixed priority ladder | `board/src/next.rs` | **Match:** the sole selector is deterministic. |
| Non-clean review owns follow-up work | `spice/src/board_command/tests/resolution.rs` | **Stronger:** missing tracking refuses atomically. |
| One resident owns lifecycle | `spice/src/resident/lifecycle.rs`, `lane/src/lib.rs` | **Match:** every lifecycle invariant has one owner. |
| Lane outlives occupant; server owns topology revision | `lane/src/table.rs`, `spice/src/resident/arrangement/concurrency.rs` | **Stronger:** every topology mutation shares one revision predicate. |
| Git writes only at claim, completion, launch | `spice/src/task_git.rs`, `xtask/tests/git_boundaries.rs` | **Stronger:** one module plus a tree-wide network-command audit. |
| Claim advances only a clean non-diverged lane | `spice/src/task_git.rs::prepare_claim` | **Match:** dirt, loose commits, divergence, and missing recovery refuse. |
| Completion publishes before phase movement | `spice/src/task_git.rs::publish`, `spice/src/board_command/mutation.rs` | **Match:** the committing task fact carries the Git boundary. |
| Publication retry is narrow; conflict recoverable | `spice/src/task_git.rs::publish_with_retry`, `spice/src/task_git/transaction.rs` | **Stronger:** bounded CAS retry and write-ahead conflict state. |
| Only the task footprint lands | `spice/src/task_git/landing.rs`, `spice/src/task_git/landing_tests.rs` | **Stronger:** drift is classified with an executable remedy. |
| One collect-all gate roster | `gate/src/battery/manifest.toml`, `gate/src/battery.rs` | **Match:** 27 canonical arms publish one result vector. |
| Base, flex, latch, actor, jitter share one rule | `gate/src/state/bound.rs`, `gate/src/metrics/state_binding.rs`, `gate/src/core/acquire.rs` | **Stronger:** `BoundDecision` drives evidence and pending state. |
| Peer claims redirect flex debt | `gate/src/state/transaction.rs`, `gate/src/battery/authorities.rs`, `gate/src/battery/state_run.rs` | **Stronger:** accepted production runs publish one stale-detecting transaction; partial runs preserve unrelated claims and staged renames carry lineage. `STATE-1kLg3Pyn`, `STATE-1kLkPtDG`. |
| Pre-commit is bounded and local | `gate/src/change_selection.rs`, `spice/src/change_gate.rs`, `gate/src/workspace_tests/selection.rs` | **Deviation:** recognized staged paths select local policy/test evidence. Selector/toolchain authority, unclassified, and out-of-workspace Rust/manifest paths refuse before execution. Bare `cargo xtask verify` refuses rather than widening to the complete battery. |
| One run reads stable selected content | `gate/src/tree.rs`, `gate/src/battery/repository_executor.rs`, `spice/src/change_gate.rs` | **Deviation:** an immutable staged tree and digest replace GOV-36's selector/content split; tree/change-gate tests prove stability and fail closed on movement. |
| Durable gate state moves only on acceptance | `gate/src/state/transaction.rs`, `gate/src/battery/authorities.rs`, `gate/src/battery/state_run.rs` | **Stronger:** focused, selected, and full production paths publish only after total acceptance; rejection and historical revision runs are byte-preserving. `STATE-1kLg3Pyn`, `STATE-1kLkPtDG`. |
| Install plan is read-only and apply digest-pinned | `spice/src/install.rs`, `spice/src/install/progress.rs` | **Stronger:** apply re-plans and binds destination parent/generation. |
| Install interruption resumes and reversal is exact | `spice/src/install/record.rs`, `spice/src/install/progress.rs`, `spice/src/install/files.rs`, safety tests | **Match:** repeated apply/unapply converges; drift becomes residue. |
| Install and authority switch coincide | `INSTALL.md`, `spice/src/install.rs`, `spice/tests/coexistence.rs` | **Deviation:** common-Git assets install beside the predecessor; only operator authorization changes command/hook authority. Tests prove both boundaries. |
| Corrupt journal refuses; history migrates once | `journal/src/replay.rs`, `journal/src/guard/commit.rs`, `journal/src/migrate.rs`, tests | **Deviation:** an ordered one-way chain replaces one historical step because shipped Rust schemas must recover. Exclusive transaction and byte-exact rollback tests validate it. |
| Restore authenticates before authority changes | `journal/src/restore.rs`, `spice/src/restore_command.rs` | **Stronger:** owned bytes, manifest/digest/schema/replay checks, exact plan digest, and replacement rollback. |
| Current identity comes only from live environment | `spice/src/session_command/authority.rs` | **Stronger:** transcript inputs select evidence only; the preferred live vendor binds authority, while absent, malformed, unknown, or ambiguous live identity remains unknown. `SESSION-1kLg3XMy`. |

The journal is replayable authority beside Git, which attests publication.
Task-Git and resolution tests prove landings and restart survival. Gate state
publishes as one accepted-run transaction; rejection is byte-preserving.

## Risk ledger and exit

| Risk | Disposition and consequence |
| --- | --- |
| Gate state lineage | Resolved: `STATE-1kLg3Pyn`, `STATE-1kLkPtDG`; rejected/historical runs do not publish. |
| Live session authority | Resolved: `SESSION-1kLg3XMy`; transcript identity cannot impersonate the caller. |
| Canvas compatibility | Resolved: `COMPATI-1kLcFlbD`; installed loading is versioned and clean. |
| Local selector authority | Resolved: recognized paths select local evidence; selector/toolchain authority, unclassified, and out-of-workspace Rust/manifest paths refuse before execution. Renames retain both owners. |
| Fleet operation | Reviewed: `DEVELOP-1kLbrBsW`, `PERFORM-1kLbsp1T`, `PERFORM-1kLhqWQL`, `PERFORM-1kLbfC2X`; scheduling follows observed peer pressure. |
| Complete gate ownership | Retired: no routine command runs all 27 arms; staged selection and explicit study arms are the visible paths. |

The dependency chain proves selected and package feedback, installed entry and
Canvas load, digest-bound authority switch, exact rollback, authenticated
restore, and cleanup from Rust authority alone. Coexistence is only reversible
preparation before the operator-authorized switch.

Final decision: **GO, recoverable, with no unowned correctness gap.** Retained
deviations have executable validation. Package checks serialize their Cargo
children at one build job instead of creating a competing repository-wide
fleet. Canvas version mismatch reloads the authenticated current
document, gate findings migrate once or refuse with an owner, and gate state
admits one schema or names the owning release. None keeps a silent fallback.
