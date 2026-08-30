use super::*;

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
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &task.task)
    else {
        return refusal(
            "task::unknown",
            Some(task.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let Some(claim) = entry.claim() else {
        return refusal(
            "claim::head-missing",
            Some(task.task.to_string()),
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
    let mut next = claim.clone();
    next.disposition = ClaimDisposition::Freed;
    write.claim_head(entry.born().clone(), freed, move |fact| ClaimHead {
        fact,
        ..next
    });
    if entry.workflow().phase.as_str() == "review"
        && entry.workflow().assignment.as_ref() == Some(command.actor())
    {
        let mut workflow = entry.workflow().clone();
        workflow.assignment = None;
        workflow.review = "unassigned".to_owned();
        write.workflow_head(entry.born().clone(), freed, move |_| workflow);
    }
    let answer = wire::intent::ClaimAnswer {
        task: task.task.clone(),
        holder: command.actor().clone(),
        warnings: Vec::new(),
        next: None,
    };
    if let Err(error) = write.command_result(
        request(command, task.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::free-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
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
    let (until, lease) = match reclaim_window(entry, &snapshot, command, renew, at) {
        Ok(window) => window,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::BusTimer);
    guard(&mut write, command.expected().to_vec());
    let renewed = match write.fact(
        body::ClaimRenewed {
            task: entry.born().clone(),
            epoch: command.epoch(),
            until,
            maximum: lease.maximum,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("claim::reclaim-build", &error.to_string()),
    };
    let mut next = lease.clone();
    next.renewal = at;
    next.expiry = until;
    write.lease_head(entry.born().clone(), renewed, move |_| next);
    let answer = wire::intent::ClaimAnswer {
        task: renew.task.clone(),
        holder: command.actor().clone(),
        warnings: Vec::new(),
        next: None,
    };
    if let Err(error) = write.command_result(
        request(command, renew.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::reclaim-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

fn reclaim_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    renew: &ClaimRenew,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &renew.task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(renew.task.to_string()),
                "read board and reclaim one current task handle",
            )
        })
}

fn reclaim_window(
    entry: &Entry,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    at: Instant,
) -> Result<(Instant, LeaseHead), Outcome<CommandAnswer>> {
    let claim = entry.claim().ok_or_else(|| {
        refusal(
            "claim::head-missing",
            Some(renew.task.to_string()),
            "reclaim only the current lane's active task",
        )
    })?;
    let lease = entry.lease().ok_or_else(|| {
        protocol_fault(
            "claim::lease-missing",
            "an active claim head has no durable lease head",
        )
    })?;
    let Actor::Lane(lane) = command.actor() else {
        return Err(refusal(
            "claim::lane-required",
            Some(command.actor().to_string()),
            "reclaim through the lane capability that owns the claim",
        ));
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *command.actor() {
        return Err(refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "reclaim only an active claim owned by this lane capability",
        ));
    }
    board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            "reclaim before lapse; use task next after reallocation",
        )
    })?;
    admit_reclaim_residency(snapshot, command, renew, claim, lease, *lane)?;
    let seconds = i64::try_from(renew.lease_seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            refusal(
                "claim::lease-seconds-invalid",
                Some(renew.lease_seconds.to_string()),
                "request a positive representable lease duration",
            )
        })?;
    let until = std::cmp::min(instant_after(at, seconds)?, lease.maximum);
    if until <= lease.expiry {
        return Err(refusal(
            "claim::lease-nonextending",
            Some(lease.expiry.to_string()),
            "request an extension before the durable maximum",
        ));
    }
    Ok((until, lease.clone()))
}

fn admit_reclaim_residency(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    claim: &ClaimHead,
    lease: &LeaseHead,
    lane: wire::CanonicalUuid,
) -> Result<(), Outcome<CommandAnswer>> {
    let residency = &snapshot.answer.residency.body;
    let seated = residency.lanes.iter().find(|seat| seat.lane == lane);
    let exact = claim.lane == lane
        && claim.epoch == command.epoch()
        && lease.epoch == command.epoch()
        && residency.epoch == command.epoch()
        && seated.is_some_and(|seat| {
            seat.disposition == "occupied" && seat.occupant == Some(claim.occupant)
        });
    if exact {
        return Ok(());
    }
    Err(refusal(
        "claim::residency-moved",
        Some(renew.task.to_string()),
        "read current lane residency and reclaim its exact active claim",
    ))
}
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
                Some(done.task.to_string()),
                "retry with a current client that supplies one durable completion operation",
            );
        }
    };
    match replay_task_completion(bus, command, done, entry, operation) {
        Ok(Some(replayed)) => return replayed,
        Ok(None) => {}
        Err(outcome) => return outcome,
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
    let resume_epoch = match resumable_task_completion(repository, &snapshot, command, entry, done)
    {
        Ok(resume) => resume,
        Err(outcome) => return outcome,
    };
    let (at, lane, transition) = match done_transition(&snapshot, entry, command, resume_epoch) {
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
    snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &done.task)
        .ok_or_else(|| {
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

fn replay_task_completion(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    entry: &Entry,
    operation: wire::CanonicalUuid,
) -> Result<Option<Outcome<CommandAnswer>>, Outcome<CommandAnswer>> {
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| store_fault("task::completion-replay-unavailable", &error.to_string()))?;
    let facts = snapshot.facts();
    let terminal = facts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, envelope)| {
            let completed = envelope.fact.read::<body::TaskDone>()?;
            let terminal = completed.terminal?;
            (envelope.actor == *command.actor()
                && completed.task == *entry.born()
                && completed.validation == done.validation
                && terminal.operation == operation)
                .then_some((index, envelope.seq, terminal))
        });
    Ok(terminal.map(|(index, first, terminal)| {
        let last = facts
            .get(index + 1)
            .and_then(|following| {
                let phased = following.fact.read::<body::TaskPhased>()?;
                (following.cause == Some(first) && phased.task == *entry.born())
                    .then_some(following.seq)
            })
            .unwrap_or(first);
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange { first, last }),
            body: CommandAnswer::Task(wire::intent::TaskAnswer {
                task: entry.handle().clone(),
                status: terminal.status,
                completion: None,
                learning: None,
                next: terminal.next,
            }),
        })
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
            concat!(
                "win the task again under the current resident ",
                "generation",
            ),
        ));
    }
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
    if !completes_plan && !entry.statement().acceptance.is_empty() {
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
        if child.workflow.body.status == "pending" && !child.statement.body.acceptance.is_empty() {
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
        task: done.task.clone(),
        status,
        completion: None,
        learning: None,
        next,
    })
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
    next_action(snapshot, &post, false)
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
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &review.task)
    else {
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
            Some(review.task.to_string()),
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
    let transition = board::Transition::complete(entry, &active, at).map_err(|error| {
        refusal(
            "review::phase-refused",
            Some(error.to_string()),
            "review only the final assigned workflow phase",
        )
    })?;
    Ok((entry, transition, at, active.lane()))
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

pub(super) fn task_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let (entry, transition, at, lane) = match admit_review(&snapshot, command, review) {
        Ok(admitted) => admitted,
        Err(outcome) => return outcome,
    };
    let source = match bus.source_boundary(&lane) {
        Ok(source) => source,
        Err(error) => return store_fault("review::boundary-unreadable", &error.to_string()),
    };
    let (verdict, finding) = match review_verdict(review) {
        Ok(decided) => decided,
        Err(outcome) => return outcome,
    };
    let parent = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let origin = match resolve_creation_origin(None, command.actor(), &snapshot, Some(parent)) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let git = match publish_task_boundary(repository, &snapshot, command, entry) {
        Ok(git) => git,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    let reviewed = match write.fact(
        body::TaskReviewed {
            task: entry.born().clone(),
            reviewer: command.actor().clone(),
            verdict,
            finding,
            git,
            source,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("review::build", &error.to_string()),
    };
    let mut workflow = transition.next().clone();
    workflow.review = match verdict {
        body::Verdict::Clean => "clean",
        body::Verdict::Finding => "finding",
    }
    .to_owned();
    write.workflow_head(entry.born().clone(), reviewed, move |_| workflow);
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(entry.born().clone(), reviewed, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let tracking = record_followups(
        &mut write, command, &snapshot, review, reviewed, &origin, projects,
    );
    if let Err(outcome) = tracking {
        return outcome;
    }
    let answer = wire::intent::TaskAnswer {
        task: review.task.clone(),
        status: "completed".to_owned(),
        completion: None,
        learning: None,
        next: None,
    };
    if let Err(error) = write.command_result(
        request(command, review.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("review::receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
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
    project: String,
    acceptance: Vec<String>,
    tags: Vec<String>,
    dependencies: Vec<wire::TaskStamp>,
    flow: Vec<wire::Phase>,
    initial: wire::Phase,
}

pub(super) fn doc_apply(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
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
    commit_doc_plan(bus, command, apply, &plan)
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
        jiff::Timestamp::now().as_millisecond(),
    );
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
    let facts = bus.fact_snapshot().ok()?;
    let existing = facts.facts().iter().find(|envelope| {
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
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    let mut previous = None;
    let mut operations = 0_u32;
    for node in plan
        .preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
    {
        let spec = match doc_task_spec(plan, node) {
            Ok(spec) => spec,
            Err(outcome) => return outcome,
        };
        previous = match write_doc_task(&mut write, plan, spec, &origin, previous) {
            Ok(fact) => Some(fact),
            Err(outcome) => return outcome,
        };
        operations = operations.saturating_add(1);
    }
    finish_doc_commit(bus, command, apply, plan, write, previous, operations)
}

fn doc_task_spec(
    plan: &DocPlan,
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
        .collect();
    let project = plan
        .preview
        .document()
        .project
        .clone()
        .unwrap_or_else(|| "task.document".to_owned());
    let flow = if node.flow.is_empty() {
        task_flow(None, &node.acceptance)?
    } else {
        node.flow
            .iter()
            .map(|station| phase(station.as_str()))
            .collect()
    };
    let initial = flow[0].clone();
    let mut tags = node.tags.clone();
    tags.push(document_slug_tag(&node.slug));
    if let Some(priority) = node.priority {
        tags.push(format!("priority:{priority}"));
    }
    Ok(DocTaskSpec {
        born: task
            .born()
            .expect("document handles carry canonical task stamps"),
        task,
        title: node.title.clone(),
        project,
        acceptance: node.acceptance.clone(),
        tags,
        dependencies,
        flow,
        initial,
    })
}

fn write_doc_task(
    write: &mut JournalWrite,
    plan: &DocPlan,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    previous: Option<PendingFact>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let cause = previous.map_or(FactCause::Root, FactCause::Pending);
    let existing = plan
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &spec.task);
    match existing {
        Some(entry) => edit_doc_task(write, entry, spec, cause),
        None => add_doc_task(write, spec, origin, cause),
    }
}

fn edit_doc_task(
    write: &mut JournalWrite,
    existing: &Entry,
    spec: DocTaskSpec,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    if existing.workflow().status == "completed" {
        return Err(refusal(
            "doc::settled-task",
            Some(spec.task.to_string()),
            "apply authored edits only to pending task statements",
        ));
    }
    let revision = u32::try_from(existing.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                task: existing.born().clone(),
                revision,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::edit-build", &error.to_string()))?;
    let mut statement = existing.statement().clone();
    statement.title = spec.title;
    statement.project = spec.project;
    statement.acceptance = spec.acceptance;
    statement.tags = spec.tags;
    statement.dependencies = spec.dependencies;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    Ok(edited)
}

fn add_doc_task(
    write: &mut JournalWrite,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let added = write
        .fact(
            body::TaskAdded {
                task: spec.born.clone(),
                title: spec.title.clone(),
                description: None,
                acceptance: spec.acceptance.clone(),
                project: spec.project.clone(),
                tags: spec.tags.clone(),
                depends: spec.dependencies.clone(),
                flow: spec.flow.clone(),
                phase: spec.initial.clone(),
                revision: 1,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::add-build", &error.to_string()))?;
    let statement_task = spec.born.clone();
    let statement_origin = origin.clone();
    write.task_head(added, move |position| TaskHead {
        born: statement_task,
        queue_origin: position,
        title: spec.title,
        description: None,
        acceptance: spec.acceptance,
        project: spec.project,
        tags: spec.tags,
        dependencies: spec.dependencies,
        provenance: statement_origin,
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
    });
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
