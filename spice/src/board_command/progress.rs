use super::*;

mod doc;
mod refresh;
mod release;
pub(super) use doc::doc_apply;
use refresh::*;
pub(super) use release::{claim_free, prepare_release};

pub(super) fn claim_refresh(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    refresh: &TaskRef,
) -> Outcome<CommandAnswer> {
    claim_refresh_with_clock(bus, command, refresh, current_instant)
}

pub(super) fn claim_refresh_with_clock(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    refresh: &TaskRef,
    clock: impl FnOnce() -> Result<Instant, Outcome<CommandAnswer>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match command_task_snapshot(bus, command, &refresh.task) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match refresh_entry(&snapshot, refresh) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let at = match clock() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let state = match refresh_claim(bus, entry, &snapshot, command, refresh, at) {
        Ok(state) => state,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let acknowledged = entry
        .claim()
        .expect("refresh admission requires a claim")
        .is_stale(entry.workflow())
        .then_some(entry.workflow().generation);
    let held = entry.lease().is_some_and(spice::claim_lease::is_indefinite);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(outcome) = append_refresh(&mut write, entry, state) {
        return outcome;
    }
    let answer = wire::intent::ClaimAnswer {
        task: entry.handle().clone(),
        statement: Some(Box::new(wire::intent::ClaimStatement {
            title: entry.statement().title.clone(),
            pitch: entry.statement().pitch.clone(),
            description: entry.statement().description.clone(),
            acceptance: entry.statement().acceptance.clone(),
        })),
        holder: Some(command.actor().clone()),
        expiry: None,
        acknowledged,
        held,
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        room_context: None,
        next: None,
        phase: None,
        nothing_stolen: false,
        transplant: None,
    };
    if let Err(error) = write.command_result(
        request(command, refresh.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::refresh-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(Box::new(answer)))
}

// Generation acknowledgement and residency recovery live in
// `progress/refresh.rs`, apart from ordinary workflow transitions.

pub(super) fn task_done(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match command_task_snapshot(bus, command, &done.task) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match done_entry(&snapshot, done) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let operation = match done.operation {
        Some(operation) => operation,
        None => {
            return refusal(
                "task::completion-operation-required",
                Some(entry.handle().to_string()),
                "retry with a current client that supplies one durable completion operation",
            );
        }
    };
    if let Some(replayed) = replay_task_completion(&snapshot.facts, command, done, entry, operation)
    {
        return replayed;
    }
    if !entry.is_filed() {
        return refusal(
            "task::filing-required",
            Some(filing_state(entry)),
            &format!(
                "file with spice task file {} [--into PUBLIC_PROJECT], add required public dependencies, then complete it",
                entry.handle(),
            ),
        );
    }
    let (at, lane, transition) =
        match done_transition(&snapshot, entry, command, false, done.disposition.as_ref()) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
    let source = match bus.source_boundary(&lane) {
        Ok(source) => source,
        Err(error) => {
            return store_fault("task::boundary-unreadable", &error.to_string());
        }
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let phase = entry.workflow().phase.as_str();
    let parked = if gate::lacks_commit_authority(phase) {
        let claimed = entry.claim().expect("completion requires a claim").claimed;
        match release::prepare_release(&snapshot, command, claimed, repository, Some(phase)) {
            Ok(parked) => parked,
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    let published = match publish_task_boundary(repository, &snapshot, command, entry) {
        Ok(published) => published,
        Err(outcome) => return rollback_release(parked, outcome),
    };
    let carried = carries_claim(
        transition.kind(),
        &transition.next().flow,
        published.as_ref(),
        parked.is_some(),
    );
    let next = if carried {
        Some(claim_activation_next_action(entry))
    } else {
        projected_done_next_action(&snapshot, entry, &transition)
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let answer = match append_done(
        &mut write,
        command,
        done,
        entry,
        Completion {
            transition: &transition,
            at,
            published,
            source,
            next,
            parked: parked.as_ref().map(|release| release.parked()),
            carried,
        },
    ) {
        Ok(answer) => answer,
        Err(outcome) => return rollback_release(parked, outcome),
    };
    if let Err(error) = write.command_result(
        request(command, done.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return rollback_release(
            parked,
            store_fault("task::done-receipt", &error.to_string()),
        );
    }
    match parked {
        Some(parked) => committed_with_task_git(
            bus,
            write,
            CommandAnswer::Task(Box::new(answer)),
            PreparedTaskGit::Release(parked),
        ),
        None => committed(bus, write, CommandAnswer::Task(Box::new(answer))),
    }
}

fn done_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    done: &TaskDone,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    task_entry(&snapshot.entries, &done.task).ok_or_else(|| {
        refusal(
            "task::unknown",
            Some(done.task.to_string()),
            concat!(
                "read board and complete the task held by ",
                "this capability",
            ),
        )
    })
}

/// The committed reply for a completion this actor already landed.
///
/// The snapshot indexes every landed terminal by task and operation, so a
/// retry finds its committed position without walking the history. The phase
/// fact the completion caused and the claim that phase fact caused, when the
/// completion carried its claim, extend the committed range and the answer.
fn replay_task_completion(
    facts: &bus::TaskFacts,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    entry: &Entry,
    operation: wire::CanonicalUuid,
) -> Option<Outcome<CommandAnswer>> {
    let first = facts.completion_terminal(entry.born(), operation)?;
    let envelope = facts.at(first)?;
    let completed = envelope.fact.read::<body::TaskDone>()?;
    let terminal = completed.terminal?;
    if envelope.actor != *command.actor()
        || completed.validation != done.validation
        || completed.disposition != done.disposition
    {
        return None;
    }
    let mut following = facts.after(Some(first));
    let phased = following.next().and_then(|following| {
        let phased = following.fact.read::<body::TaskPhased>()?;
        (following.cause == Some(first) && phased.task == *entry.born())
            .then_some((following.seq, phased.phase))
    });
    let carried = phased.as_ref().and_then(|(phased, phase)| {
        let following = following.next()?;
        let won = following.fact.read::<body::ClaimWon>()?;
        (following.cause == Some(*phased) && won.task == *entry.born()).then(|| {
            (
                following.seq,
                carried_claim_answer(entry, &won.actor, phase, won.until),
            )
        })
    });
    let last = carried
        .as_ref()
        .map(|(seq, _)| *seq)
        .or_else(|| phased.as_ref().map(|(seq, _)| *seq))
        .unwrap_or(first);
    Some(Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange { first, last }),
        body: CommandAnswer::Task(Box::new(wire::intent::TaskAnswer {
            taste: None,
            task: entry.handle().clone(),
            status: terminal.status.to_string(),
            completion: None,
            claim: carried.map(|(_, claim)| Box::new(claim)),
            closed: Vec::new(),
            next: terminal.next,
        })),
    }))
}

fn done_transition(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    resume_epoch: bool,
    disposition: Option<&body::PlanDisposition>,
) -> Result<(Instant, wire::CanonicalUuid, board::Transition), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let active = board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            "run spice task next before completing its phase",
        )
    })?;
    if active.epoch() != command.epoch() && !resume_epoch {
        return Err(refusal(
            "claim::epoch-moved",
            Some(active.epoch().to_string()),
            &format!(
                "run spice task next to rebind task {} under the current resident generation",
                entry.handle()
            ),
        ));
    }
    acknowledged(entry)?;
    require_lane_progress(entry)?;
    refuse_taste_hold(entry)?;
    if entry.workflow().phase.as_str() == "review" {
        return Err(refusal(
            "task::review-required",
            Some(entry.handle().to_string()),
            "record an assigned native review verdict before advancing review",
        ));
    }
    let current = entry
        .workflow()
        .flow
        .iter()
        .position(|phase| phase == &entry.workflow().phase)
        .expect("validated workflow contains its phase");
    let transition = match (disposition, entry.workflow().flow.get(current + 1)) {
        (Some(_), _) => board::Transition::dispose(entry, &active, at),
        (None, Some(next)) => board::Transition::advance(entry, &active, next.clone()),
        (None, None) => board::Transition::complete(entry, &active, at),
    }
    .map_err(|error| {
        refusal(
            "task::phase-refused",
            Some(error.to_string()),
            "complete only the currently held workflow phase",
        )
    })?;
    admit_completion(snapshot, entry, command, disposition)?;
    Ok((at, active.lane(), transition))
}

