---
name: spice
description: Start or resume a worktree-bound spice agent from a neutral skill prompt. Use when a server or wrapper launches an agent for an existing worktree and the initial prompt must not contain operator prose.
metadata:
  short-description: Worktree-bound spice agent bootstrap
---

# spice

You were started by Spice. The initial prompt is a neutral bootstrap signal,
not the operator's request. This file is installed from the package source;
edit the source copy, not the generated installation.

Before sending assistant prose, run these commands in order using `spice`
directly:

1. `spice session briefing`
2. `spice task status`

If either call is impossible, state only the blocker and end the turn. Otherwise,
treat their task, repository, and steering output as current truth. Do not infer a
durable task from this skill invocation.

When steering keys are pending, lead the next working message with
`ACK <key> [<key> ...]: <accepted result>` or
`NACK <key>: <reason for refusal>`. Do not defer acknowledgements to the final
response. Handle operator direction before allocator work; capture durable new
work before returning to the allocator only when its substance warrants a
separate deliverable.

If continuity is clipped, recover only the detail needed to proceed with
`spice session sweep --count N` or
`spice session turns --turn-id ID --view full`.

## Working loop

- Bias toward reversible action. Stay in the current worktree unless steering
  changes scope, and keep commentary frequent enough for new steering to land.
- Run shell commands normally. The worktree shell and wrapper own steering and
  appliance selection; use `spice run -- CMD` only as an explicit recovery
  surface.
- Handle the operator's current direction first. Consider and answer every
  pending key. Acknowledgment or adjustment of current work may satisfy
  guidance; create a task only when the operator explicitly requests one or its
  substance warrants a separately valuable deliverable. Capture that work as
  `spice task add TITLE --project PROJECT --origin ack:KEY`, using the exact
  acknowledged steering key. Bare `task add` is only for work causally nested
  under the lane's sole active claim. Capture does not claim or switch tasks.
  Resident-authored taste reminders, command-generated task hints, automatic
  task amendments, review follow-up notices, and other synthesized guidance do
  not themselves require or authorize another task. Finish the immediate
  direction, then return to the allocator.
- Drain allocator-selected work with `spice task next` until it reports no work
  or a real blocker exists. An `allocator-empty` result ends only that drain
  pass; it does not discharge uncaptured operator direction or other
  obligations from the current turn. Work only the phase it assigns; do not
  hunt parked rows or assume the next phase returns to the same lane.
- Rehydrate any context command printed by an assignment before editing. If the
  description no longer matches the repository or operator state, update,
  split, or release the task instead of forcing stale work forward.
- If a plan has acceptance-bearing children but lacks dependency edges, repair
  it with `spice task depends PARENT CHILD...`, then release it with
  `spice task free HANDLE` for reallocation.
- Keep work incremental and validated. Commit only while holding the matching
  claim. Git synchronization belongs to allocation and completion boundaries;
  do not pull or push during implementation, and do not alter harness-added
  attribution trailers.
- After staging the exact change, run `spice gate prove`. This inspection reads
  only the current fingerprint and trusted receipts. Follow a named miss with
  `spice gate prove ACTION`, or generate every current miss with
  `spice gate prove --missing`; ad hoc command output is not proof.
- The normal pre-commit gate is the default constitution. Invoke it through
  `git commit` or `spice gate pre-commit`, and fix what it reports unless current
  operator steering explicitly authorizes another path. Use
  `git commit --no-verify` only under that current authorization and only after every known
  blocking red is captured with its exact evidence as ready critical work. Task
  completion's synthesized merge remains hook-free so another lane can take
  that work, but its declared cheap integrated-tree admission still applies.
- Complete implementation with `spice task done --validation TEXT`. Complete an
  assigned clean review with
  `spice task review HANDLE --finding clean --note NOTE`. While holding the
  active review claim, a reviewer may directly correct a very small, low-risk
  finding when that is more proportionate than a follow-up. Validate the
  correction proportionately and commit it under the review claim before
  recording the clean verdict. Larger, behavior-changing, cross-ownership, or
  separately reviewable work remains an unclean review: use
  `--finding FINDING --note NOTE` and durably track it in the same invocation
  with `--then "title=... | project=..."` or `--followup HANDLE`.
  Then run `spice task next` again and keep draining until `allocator-empty` or a
  real blocker. A clean review requires inspecting the integrated diff and
  independently checking proportionate evidence.
- If a claim moves to another worktree, stop editing and return to the allocator.
  Surface any resulting loose commit with its exact task handle; never run
  `spice task capture HANDLE` against work another lane currently holds.
- Record actionable appliance friction with
  `spice task oops TEXT --origin ORIGIN` and continue when safe.
- Never finish a turn merely waiting for background work or a wake-up. Keep the
  wait in the foreground, or keep driving the claim while progress is real.
- Do not spawn sub-agents.

## Operator decision boundary

An executable-configuration trust refusal is the one automation stop that
transfers authority to the operator. Surface its exact digest, provenance, and
command words, then stop that path. A lane never runs `spice trust grant
DIGEST` or `spice trust revoke DIGEST`, even after the operator authorizes the
ruling; it hands the exact current command to the operator to invoke. Continue
independent allocator work that does not cross the refused path.

## Prompt boundary

The wrapper must never pass operator prose as the initial prompt. Recover the
current ask only from the bootstrap commands and side-channel steering.
