use super::*;

mod reclaim;
use reclaim::*;

pub(super) fn claim_free(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskRef,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &task.task) else {
        return refusal(
            "task::unknown",
            Some(task.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let Some(claim) = entry.claim() else {
        return refusal(
            "claim::head-missing",
            Some(entry.handle().to_string()),
            "free only a task carrying an active claim",
        );
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *command.actor() {
        return refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "free the task through the capability that currently owns it",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut post_claim = claim.clone();
    post_claim.disposition = ClaimDisposition::Freed;
    let mut post_workflow = entry.workflow().clone();
    let review_released = post_workflow.phase.as_str() == "review"
        && post_workflow.assignment.as_ref() == Some(command.actor());
    if review_released {
        post_workflow.assignment = None;
        post_workflow.review = "unassigned".to_owned();
    }
    let post = Entry::new(
        entry.statement().clone(),
        post_workflow,
        Some(post_claim),
        entry.lease().cloned(),
        projects,
    )
    .expect("releasing a claim preserves a valid task entry");
    let next = next_action(&snapshot, &post);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let freed = match write.fact(
        body::ClaimFreed {
            task: entry.born().clone(),
            actor: command.actor().clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("claim::free-build", &error.to_string()),
    };
    let next_claim = post
        .claim()
        .expect("released task retains its claim history")
        .clone();
    write.claim_head(entry.born().clone(), freed, move |fact| ClaimHead {
        fact,
        ..next_claim
    });
    if review_released {
        let workflow = post.workflow().clone();
        write.workflow_head(entry.born().clone(), freed, move |_| workflow);
    }
    let answer = wire::intent::ClaimAnswer {
        task: entry.handle().clone(),
        holder: command.actor().clone(),
        expiry: None,
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        next,
        phase: None,
    };
    if let Err(error) = write.command_result(
        request(command, task.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::free-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(Box::new(answer)))
}

pub(super) fn claim_reclaim(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    claim_reclaim_with_clock(bus, command, renew, projects, allocation, current_instant)
}

pub(super) fn claim_reclaim_with_clock(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    clock: impl FnOnce() -> Result<Instant, Outcome<CommandAnswer>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match reclaim_entry(&snapshot, renew) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let at = match clock() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let window = match reclaim_window(bus, entry, &snapshot, command, renew, at) {
        Ok(window) => window,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, window.producer());
    guard(&mut write, command.expected().to_vec());
    let until = window.expiry();
    if let Err(outcome) = append_reclaim(&mut write, entry, command, at, window) {
        return outcome;
    }
    let answer = wire::intent::ClaimAnswer {
        task: entry.handle().clone(),
        holder: command.actor().clone(),
        expiry: Some(Box::new(until)),
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        next: None,
        phase: None,
    };
    if let Err(error) = write.command_result(
        request(command, renew.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::reclaim-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(Box::new(answer)))
}

// Reclaim admission and head publication live in `progress/reclaim.rs` so the
// ordinary workflow transitions below remain independently reviewable.

pub(super) fn task_done(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
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
    if is_oops(entry) {
        return refusal(
            "task::promotion-required",
            Some(oops_state(entry)),
            &format!(
                "promote with spice task wake {} --into PUBLIC_PROJECT, add required public dependencies, then complete it",
                entry.handle(),
            ),
        );
    }
    let (at, lane, transition) = match done_transition(&snapshot, entry, command, false) {
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
    let published = match publish_task_boundary(repository, &snapshot, command, entry) {
        Ok(published) => published,
        Err(outcome) => return outcome,
    };
    let next = projected_done_next_action(&snapshot, entry, &transition, projects);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let answer = match append_done(
        &mut write,
        done,
        entry,
        &transition,
        published,
        source,
        next,
    ) {
        Ok(answer) => answer,
        Err(outcome) => return outcome,
    };
    if let Err(error) = write.command_result(
        request(command, done.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::done-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
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
/// retry finds its committed position without walking the history.
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
    if envelope.actor != *command.actor() || completed.validation != done.validation {
        return None;
    }
    let last = facts
        .after(Some(first))
        .next()
        .and_then(|following| {
            let phased = following.fact.read::<body::TaskPhased>()?;
            (following.cause == Some(first) && phased.task == *entry.born())
                .then_some(following.seq)
        })
        .unwrap_or(first);
    Some(Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange { first, last }),
        body: CommandAnswer::Task(wire::intent::TaskAnswer {
            task: entry.handle().clone(),
            status: terminal.status,
            guidance: terminal.guidance,
            completion: None,
            learning: None,
            claim: None,
            next: terminal.next,
        }),
    }))
}

fn done_transition(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    resume_epoch: bool,
) -> Result<(Instant, wire::CanonicalUuid, board::Transition), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let active = board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            concat!(
                "renew or win a current claim before ",
                "completing its phase",
            ),
        )
    })?;
    if active.epoch() != command.epoch() && !resume_epoch {
        return Err(refusal(
            "claim::epoch-moved",
            Some(active.epoch().to_string()),
            &format!(
                "run spice task reclaim {} under the current resident generation",
                entry.handle()
            ),
        ));
    }
    acknowledged(entry)?;
    let current = entry
        .workflow()
        .flow
        .iter()
        .position(|phase| phase == &entry.workflow().phase)
        .expect("validated workflow contains its phase");
    let transition = match entry.workflow().flow.get(current + 1) {
        Some(next) => board::Transition::advance(entry, &active, next.clone()),
        None => board::Transition::complete(entry, &active, at),
    }
    .map_err(|error| {
        refusal(
            "task::phase-refused",
            Some(error.to_string()),
            "complete only the currently held workflow phase",
        )
    })?;
    admit_plan_transition(snapshot, entry, command, &transition)?;
    Ok((at, active.lane(), transition))
}

fn admit_plan_transition(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    transition: &board::Transition,
) -> Result<(), Outcome<CommandAnswer>> {
    if entry.workflow().phase.as_str() != "plan" {
        return Ok(());
    }
    let task = visible_task(snapshot, entry.born())?;
    require_expected(command, &statement_witness(task))?;
    require_expected(command, &workflow_witness(task))?;
    let completes_plan = matches!(
        transition.kind(),
        board::TransitionKind::Complete { phase } if phase.as_str() == "plan"
    );
    if has_taste_review(&entry.statement().tags) {
        // Every recheck writes its own audit, and the first one was written
        // when the task was filed. A lane that rewords and refuses again can
        // only tell progress from stasis if the refusal says how many reads
        // the statement has already survived.
        let rechecks = taste_rechecks(&snapshot.facts, entry.born()).saturating_sub(1);
        let standing = match rechecks {
            0 => "the filed statement has not been revised".to_owned(),
            1 => "one revision has been rechecked and the statement still matches".to_owned(),
            count => {
                format!("{count} revisions have been rechecked and the statement still matches")
            }
        };
        return Err(refusal(
            "task::taste-review-required",
            Some(entry.handle().to_string()),
            &format!(
                "{standing}; change what the statement says, then spice task modify {} lets the complete current taste policy recheck it",
                entry.handle(),
            ),
        ));
    }
    if !completes_plan && substantive_acceptance(&entry.statement().acceptance) {
        return Ok(());
    }
    if let Some(child) = accepted_pending_child(snapshot, entry)? {
        require_expected(command, &statement_witness(child))?;
        require_expected(command, &workflow_witness(child))?;
        return Ok(());
    }
    let remedy = if completes_plan {
        "connect a pending child carrying acceptance before completing a plan-only flow"
    } else {
        "add acceptance to the task, or connect a pending child carrying acceptance, before closing the plan phase"
    };
    Err(refusal(
        "task::plan-incomplete",
        Some(entry.handle().to_string()),
        remedy,
    ))
}

fn accepted_pending_child<'a>(
    snapshot: &'a ValidatedSnapshot,
    entry: &Entry,
) -> Result<Option<&'a BoardTask>, Outcome<CommandAnswer>> {
    for dependency in &entry.statement().dependencies {
        let child = visible_task(snapshot, dependency)?;
        if child.workflow.body.status == "pending"
            && substantive_acceptance(&child.statement.body.acceptance)
        {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn visible_task<'a>(
    snapshot: &'a ValidatedSnapshot,
    task: &wire::TaskStamp,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    snapshot
        .answer
        .tasks
        .iter()
        .find(|heads| heads.statement.body.born == *task)
        .ok_or_else(|| {
            protocol_fault(
                "task::snapshot-incoherent",
                "validated board snapshot lost a task entry",
            )
        })
}

fn require_expected(
    command: &wire::AdmittedCommand,
    witness: &Witness,
) -> Result<(), Outcome<CommandAnswer>> {
    if command
        .expected()
        .iter()
        .any(|expected| expected == witness)
    {
        return Ok(());
    }
    Err(refusal(
        "task::plan-witness-missing",
        Some(format!("{witness:?}")),
        "read the current plan task and its accepted child before closing the plan phase",
    ))
}

fn append_done(
    write: &mut JournalWrite,
    done: &TaskDone,
    entry: &Entry,
    transition: &board::Transition,
    published: Option<body::GitBoundary>,
    source: body::SourceBoundary,
    next: Option<Box<str>>,
) -> Result<wire::intent::TaskAnswer, Outcome<CommandAnswer>> {
    let status = transition.next().status.clone();
    let guidance = plan_advance_guidance(transition);
    let completed = write
        .fact(
            body::TaskDone {
                task: entry.born().clone(),
                validation: done.validation.clone(),
                terminal: Some(body::TaskCompletionTerminal {
                    operation: done
                        .operation
                        .expect("live task completion requires an operation"),
                    status: status.clone(),
                    guidance,
                    next: next.clone(),
                }),
                git: published,
                source,
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
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(entry.born().clone(), completed, move |fact| ClaimHead {
        fact,
        ..claim
    });
    Ok(wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status,
        guidance,
        completion: None,
        learning: None,
        claim: None,
        next,
    })
}

fn plan_advance_guidance(transition: &board::Transition) -> Option<wire::intent::TaskGuidance> {
    matches!(
        transition.kind(),
        board::TransitionKind::Advance { from, .. } if from.as_str() == "plan"
    )
    .then_some(wire::intent::TaskGuidance::PlanAdvanceOwnership)
}

fn projected_done_next_action(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    transition: &board::Transition,
    projects: &ProjectPolicy,
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
        projects,
    )
    .expect("an admitted transition preserves a valid task entry");
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
        || entry.workflow().assignment.as_ref() != Some(command.actor())
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
    let transition = board::Transition::complete(entry, &active, at).map_err(|error| {
        refusal(
            "review::phase-refused",
            Some(error.to_string()),
            "review only the final assigned workflow phase",
        )
    })?;
    Ok((entry, transition, at, active.lane()))
}

/// Refuse an active claim that has not acknowledged its task's generation.
///
/// The task changed after this claim last took stock of it (a statement edit,
/// an observation, a deferral, or a workflow move), so the holder must reread
/// and reclaim before completing or reviewing on stale understanding.
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
            "task {} changed since this claim acknowledged it; reread it, then run spice task reclaim {}",
            entry.handle(),
            entry.handle()
        ),
    ))
}