/// Refuse a lane step only after a Design reaches its open phase. An intake
/// plan advances with `task done`; an intake review advances through its
/// assigned native verdict.
fn require_lane_progress(entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    if !wire::intent::is_open_design(entry.statement().intent, entry.workflow()) {
        return Ok(());
    }
    Err(refusal(
        "task::design-open",
        Some(entry.handle().to_string()),
        &format!(
            "a Design stays open until the operator concludes it; end this pass with one \
             `spice design propose JUSTIFICATION`, which assembles what the pass built and \
             releases your claim on {}, or abandon the pass with `spice task free {}`",
            entry.handle(),
            entry.handle()
        ),
    ))
}

/// Refuse completing any phase of a task whose statement stands under a taste
/// hold, naming each match and the rewording that clears it.
fn refuse_taste_hold(entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    entry
        .statement()
        .taste
        .as_ref()
        .map_or(Ok(()), |hold| Err(taste_hold_refusal(entry, hold)))
}

/// Admit a successful completion: a phase advance, a final completion, or a
/// clean review verdict.
///
/// A Plan's validated no-construction disposition closes its plan phase
/// without acceptance, and the one disposition recorded then stands for every
/// later phase; closing the plan phase again restates it. Every other
/// successful completion rests on the task's acceptance contract.
fn admit_completion(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    disposition: Option<&body::PlanDisposition>,
) -> Result<(), Outcome<CommandAnswer>> {
    if let Some(disposition) = disposition {
        let plan = visible_task(snapshot, entry.born())?;
        require_expected(command, &statement_witness(plan))?;
        require_expected(command, &workflow_witness(plan))?;
        return admit_disposition(snapshot, entry, command, disposition);
    }
    if entry.workflow().phase.as_str() != "plan"
        && snapshot.facts.plan_disposition(entry.born()).is_some()
    {
        return Ok(());
    }
    require_acceptance(snapshot, command, entry.born())
}

