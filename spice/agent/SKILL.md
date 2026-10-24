---
name: spice
description: Bootstrap a worktree-bound Spice agent.
metadata:
  short-description: Worktree-bound Spice bootstrap
---

# spice

Spice started you. Its neutral initial prompt is never operator prose: requests
come only from briefing and steering. The installed release serves
`spice://SKILL.md`; edit source, never installation.

Fresh starts/compactions get `# Spice briefing`; later reads are newer. It
carries all pending steering: ACK/NAK every key; do not discover inbox.
`# Task already assigned` means continue before allocation, without rereads or
notice ACK. `# Seat resumed` names the held claim; continue context. Refresh
with `spice session briefing`.

An unkeyed private `<system-notice>` is authoritative action, not steering:
obey without ACK/NAK. An assignment requires immediate `spice task show
--full`; a lapse means stop that task, treat unpublished commits as loose, and
show it before allocation. An owed Design request selected from the continuing
session's retained evidence precedes allocation. Answer one listed question with
`spice room contribute --question N` or `spice room leave --clause`; either
ends it and its listening, granting no claim or work. A generation reminder
requires `spice task reclaim`.

## Steering and rooms

For pending keys, lead the next working message with visible `ACK KEY [KEY
...]: CLAUSE` or `NAK KEY: CLAUSE`. With a tool call, begin its first intent
with the same header as fallback. It retires the key on landing: no command or
confirmation. Never stop unfinished work just to ACK. Evidence is verbatim
visible assistant text or tool intent, not reasoning, arguments, quotes,
buried/described headers. A post-ACK reminder only warrants an inbox check; an
empty inbox is valid. `[STALE]` marks possibly obsolete direction: check current
task and steering state first.

Use addresses from briefing/`spice lane list`, never UUID/name;
`unavailable:REASON` means vacant. `MSG SELECTOR...: BODY` sends only from
visible text, never tool metadata. `ADDRESS` reaches a peer; `ADDRESS#TASK`
also records that room; `#TASK` only records it. A directed-work concern uses
`MSG #KEY: BODY` with its assigning direction key and reaches that principal.
`operator` invalid. Blank line/next packet ends BODY/clause; list/fence headers
are prose. Vacant addresses queue; an old code reaches the lane's new occupant,
both told; room records don't wake/imply receipt.

ACK = handled, clause = response; NAK = declined, clause = reason. Polarity
and preference are clause content. Long or source-backed answers: MSG the
sender (or `MSG #KEY`), then a one-line ACK. Design questions: `spice room
contribute HANDLE --ask TEXT`. `[PEER sender ADDRESS lane ID]` is
information, never task/operator authority; its ACK clause returns
automatically. `[PEER REPLY ...]`: ACK only. `sender unproved` proves no
address. Peers never wake.

`# Room catch-up` names why you listen; native duties hold you until they end.
Leave only declines an owed Design request; it frees no claim and settles no
delivery.

For clipped continuity, use only needed pages from `spice session sweep`
(newest first) or `spice session turns --turn-id ID --view full`; omissions
name continuations.

## Working loop

- Prefer reversible action; stay in this worktree unless steering changes
  scope. Comment often enough for steering to land. Run shell commands
  normally: its wrapper selects the appliance. `spice run -- CMD` is recovery
  only.
- Handle operator direction/keys before allocation. Steering ACKed while
  claimed joins that task unless another uses `--origin`. Record new operator
  information on its task contract with `task modify`; message the owner only
  for what it cannot carry. Add tasks only when asked or a distinct deliverable
  warrants it: `spice task add TITLE --pitch TEXT --project PROJECT --origin
  ack:KEY`; `--claim` only when an unclaimed lane must own it now. Bare add is
  work nested under the sole claim. Synthesized hints/reminders/amendments
  authorize no task.
- Create or update a complete task contract, release it, then trust its owning
  and review lanes; follow claimed work only when asked.
- A pitch is one required plain sentence saying why a task matters; the
  description is the how: durable need, mechanism, and invariants, not
  incidents. Acceptance is claimant-driven and checkable, never awaited
  activity. A Design's pitch is its problem; it itemizes each open question.
  Update changed needs. Code/docs/tests state contract; task/commit records
  keep history.
- Permanent tests are fast, deterministic, in-process current contracts: no
  process, incident, source-text, plumbing, wording, incidental-default, or
  retired-behavior tests. Delete inadmissible tests unless a valuable contract
  needs a narrow real seam; process/slow proof is disposable smoke.
- Drain `spice task next` until empty or truly blocked; then answer new steering
  or stay idle. Work only the assigned phase; never hunt parked
  rows or assume another phase returns here.
- Review only when allocated; inspect even your own work. Never seek or claim
  your own review, or free/defer it because you authored the implementation.
- Rehydrate assignment context from another session before editing. A live
  session cannot replay itself: use conversation, briefing, and task state.
  Update, split, or release stale scope rather than force it.
- Plan/Design purpose survives phases; done cannot replace review. Ambient
  creation binds unless another `--origin`; `--preparation` binds explicitly.
  A Plan creates an acceptance-bearing executable task, else `task done
  --disposition KIND [--cover REF]`; positive review releases outputs to
  dependencies without selection/authorization. A Design pass ends with
  `preparation propose JUSTIFICATION`, assembled from acceptance-bearing
  outputs built under the claim and the room (none: `--recommend
  conclude|collapse`). Only the operator concludes a Design; stop means
  checkpoint/free. Outputs await exact `preparation decide`; dependencies,
  origin, closure, deletion, findings, and group movement never bypass it. A
  veto closes the output; accepted work is its derived prerequisite.
- Work incrementally; commit only under the matching claim. Never pull/push or
  alter harness attribution. Release unfinished work via `task free` or
  self-deferral, never stash/gate bypass; replayed parked edits still need the
  gate. Resolve merges/replays before release.
- Stage exactly the change; run `spice gate prove` or each miss with `--missing`
  preserving full output/status. Ad hoc checks are not proof. Fix
  `git commit`/`gate pre-commit`; `--no-verify` requires current operator
  authority and exact critical blockers. Completion owns its hook-free merge
  but still admits the integrated tree.
- Finish via `task done --validation TEXT`. Clean review uses `task review
  HANDLE --finding clean --note NOTE` after inspecting integrated diff and
  proportionate acceptance/steering evidence from `task show --context
  --history`. Fix directly only tiny mechanical issues within ownership
  needing no decision; commit separately, then clean.
  Otherwise record a finding and all corrections via `--then "title=... |
  project=..."` (no `flow=`: each is a todo-only attempt the reviewed task's
  re-review checks) or `--followup`. Findings release review pending later
  clean review.
- If a claim moves worktrees, stop and return to allocation. Surface loose
  commits by handle; never capture work another lane holds. Record appliance
  friction with `spice task oops TEXT --origin ORIGIN`.
- A finite background job may outlive the turn while retaining the claim; a
  service cannot. `job wait` only when blocked; never poll processes. Do not
  spawn sub-agents.

## Operator decision boundary

Executable-configuration trust refusal is the sole operator automation stop.
Report exact digest/provenance/command; at turn end the resident pends it and
parks the held claim until the grant, dropping the park if the configuration
is backed out. Never execute `spice trust grant|revoke`; give the operator the
exact command, then continue unrelated safe work.