/// Decide the verdict a review records and the finding text it carries.
///
/// A finding that tracks nothing is refused here rather than written, so the
/// change a reviewer asked for always has a task behind the reviewed one.
fn review_verdict(
    review: &TaskReview,
) -> Result<(body::Verdict, Option<String>), Outcome<CommandAnswer>> {
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
    let finding = (verdict == body::Verdict::Finding).then(|| match &review.note {
        Some(note) if !note.trim().is_empty() => format!("{}\n\n{note}", review.finding),
        _ => review.finding.clone(),
    });
    Ok((verdict, finding))
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
    verdict: body::Verdict,
    finding: Option<String>,
    statement: TaskHead,
    origin: wire::Origin,
    git: Option<body::GitBoundary>,
    creation: Option<TaskCreationCapture>,
}

fn prepare_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<PreparedReview, Outcome<CommandAnswer>> {
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    let (entry, transition, at, lane) = admit_review(&snapshot, command, review)?;
    let source = bus
        .source_boundary(&lane)
        .map_err(|error| store_fault("review::boundary-unreadable", &error.to_string()))?;
    let (verdict, finding) = review_verdict(review)?;
    let statement = match verdict {
        body::Verdict::Clean => entry.statement().clone(),
        body::Verdict::Finding => {
            correction_statement(&snapshot, command, entry, review, projects)?
        }
    };
    let parent = task_origin(entry.born())?;
    let origin = resolve_creation_origin(None, command.actor(), &snapshot, Some(parent))?;
    let git = publish_task_boundary(repository, &snapshot, command, entry)?;
    let creation = review_followup_creation(bus, command, &snapshot, review)?;
    Ok(PreparedReview {
        entry: entry.clone(),
        snapshot,
        transition,
        at,
        source,
        verdict,
        finding,
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
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    task_review_with(
        bus,
        command,
        review,
        repository,
        projects,
        allocation,
        || {},
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn task_review_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let prepared = match prepare_review(bus, command, review, repository, projects, allocation) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    after_validation();
    commit_review(bus, command, review, repository, projects, prepared)
}

fn commit_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    prepared: PreparedReview,
) -> Outcome<CommandAnswer> {
    let PreparedReview {
        snapshot,
        entry,
        transition,
        at,
        source,
        verdict,
        finding,
        statement,
        origin,
        git,
        creation,
    } = prepared;
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    if let Some(creation) = &creation {
        creation.guard(&mut write);
    }
    let reviewed =
        match append_reviewed_fact(&mut write, command, &entry, verdict, finding, git, source) {
            Ok(reviewed) => reviewed,
            Err(outcome) => return outcome,
        };
    let workflow = review_workflow(&entry, &transition, verdict);
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
        projects,
    )
    .expect("an admitted review preserves a valid task entry");
    let next = next_action(&snapshot, &post);
    if let Err(outcome) = record_followups(
        &mut write,
        command,
        &snapshot,
        review,
        reviewed,
        &origin,
        creation.as_ref(),
        repository,
        projects,
    ) {
        return outcome;
    }
    let state_cause = match append_review_statement(&mut write, &entry, statement, reviewed) {
        Ok(cause) => cause,
        Err(outcome) => return outcome,
    };
    write.workflow_head(entry.born().clone(), state_cause, move |_| workflow);
    write.claim_head(entry.born().clone(), state_cause, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let answer = review_answer(entry.handle().clone(), verdict, next);
    if let Err(error) = write.command_result(
        request(command, review.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("review::receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn append_reviewed_fact(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    verdict: body::Verdict,
    finding: Option<String>,
    git: Option<body::GitBoundary>,
    source: body::SourceBoundary,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    write
        .fact(
            body::TaskReviewed {
                task: entry.born().clone(),
                reviewer: command.actor().clone(),
                verdict,
                finding,
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
        body::Verdict::Clean => workflow.review = "clean".to_owned(),
        body::Verdict::Finding => {
            workflow.status = "pending".to_owned();
            workflow.review = "unassigned".to_owned();
            workflow.assignment = None;
            workflow.completion = None;
        }
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
    let notice = task_edit_notice(entry, &statement);
    let edited = write
        .fact(
            body::TaskEdited {
                task: entry.born().clone(),
                revision,
                notice,
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
    verdict: body::Verdict,
    next: Option<Box<str>>,
) -> wire::intent::TaskAnswer {
    wire::intent::TaskAnswer {
        task,
        status: match verdict {
            body::Verdict::Clean => "completed",
            body::Verdict::Finding => "pending",
        }
        .to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        claim: None,
        next,
    }
}

struct DocPlan {
    preview: board::Preview,
    snapshot: ValidatedSnapshot,
    touched: Vec<TaskHandle>,
    handles: std::collections::BTreeMap<usize, TaskHandle>,
}

struct DocTaskSpec {
    task: TaskHandle,
    born: wire::TaskStamp,
    title: String,
    description: Option<String>,
    annotations: Vec<String>,
    project: String,
    acceptance: Vec<String>,
    tags: Vec<String>,
    dependencies: Vec<wire::TaskStamp>,
    flow: Vec<wire::Phase>,
    initial: wire::Phase,
    taste_review: Option<TasteReview>,
}

pub(super) fn doc_apply(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let plan = match validate_doc_plan(bus, command, apply, projects, allocation) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    if let Some(outcome) = repeated_doc_apply(bus, apply, &plan.touched) {
        return outcome;
    }
    commit_doc_plan(bus, command, apply, repository, &plan)
}

fn validate_doc_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<DocPlan, Outcome<CommandAnswer>> {
    let preview = board::Preview::classify(&apply.document).map_err(|error| {
        refusal(
            "doc::classification-refused",
            Some(error.to_string()),
            "preview the current canonical document before applying it",
        )
    })?;
    if preview.digest() != &apply.preview {
        return Err(refusal(
            "doc::preview-moved",
            Some(apply.preview.to_string()),
            &format!("apply the document digest {}", preview.digest()),
        ));
    }
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    require_doc_barrier(command, apply, &snapshot)?;
    let project = preview
        .document()
        .project
        .as_deref()
        .unwrap_or("task.document");
    projects.admit(project).map_err(|error| {
        refusal(
            "doc::project-refused",
            Some(error.to_string()),
            "author a project admitted by the captured task metadata",
        )
    })?;
    let touched = document_handles(
        preview.document(),
        &snapshot.answer,
        jiff::Timestamp::now().as_microsecond(),
    )?;
    if touched.is_empty() {
        return Err(refusal(
            "doc::empty",
            None,
            "author at least one task node before applying the document",
        ));
    }
    let handles = preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
        .map(|node| node.id)
        .zip(touched.iter().cloned())
        .collect();
    Ok(DocPlan {
        preview,
        snapshot,
        touched,
        handles,
    })
}

fn require_doc_barrier(
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    snapshot: &ValidatedSnapshot,
) -> Result<(), Outcome<CommandAnswer>> {
    let barrier = board_barrier(&snapshot.answer);
    let preview = command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Preview { digest } if digest == &apply.preview));
    let board = command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Barrier { name, digest }
                if name.as_str() == "board" && digest == &barrier
        )
    });
    if command.expected().len() == 2 && preview && board {
        return Ok(());
    }
    Err(refusal(
        "doc::barrier-moved",
        Some(barrier.to_string()),
        "preview the current document against the current board and apply that exact digest",
    ))
}

fn repeated_doc_apply(
    bus: &Bus,
    apply: &DocApply,
    touched: &[TaskHandle],
) -> Option<Outcome<CommandAnswer>> {
    let snapshot = bus.task_snapshot().ok()?;
    let existing = snapshot.facts().iter().find(|envelope| {
        envelope.fact.kind() == wire::FactKind::DocApplied
            && envelope
                .fact
                .body()
                .get("digest")
                .and_then(serde_json::Value::as_str)
                == Some(apply.preview.as_str())
    })?;
    Some(Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: existing.seq,
            last: existing.seq,
        }),
        body: CommandAnswer::Doc(wire::intent::DocAnswer {
            preview: apply.preview.clone(),
            touched: touched.to_vec(),
        }),
    }))
}