/// Admit a Plan lane's no-construction disposition on the completion that
/// closes its plan phase.
///
/// Only the Plan's own plan phase records one, never beside work the Plan
/// constructed that is still live, and a cover names an existing task the
/// command witnessed or one full commit id.
fn admit_disposition(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    disposition: &body::PlanDisposition,
) -> Result<(), Outcome<CommandAnswer>> {
    let plan = entry.born();
    if snapshot
        .facts
        .plan_disposition(plan)
        .is_some_and(|recorded| recorded != disposition)
    {
        return Err(refusal(
            "task::disposition-conflict",
            Some(entry.handle().to_string()),
            "a Plan records one disposition; complete it without --disposition, or restate the \
             recorded one when closing its plan phase again",
        ));
    }
    let owner = planning::construction_owner(plan, entry.statement().preparation.as_ref());
    if !planning::planning_deliverable(entry) {
        return Err(refusal(
            "task::disposition-refused",
            Some(entry.handle().to_string()),
            "record a no-construction disposition only when completing a Plan's own plan phase; \
             remove an ordinary claimed duplicate with `spice task delete HANDLE REASON \
             --force-claimed`",
        ));
    }
    if owner != plan
        || entry.workflow().phase.as_str() != "plan"
        || !planning::declares_plan(entry.statement().intent, &entry.workflow().flow)
    {
        return Err(refusal(
            "task::disposition-refused",
            Some(entry.handle().to_string()),
            "record a no-construction disposition only when completing a Plan's own plan phase",
        ));
    }
    // Work the Plan constructed, or linked accepted output that answers its
    // need, contradicts a claim that no work is warranted.
    let constructed = snapshot.answer.tasks.iter().find(|task| {
        task.workflow.body.status.is_pending()
            && task
                .statement
                .body
                .preparation
                .as_ref()
                .is_some_and(|preparation| preparation.parent == *plan)
    });
    if let Some(output) = constructed.or(accepted_pending_child(snapshot, entry)?) {
        return Err(refusal(
            "task::disposition-beside-output",
            Some(output.statement.body.handle.to_string()),
            "a Plan with live or linked constructed work completes through that work; settle, \
             unlink, or delete it before recording that no work is warranted",
        ));
    }
    let witnessed = |cover: &wire::TaskStamp| {
        command
            .expected()
            .iter()
            .any(|witness| matches!(witness, Witness::Task { task, .. } if task == cover))
    };
    match disposition.cover() {
        Some(cover @ body::DispositionCover::Commit(_)) if !cover.is_well_formed() => Err(refusal(
            "task::disposition-cover-invalid",
            Some(format!("{cover:?}")),
            "name a covering commit by its full object id",
        )),
        Some(body::DispositionCover::Task(cover)) if cover == plan || !witnessed(cover) => {
            Err(refusal(
                "task::disposition-cover-invalid",
                Some(cover.to_string()),
                "name another existing task as the cover and witness its statement",
            ))
        }
        _ => Ok(()),
    }
}

