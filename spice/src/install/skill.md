---
name: spice
description: Start or resume a worktree-bound Spice agent from a neutral bootstrap prompt.
---

# Spice

You were started by Spice. The initial prompt is only a bootstrap signal, not
the operator's request. Recover current work and steering from the Spice
surface before sending assistant prose:

1. `spice recall`
2. `spice board`
3. `spice inbox`

The launcher supplies the lane identity through `SPICE_LANE`. Treat a missing
lane, resident, or repository as a real refusal; do not fall back to another
runtime or infer work from this skill invocation.

Start the shared installed appliance with `$(git rev-parse --path-format=absolute --git-common-dir)/.spice-install/bin/spice up`.

## Work loop

- Acquire allocator-selected work with `spice task next`. Continue calling it
  after each completed phase until it reports that the lane has no work.
- Run external commands through `spice run -- <command>` so live steering is
  observed while you work.
- Handle every pending steering key promptly, then retire it with
  `spice ack <key>`. Use `spice ack <key> --decline` only for a reasoned refusal.
- Keep durable work in the task workflow. Use the landed leaves such as
  `spice task add`, `spice task note`, `spice task oops`, `spice task depends`,
  `spice task done`, and `spice task next`; trust their printed next action.
- Correct a task filed wrong with `spice task modify <handle>`, naming any of
  `--title`, `--description`, or `--acceptance`. The correction publishes a new
  statement revision attributed to you rather than overwriting the old text, so
  file the correction instead of restating it in a note. A settled task refuses
  the correction; carry the change forward in a new task.
- Withdraw a task that should never have been filed with `spice task delete
  <handle> "<reason>"`. The reason is required and recorded: the row leaves the
  working board but stays readable through `spice task list --status deleted`,
  so a withdrawal can be explained later. A task another lane is holding is
  refused and names the holder; `--force-claimed` withdraws it anyway, records
  whose work it took, and releases that lane. Withdraw what was filed in error
  and complete what was real work.
- Stay in the assigned worktree, do not spawn sub-agents, validate before
  completing a phase, and keep going while the allocator has work.

An executable-config or authored-input apply refusal is an operator decision
boundary. Surface the exact refusal and continue only independent allocator
work until the operator authorizes that exact mutation.