fn commit_doc_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    repository: Option<&Repository>,
    plan: &DocPlan,
) -> Outcome<CommandAnswer> {
    let intrinsic = match format!("doc:{}", apply.preview).parse::<wire::Origin>() {
        Ok(origin) => origin,
        Err(error) => {
            return protocol_fault(
                "doc::origin-invalid",
                &format!("preview digest did not form a document origin: {error}"),
            );
        }
    };
    let origin =
        match resolve_creation_origin(None, command.actor(), &plan.snapshot, Some(intrinsic)) {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let creates = plan.preview.document().nodes.iter().any(|node| {
        node.kind != dialect::NodeKind::SyntheticRoot
            && plan
                .handles
                .get(&node.id)
                .is_some_and(|handle| task_entry(&plan.snapshot.entries, handle).is_none())
    });
    let creation = if creates {
        match task_creation_capture(bus, command, &plan.snapshot) {
            Ok(creation) => Some(creation),
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    if let Some(creation) = &creation {
        creation.guard(&mut write);
    }
    let mut previous = None;
    let mut operations = 0_u32;
    for node in plan
        .preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
    {
        let spec = match doc_task_spec(plan, command, repository, node) {
            Ok(spec) => spec,
            Err(outcome) => return outcome,
        };
        previous =
            match write_doc_task(&mut write, plan, spec, &origin, creation.as_ref(), previous) {
                Ok(fact) => Some(fact),
                Err(outcome) => return outcome,
            };
        operations = operations.saturating_add(1);
    }
    finish_doc_commit(bus, command, apply, plan, write, previous, operations)
}

fn doc_task_spec(
    plan: &DocPlan,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    node: &dialect::Node,
) -> Result<DocTaskSpec, Outcome<CommandAnswer>> {
    let task = plan
        .handles
        .get(&node.id)
        .expect("every applied node was assigned a handle")
        .clone();
    let dependencies = plan
        .preview
        .document()
        .edges
        .iter()
        .filter(|edge| edge.source == node.id)
        .filter_map(|edge| plan.handles.get(&edge.target))
        .map(|handle| {
            handle
                .born()
                .expect("document handles carry canonical task stamps")
        })
        .collect::<Vec<_>>();
    let project = plan
        .preview
        .document()
        .project
        .clone()
        .unwrap_or_else(|| "task.document".to_owned());
    validate_acceptance(&node.acceptance)?;
    if node.tags.iter().any(|tag| tag == TASTE_REVIEW_TAG) {
        return Err(refusal(
            "doc::tag-reserved",
            Some(TASTE_REVIEW_TAG.to_owned()),
            "remove the system-owned taste-review marker from the authored document",
        ));
    }
    let mut flow = if node.flow.is_empty() {
        task_flow(None, &node.acceptance)?
    } else {
        node.flow
            .iter()
            .map(|station| phase(station.as_str()))
            .collect()
    };
    let existing = task_entry(&plan.snapshot.entries, &task);
    if existing.is_none() {
        let _dependency_witnesses =
            derive_dependency_flow(&mut flow, &dependencies, &plan.snapshot)?;
    }
    let taste_review = task_taste_review(
        repository,
        &plan.snapshot,
        command.actor(),
        &node.title,
        None,
        &node.acceptance,
    )?;
    let was_required = existing.is_some_and(|entry| has_taste_review(&entry.statement().tags));
    let taste_review_required = taste_review.as_ref().is_some_and(TasteReview::required);
    if taste_review.is_none() && was_required {
        return Err(environment_fault(
            "task::taste-policy-unavailable",
            "an existing taste review cannot clear without repository policy authority",
        ));
    }
    if taste_review_required && existing.is_none() {
        prepend_plan(&mut flow);
    }
    if taste_review_required
        && existing.is_some_and(|entry| {
            !has_taste_review(&entry.statement().tags) && entry.workflow().phase.as_str() != "plan"
        })
    {
        return Err(refusal(
            "task::taste-review-required",
            Some(task.to_string()),
            "revise the authored task statement so it passes taste review before applying it",
        ));
    }
    let initial = flow[0].clone();
    let mut tags = node.tags.clone();
    tags.push(document_slug_tag(&node.slug));
    if let Some(priority) = node.priority {
        tags.push(format!("priority:{priority}"));
    }
    if taste_review_required {
        require_taste_review(&mut tags);
    }
    Ok(DocTaskSpec {
        born: task
            .born()
            .expect("document handles carry canonical task stamps"),
        task,
        title: node.title.clone(),
        description: (!node.description.is_empty()).then(|| node.description.clone()),
        annotations: node.annotations.clone(),
        project,
        acceptance: node.acceptance.clone(),
        tags,
        dependencies,
        flow,
        initial,
        taste_review: (was_required || taste_review_required)
            .then(|| taste_review.expect("repository-backed review has a verdict")),
    })
}

fn write_doc_task(
    write: &mut JournalWrite,
    plan: &DocPlan,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    creation: Option<&TaskCreationCapture>,
    previous: Option<PendingFact>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let task = spec.born.clone();
    let annotations = spec.annotations.clone();
    let cause = previous.map_or(FactCause::Root, FactCause::Pending);
    let existing = plan
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == &spec.born);
    let written = match existing {
        Some(entry) => edit_doc_task(write, entry, spec, cause),
        None => add_doc_task(write, spec, origin, creation, cause),
    }?;
    append_doc_annotations(write, plan, &task, annotations, written)
}

fn append_doc_annotations(
    write: &mut JournalWrite,
    plan: &DocPlan,
    task: &wire::TaskStamp,
    annotations: Vec<String>,
    mut cause: PendingFact,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let mut retained = plan
        .snapshot
        .facts
        .iter()
        .filter_map(|envelope| envelope.fact.read::<body::TaskNoted>())
        .filter(|noted| &noted.task == task)
        .map(|noted| noted.note)
        .collect::<std::collections::BTreeSet<_>>();
    for note in annotations {
        if !retained.insert(note.clone()) {
            continue;
        }
        cause = write
            .fact(
                body::TaskNoted {
                    task: task.clone(),
                    note,
                },
                FactCause::Pending(cause),
            )
            .map_err(|error| store_fault("doc::annotation-build", &error.to_string()))?;
    }
    Ok(cause)
}

fn edit_doc_task(
    write: &mut JournalWrite,
    existing: &Entry,
    spec: DocTaskSpec,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    if existing.workflow().status != "pending" {
        return Err(refusal(
            "doc::settled-task",
            Some(spec.task.to_string()),
            "apply authored edits only to pending task statements",
        ));
    }
    let taste_review = spec.taste_review.clone();
    let revision = u32::try_from(existing.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let mut statement = existing.statement().clone();
    statement.title = spec.title;
    statement.description = spec.description;
    statement.project = spec.project;
    statement.acceptance = spec.acceptance;
    statement.tags = spec.tags;
    statement.dependencies = spec.dependencies;
    let notice = task_edit_notice(existing, &statement);
    let edited = write
        .fact(
            body::TaskEdited {
                task: existing.born().clone(),
                revision,
                notice,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::edit-build", &error.to_string()))?;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    if let Some(review) = taste_review.as_ref() {
        return append_taste_review_audit(
            write,
            existing.born(),
            review,
            FactCause::Pending(edited),
        );
    }
    Ok(edited)
}

fn add_doc_task(
    write: &mut JournalWrite,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    creation: Option<&TaskCreationCapture>,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let creation = creation.ok_or_else(|| {
        protocol_fault(
            "doc::creation-context-missing",
            "a newly authored document task has no creation-time transcript capture",
        )
    })?;
    let taste_review = spec.taste_review.clone();
    let audit_task = spec.born.clone();
    let added = write
        .fact(
            body::TaskAdded {
                task: spec.born.clone(),
                title: spec.title.clone(),
                description: spec.description.clone(),
                acceptance: spec.acceptance.clone(),
                requirements: wire::EnvironmentRequirements::default(),
                project: spec.project.clone(),
                tags: spec.tags.clone(),
                depends: spec.dependencies.clone(),
                flow: spec.flow.clone(),
                phase: spec.initial.clone(),
                revision: 1,
                creation: creation.context(),
            },
            cause,
        )
        .map_err(|error| store_fault("doc::add-build", &error.to_string()))?;
    let statement_task = spec.born.clone();
    let statement_origin = origin.clone();
    let statement_creation = creation.context();
    write.task_head(added, move |position| TaskHead {
        born: statement_task,
        queue_origin: position,
        title: spec.title,
        description: spec.description,
        acceptance: spec.acceptance,
        requirements: wire::EnvironmentRequirements::default(),
        project: spec.project,
        tags: spec.tags,
        dependencies: spec.dependencies,
        provenance: statement_origin,
        creation: statement_creation,
        revision: position,
    });
    write.workflow_head(spec.born, added, move |_| WorkflowHead {
        flow: spec.flow,
        phase: spec.initial,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
    if let Some(review) = taste_review.as_ref() {
        return append_taste_review_audit(write, &audit_task, review, FactCause::Pending(added));
    }
    Ok(added)
}

fn finish_doc_commit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    plan: &DocPlan,
    mut write: JournalWrite,
    previous: Option<PendingFact>,
    operations: u32,
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.fact(
        body::DocApplied {
            digest: apply.preview.clone(),
            barrier: plan.snapshot.cursor,
            operations,
        },
        previous.map_or(FactCause::Root, FactCause::Pending),
    ) {
        return store_fault("doc::apply-build", &error.to_string());
    }
    let answer = wire::intent::DocAnswer {
        preview: apply.preview.clone(),
        touched: plan.touched.clone(),
    };
    if let Err(error) = write.command_result(
        request(command, apply.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("doc::apply-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Doc(answer))
}