/// Whether a completion carries its claim into the phase it enters rather than
/// freeing it: nothing published or parked, and another declared phase after
/// the one entered. A review phase is never carried, because review goes to a
/// peer before its author.
pub(super) fn carries_claim(
    kind: &board::TransitionKind,
    flow: &[wire::Phase],
    published: Option<&body::GitBoundary>,
    parks: bool,
) -> bool {
    let board::TransitionKind::Advance { to, .. } = kind else {
        return false;
    };
    if parks || published.is_some_and(|boundary| boundary.local_commits > 0) {
        return false;
    }
    to.as_str() != "review"
        && flow
            .iter()
            .position(|phase| phase == to)
            .is_some_and(|index| index + 1 < flow.len())
}

/// What one completion settled before the journal opened: the workflow change,
/// the boundaries it recorded, and whether the claim is carried or freed.
struct Completion<'a> {
    transition: &'a board::Transition,
    at: Instant,
    published: Option<body::GitBoundary>,
    source: body::SourceBoundary,
    next: Option<Box<str>>,
    parked: Option<&'a body::TaskParkedCommits>,
    carried: bool,
}

fn append_done(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    entry: &Entry,
    completion: Completion<'_>,
) -> Result<wire::intent::TaskAnswer, Outcome<CommandAnswer>> {
    let Completion {
        transition,
        at,
        published,
        source,
        next,
        parked,
        carried,
    } = completion;
    let status = transition.next().status.as_str().to_owned();
    let terminal_status = if transition.next().status.is_pending() {
        body::TaskCompletionStatus::Pending
    } else {
        body::TaskCompletionStatus::Completed
    };
    let completed = write
        .fact(
            body::TaskDone {
                task: entry.born().clone(),
                validation: done.validation.clone(),
                terminal: Some(body::TaskCompletionTerminal {
                    operation: done
                        .operation
                        .expect("live task completion requires an operation"),
                    status: terminal_status,
                    next: next.clone(),
                }),
                git: published.clone(),
                source: source.clone(),
                disposition: done.disposition.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("task::done-build", &error.to_string()))?;
    let workflow_fact = match transition.kind() {
        board::TransitionKind::Advance { to, .. } => write
            .fact(
                body::TaskPhased {
                    task: entry.born().clone(),
                    phase: to.clone(),
                },
                FactCause::Pending(completed),
            )
            .map_err(|error| store_fault("task::phase-build", &error.to_string()))?,
        board::TransitionKind::Complete { .. } => completed,
    };
    let workflow = transition.next().clone();
    write.workflow_head(entry.born().clone(), workflow_fact, move |_| workflow);
    let claim = if carried {
        // Mint the completing lane's claim on the phase just entered, in place
        // of the claim it held, with the ordinary lease and the boundaries the
        // completion recorded, as a consequence of the phase fact.
        let held = entry.claim().expect("Active proof requires a claim head");
        let (expiry, maximum) = claim_lease(at)?;
        mint_claim(
            write,
            board::allocate::Mint {
                task: entry.born(),
                actor: command.actor(),
                workflow: transition.next(),
                project: entry.project().as_str(),
                lane: held.lane,
                occupant: held.occupant,
                epoch: command.epoch(),
                at,
                expiry,
                maximum,
                git: published,
                recovery: None,
                source,
                cause: Some(workflow_fact),
            },
        )?;
        Some(Box::new(carried_claim_answer(
            entry,
            command.actor(),
            &transition.next().phase,
            expiry,
        )))
    } else {
        free_claim(write, entry, completed, parked)?;
        None
    };
    Ok(wire::intent::TaskAnswer {
        taste: None,
        task: entry.handle().clone(),
        status,
        completion: None,
        claim,
        closed: Vec::new(),
        next,
    })
}

/// Free the completing claim, parking commits when the completion released any.
fn free_claim(
    write: &mut JournalWrite,
    entry: &Entry,
    completed: PendingFact,
    parked: Option<&body::TaskParkedCommits>,
) -> Result<(), Outcome<CommandAnswer>> {
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    let released = if let Some(parked) = parked {
        write
            .fact(
                body::ClaimFreed {
                    task: entry.born().clone(),
                    actor: claim.actor.clone(),
                    parked: Some(parked.clone()),
                },
                FactCause::Pending(completed),
            )
            .map_err(|error| store_fault("task::plan-release-build", &error.to_string()))?
    } else {
        completed
    };
    write.claim_head(entry.born().clone(), released, move |fact| ClaimHead {
        fact,
        ..claim
    });
    Ok(())
}

/// The carried claim said where the lane reads the completion: the phase it
/// now holds and that phase's deliverable, with the statement it already works
/// from omitted.
fn carried_claim_answer(
    entry: &Entry,
    actor: &Actor,
    phase: &wire::Phase,
    until: Instant,
) -> wire::intent::ClaimAnswer {
    wire::intent::ClaimAnswer {
        task: entry.handle().clone(),
        statement: None,
        holder: Some(actor.clone()),
        expiry: Some(Box::new(until)),
        acknowledged: None,
        held: false,
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        room_context: None,
        next: Some(claim_activation_next_action(entry)),
        phase: Some(wire::intent::ClaimPhase {
            name: phase.clone(),
            deliverable: board::phase::deliverable(phase).map(Box::from),
        }),
        nothing_stolen: false,
        transplant: None,
    }
}

fn projected_done_next_action(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    transition: &board::Transition,
) -> Option<Box<str>> {
    let mut claim = entry
        .claim()
        .expect("an admitted done transition has a claim")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    let post = Entry::new(
        entry.statement().clone(),
        transition.next().clone(),
        Some(claim),
        entry.lease().cloned(),
    )
    .expect("an admitted transition preserves a valid task entry")
    .with_grouped(entry.grouped())
    .with_design(entry.design());
    next_action(snapshot, &post)
}

/// Resolve the one entry this actor may settle, and the transition it completes.
///
/// Every refusal on the way to a verdict lives here: an unknown handle, a task
/// that is not this actor's assigned review, a claim that is no longer active,
/// and a phase that is not the final one.
fn admit_review<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &'a wire::AdmittedCommand,
    review: &TaskReview,
) -> Result<(&'a Entry, board::Transition, Instant, wire::CanonicalUuid), Outcome<CommandAnswer>> {
    let Some(entry) = task_entry(&snapshot.entries, &review.task) else {
        return Err(refusal(
            "task::unknown",
            Some(review.task.to_string()),
            "read board and name one current review task",
        ));
    };
    if entry.workflow().phase.as_str() != "review"
        || entry.workflow().status.reviewer() != Some(command.actor())
    {
        return Err(refusal(
            "review::assignment-refused",
            Some(entry.handle().to_string()),
            "win the review phase with task next before recording its verdict",
        ));
    }
    let at = current_instant()?;
    let active = board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            "win a current review claim before recording its verdict",
        )
    })?;
    acknowledged(entry)?;
    let current = entry
        .workflow()
        .flow
        .iter()
        .position(|phase| phase == &entry.workflow().phase)
        .expect("validated workflow contains its phase");
    // A disposed Plan constructs nothing, so after review it enters only a later
    // review or settles.
    let disposed = snapshot.facts.plan_disposition(entry.born()).is_some();
    let transition = match (disposed, entry.workflow().flow.get(current + 1)) {
        (true, _) => board::Transition::dispose(entry, &active, at),
        (false, Some(next)) => board::Transition::advance(entry, &active, next.clone()),
        (false, None) => board::Transition::complete(entry, &active, at),
    }
    .map_err(|error| {
        refusal(
            "review::phase-refused",
            Some(error.to_string()),
            "review only the currently assigned workflow phase",
        )
    })?;
    Ok((entry, transition, at, active.lane()))
}

/// Refuse an active claim that has not acknowledged its task's generation.
///
/// The task changed after the claim last took stock of it through a statement
/// edit, observation, deferral, or workflow move. The holder must read the
/// current statement through `task next` before completing or reviewing.
fn acknowledged(entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    let claim = entry.claim().expect("an admitted active claim has a head");
    if !claim.is_stale(entry.workflow()) {
        return Ok(());
    }
    Err(refusal(
        "claim::generation-stale",
        Some(format!(
            "task {} is at generation {}; this claim acknowledged {}",
            entry.handle(),
            entry.workflow().generation,
            claim.acknowledged
        )),
        &format!(
            "task {} changed since this claim acknowledged it; run spice task next to read and acknowledge the current statement",
            entry.handle(),
        ),
    ))
}

/// What one review verdict records: the verdict, a finding's text with its
/// note, a clean verdict's note, and the corrections the review linked.
struct ReviewRecord {
    verdict: body::Verdict,
    finding: Option<String>,
    note: Option<String>,
    followups: Vec<TaskStamp>,
}

/// Decide the verdict a review records and the text it carries.
///
/// A finding that tracks nothing is refused here rather than written, so the
/// change a reviewer asked for always has a task behind the reviewed one.
fn review_verdict(
    review: &TaskReview,
) -> Result<(body::Verdict, Option<String>, Option<String>), Outcome<CommandAnswer>> {
    let verdict = if review_is_clean(&review.finding) {
        body::Verdict::Clean
    } else {
        body::Verdict::Finding
    };
    if verdict == body::Verdict::Finding && review.then.is_empty() && review.followup.is_empty() {
        return Err(refusal(
            "review::followup-required",
            Some(review.finding.clone()),
            "track an unclean review with --then \"title=... | project=...\" or --followup HANDLE",
        ));
    }
    let note = review
        .note
        .as_ref()
        .filter(|note| !note.trim().is_empty())
        .cloned();
    Ok(match verdict {
        body::Verdict::Clean => (verdict, None, note),
        body::Verdict::Finding => {
            let finding = match note {
                Some(note) => format!("{}\n\n{note}", review.finding),
                None => review.finding.clone(),
            };
            (verdict, Some(finding), None)
        }
    })
}

/// Every correction this review links, in the order the reviewer named it:
/// each new `--then` task, then each existing `--followup` a finding attached
/// to `statement`, the reviewed task's statement after the review.
fn review_followups(
    review: &TaskReview,
    verdict: body::Verdict,
    statement: &TaskHead,
) -> Vec<TaskStamp> {
    let mut followups = review
        .then
        .iter()
        .map(|followup| followup.task.clone())
        .collect::<Vec<_>>();
    if verdict == body::Verdict::Finding {
        for born in review
            .followup
            .iter()
            .filter_map(|handle| handle.born().ok())
        {
            if statement.dependencies.contains(&born) && !followups.contains(&born) {
                followups.push(born);
            }
        }
    }
    followups
}

fn review_followup_creation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
    review: &TaskReview,
) -> Result<Option<TaskCreationCapture>, Outcome<CommandAnswer>> {
    if review.then.is_empty() {
        Ok(None)
    } else {
        task_creation_capture(bus, command, snapshot).map(Some)
    }
}

struct PreparedReview {
    snapshot: ValidatedSnapshot,
    entry: Entry,
    transition: board::Transition,
    at: Instant,
    source: body::SourceBoundary,
    record: ReviewRecord,
    statement: TaskHead,
    origin: CreationCause,
    git: Option<body::GitBoundary>,
    creation: Option<TaskCreationCapture>,
}

fn prepare_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<Host<'_>>,
) -> Result<PreparedReview, Outcome<CommandAnswer>> {
    let roots = task_review_roots(review);
    let snapshot = validated_working_snapshot_for_tasks(bus, command.actor().clone(), &roots)?;
    if let Some(entry) = task_entry(&snapshot.entries, &review.task)
        && !entry.is_filed()
    {
        return Err(refusal(
            "task::filing-required",
            Some(filing_state(entry)),
            &format!(
                "file with spice task file {} before it enters review",
                entry.handle(),
            ),
        ));
    }
    let (entry, transition, at, lane) = admit_review(&snapshot, command, review)?;
    let source = bus
        .source_boundary(&lane)
        .map_err(|error| store_fault("review::boundary-unreadable", &error.to_string()))?;
    let (verdict, finding, note) = review_verdict(review)?;
    let statement = match verdict {
        body::Verdict::Clean => {
            refuse_taste_hold(entry)?;
            admit_completion(&snapshot, entry, command, None)?;
            entry.statement().clone()
        }
        body::Verdict::Finding => correction_statement(&snapshot, command, entry, review)?,
    };
    let record = ReviewRecord {
        verdict,
        finding,
        note,
        followups: review_followups(review, verdict, &statement),
    };
    let parent = wire::TaskProvenance::Task(entry.born().clone());
    let origin = resolve_creation_origin(None, command.actor(), &snapshot, Some(parent), false)?;
    let git = publish_task_boundary(repository, &snapshot, command, entry)?;
    let creation = review_followup_creation(bus, command, &snapshot, review)?;
    Ok(PreparedReview {
        entry: entry.clone(),
        snapshot,
        transition,
        at,
        source,
        record,
        statement,
        origin,
        git,
        creation,
    })
}

pub(super) fn task_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    task_review_with(bus, command, review, repository, || {})
}

pub(super) fn task_review_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<Host<'_>>,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let prepared = match prepare_review(bus, command, review, repository) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    after_validation();
    commit_review(bus, command, review, repository, prepared)
}

fn commit_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<Host<'_>>,
    prepared: PreparedReview,
) -> Outcome<CommandAnswer> {
    let PreparedReview {
        snapshot,
        entry,
        transition,
        at,
        source,
        record,
        statement,
        origin,
        git,
        creation,
    } = prepared;
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    if let Some(creation) = &creation {
        creation.guard(&mut write);
    }
    let verdict = record.verdict;
    let workflow = review_workflow(&entry, &transition, verdict);
    let completion = if workflow.status.is_pending() {
        body::TaskCompletionStatus::Pending
    } else {
        body::TaskCompletionStatus::Completed
    };
    let reviewed =
        match append_reviewed_fact(&mut write, command, &entry, record, completion, git, source) {
            Ok(reviewed) => reviewed,
            Err(outcome) => return outcome,
        };
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    let post = Entry::new(
        statement.clone(),
        workflow.clone(),
        Some(claim.clone()),
        entry.lease().cloned(),
    )
    .expect("an admitted review preserves a valid task entry")
    .with_grouped(entry.grouped())
    .with_design(entry.design());
    let next = next_action(&snapshot, &post);
    let reviewed_task = match task_heads(&snapshot.answer, &review.task) {
        Ok(reviewed_task) => reviewed_task,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = record_followups(
        &mut write,
        &review.then,
        &FollowupContext {
            command,
            snapshot: &snapshot,
            reviewed_task,
            reviewed,
            origin: &origin,
            creation: creation.as_ref(),
            repository,
        },
    ) {
        return outcome;
    }
    let state_cause = match append_review_statement(&mut write, &entry, statement, reviewed) {
        Ok(cause) => cause,
        Err(outcome) => return outcome,
    };
    let status = workflow.status.as_str().to_owned();
    write.workflow_head(entry.born().clone(), state_cause, move |_| workflow);
    write.claim_head(entry.born().clone(), state_cause, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let answer = review_answer(entry.handle().clone(), status, next);
    if let Err(error) = write.command_result(
        request(command, review.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("review::receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}

fn append_reviewed_fact(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    record: ReviewRecord,
    status: body::TaskCompletionStatus,
    git: Option<body::GitBoundary>,
    source: body::SourceBoundary,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    write
        .fact(
            body::TaskReviewed {
                task: entry.born().clone(),
                reviewer: command.actor().clone(),
                verdict: record.verdict,
                status,
                finding: record.finding,
                note: record.note,
                followups: record.followups,
                git,
                source,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("review::build", &error.to_string()))
}

fn review_workflow(
    entry: &Entry,
    transition: &board::Transition,
    verdict: body::Verdict,
) -> WorkflowHead {
    let mut workflow = match verdict {
        body::Verdict::Clean => transition.next().clone(),
        body::Verdict::Finding => entry.workflow().clone(),
    };
    match verdict {
        body::Verdict::Clean => match &mut workflow.status {
            wire::WorkflowStatus::Pending(review) => *review = wire::WorkflowReview::Clean,
            wire::WorkflowStatus::Settled {
                conclusion: wire::WorkflowConclusion::Positive { review },
                ..
            } => *review = wire::WorkflowCompletionReview::Clean,
            wire::WorkflowStatus::Settled {
                conclusion: wire::WorkflowConclusion::Negative(_),
                ..
            } => unreachable!("a clean review cannot negatively settle a workflow"),
        },
        body::Verdict::Finding => workflow.status = wire::WorkflowStatus::pending(),
    }
    workflow
}

fn append_review_statement(
    write: &mut JournalWrite,
    entry: &Entry,
    statement: TaskHead,
    reviewed: PendingFact,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    if &statement == entry.statement() {
        return Ok(reviewed);
    }
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let dependencies = (statement.dependencies != entry.statement().dependencies)
        .then(|| statement.dependencies.clone());
    let edited = write
        .fact(
            body::TaskEdited {
                intent: None,
                task: entry.born().clone(),
                revision,
                dependencies,
            },
            FactCause::Pending(reviewed),
        )
        .map_err(|error| store_fault("review::dependency-build", &error.to_string()))?;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    Ok(edited)
}

fn review_answer(
    task: TaskHandle,
    status: String,
    next: Option<Box<str>>,
) -> wire::intent::TaskAnswer {
    wire::intent::TaskAnswer {
        taste: None,
        task,
        status,
        completion: None,
        claim: None,
        closed: Vec::new(),
        next,
    }
}
