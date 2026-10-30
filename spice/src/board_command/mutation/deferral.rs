use super::*;

/// Defer one filed pending task. Another actor's deferral of an open Design a
/// lane holds asks that holder to park in the same write; the returned lane is
/// the one asked.
pub(crate) fn task_defer(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    defer: &TaskDefer,
    repository: Option<Host<'_>>,
    config: &steer::Config,
) -> (Outcome<CommandAnswer>, Option<wire::CanonicalUuid>) {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&defer.task),
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return (outcome, None),
    };
    change_deferral(
        bus,
        command,
        &snapshot,
        DeferralRequest {
            task: &defer.task,
            deferral: Some(defer.wait),
            claim_requested: false,
            stop: Some(config),
        },
        defer.clone(),
        repository,
    )
}

/// Wake one ordinary filed task whose deferral is currently set.
pub(in crate::board_command) fn task_wake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    wake: &TaskWake,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&wake.task),
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    change_deferral(
        bus,
        command,
        &snapshot,
        DeferralRequest {
            task: &wake.task,
            deferral: None,
            claim_requested: wake.claim,
            stop: None,
        },
        wake.clone(),
        repository,
    )
    .0
}

/// File one unfiled Draft into ordinary work.
pub(in crate::board_command) fn task_file(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    file: &TaskFile,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&file.task),
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    file_unfiled(bus, command, &snapshot, file, repository)
}

fn validate_filing_entry(
    entry: &Entry,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let current_statement = Witness::Task {
        task: entry.born().clone(),
        head: entry.statement().revision,
    };
    if !command.expected().contains(&current_statement) {
        return Err(refusal(
            "task::filing-statement-witness-moved",
            Some(format!("{} current={current_statement:?}", entry.handle())),
            "read the board again and retry with the current target statement witness",
        ));
    }
    if !entry.is_draft() {
        return Err(refusal(
            "task::filing-refused",
            Some(format!("{} project={}", entry.handle(), entry.project())),
            "use task file only for an unfiled Draft",
        ));
    }
    if !entry.workflow().status.is_pending() {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "file only a pending Draft",
        ));
    }
    Ok(())
}

/// Validate a Draft as the complete ordinary task it becomes when filed, and
/// return the prerequisite workflows the filing transaction relies on.
///
/// The statement is the filed row on success, held for taste review when the
/// policy matches its prose; the workflow is never moved. Every refusal names
/// the Draft field or filing option that must be repaired, and nothing is
/// written.
fn draft_filing(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    repository: Option<Host<'_>>,
    file: &TaskFile,
    statement: &mut TaskHead,
    workflow: &WorkflowHead,
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    if let Some(named) = &file.preparation {
        if file.claim {
            return Err(refusal(
                "task::filing-claim-controlled",
                Some(file.task.to_string()),
                "file controlled work without --claim; selected authorization must precede native capture",
            ));
        }
        let parent = task_heads(&snapshot.answer, named)?;
        planning::bind_preparation(command, parent, statement)?;
    }
    planning::admit_design_flow(statement.intent, &workflow.flow)?;
    validate_acceptance(&statement.acceptance)?;
    validate_pitch(&statement.pitch)?;
    let mut dependency_witnesses = Vec::with_capacity(statement.dependencies.len());
    for born in &statement.dependencies {
        let Some(heads) = snapshot
            .answer
            .tasks
            .iter()
            .find(|heads| &heads.statement.body.born == born)
        else {
            return Err(refusal(
                "task::draft-dependency-missing",
                Some(format!("{} dependency={born}", file.task)),
                &format!(
                    "remove the missing prerequisite with spice task depends {} HANDLE --remove",
                    file.task,
                ),
            ));
        };
        if !heads.statement.body.filing.is_filed() {
            return Err(refusal(
                "task::dependency-unfiled",
                Some(heads.statement.body.handle.to_string()),
                "file every Draft prerequisite first with spice task file HANDLE",
            ));
        }
        dependency_witnesses.push(workflow_witness(heads));
    }
    planning::refuse_disposed_construction(snapshot, statement, workflow)?;
    let review = task_taste_review(
        repository,
        snapshot,
        command.actor(),
        &statement.title,
        &statement.pitch,
        statement.description.as_deref(),
        &statement.acceptance,
    )?;
    if let Some(review) = review.filter(TasteReview::required) {
        keep_taste_hold(&mut statement.taste, &review);
    }
    Ok(dependency_witnesses)
}

fn append_filing<'a>(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    post: &Entry,
    workflow: WorkflowHead,
    statement: TaskHead,
    atomic_claim: Option<AtomicClaim<'a>>,
) -> Result<Option<spice::task_git::ClaimPreparation<'a>>, Outcome<CommandAnswer>> {
    let filed = write
        .fact(
            body::TaskFiled {
                task: entry.born().clone(),
                project: statement.project.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("task::filing-build", &error.to_string()))?;
    // A Draft's preparation binds as the ordinary output association `task
    // add --preparation` writes.
    let statement_fact = filed;
    write.workflow_head(entry.born().clone(), filed, move |_| workflow);
    write.task_head(statement_fact, move |position| {
        published_statement(
            TaskHead {
                revision: position,
                ..statement
            },
            position,
        )
    });
    match atomic_claim {
        Some(claim) => claim.prepared.append(write, command, post),
        None => Ok(None),
    }
}

/// Project a filing lands in: the explicit admitted destination or the
/// unfiled row's current project.
fn filing_destination(
    entry: &Entry,
    requested: Option<&str>,
) -> Result<board::Project, Outcome<CommandAnswer>> {
    board::Project::admit(requested.unwrap_or_else(|| entry.project().as_str())).map_err(|error| {
        refusal(
            "task::filing-project-refused",
            Some(error.to_string()),
            PROJECT_REPAIR,
        )
    })
}

fn file_unfiled(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
    file: &TaskFile,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let Some(entry) = task_entry(&snapshot.entries, &file.task) else {
        return refusal(
            "task::unknown",
            Some(file.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if let Err(outcome) = validate_filing_entry(entry, command) {
        return outcome;
    }
    let project = match filing_destination(entry, file.into.as_deref()) {
        Ok(project) => project,
        Err(outcome) => return outcome,
    };
    let mut statement = entry.statement().clone();
    statement.project = project.as_str().to_owned();
    statement.filing = wire::TaskFiling::Filed;
    let mut workflow = entry.workflow().clone();
    workflow.deferral = None;
    let dependency_witnesses = match draft_filing(
        snapshot,
        command,
        repository,
        file,
        &mut statement,
        &workflow,
    ) {
        Ok(witnesses) => witnesses,
        Err(outcome) => return outcome,
    };
    let post = match Entry::new(
        statement.clone(),
        workflow.clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
    ) {
        Ok(post) => post
            .with_grouped(entry.grouped())
            .with_design(entry.design()),
        Err(error) => {
            return refusal(
                "task::filing-refused",
                Some(error.to_string()),
                "file the row into a valid project with a valid flow and dependencies",
            );
        }
    };
    let atomic_claim =
        match prepare_mutation_claim(bus, command, repository, snapshot, &post, file.claim) {
            Ok(claim) => claim,
            Err(outcome) => return outcome,
        };
    let claim_answer = atomic_claim.as_ref().map(|claim| claim.answer.clone());
    let next = claim_answer
        .as_ref()
        .and_then(|claim| claim.next.clone())
        .or_else(|| next_action(snapshot, &post));
    let mut write = match prepare_mutation_write(command, entry, snapshot, atomic_claim.as_ref()) {
        Ok(write) => write,
        Err(outcome) => return outcome,
    };
    guard(&mut write, dependency_witnesses);
    let prepared_claim_git = match append_filing(
        &mut write,
        command,
        entry,
        &post,
        workflow,
        statement,
        atomic_claim,
    ) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let answer = wire::intent::TaskAnswer {
        taste: answered_taste(command.actor(), post.statement().taste.as_ref()),
        completion_contract: Some(
            completion_contract(
                &post.statement().acceptance,
                post.born(),
                &post.statement().dependencies,
                post.statement().intent,
                snapshot.entries.iter(),
            )
            .into_boxed_str(),
        ),
        task: post.handle().clone(),
        status: "filed".to_owned(),
        completion: None,
        claim: claim_answer,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, file.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        if let Some(prepared) = prepared_claim_git
            && let Err(rollback) = prepared.rollback()
        {
            return task_git_outcome(rollback);
        }
        return store_fault("task::filing-receipt", &error.to_string());
    }
    let body = CommandAnswer::Task(Box::new(answer));
    if let Some(prepared) = prepared_claim_git {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Claim(prepared))
    } else {
        committed(bus, write, body)
    }
}

fn append_deferral_fact(
    write: &mut JournalWrite,
    entry: &Entry,
    deferral: Option<Instant>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let fact = if let Some(until) = deferral {
        write.fact(
            body::TaskDeferred {
                task: entry.born().clone(),
                until,
            },
            FactCause::Root,
        )
    } else {
        write.fact(
            body::TaskWoken {
                task: entry.born().clone(),
            },
            FactCause::Root,
        )
    };
    fact.map_err(|error| store_fault("task::deferral-build", &error.to_string()))
}

struct DeferralTransition {
    workflow: WorkflowHead,
    post: Option<Entry>,
    releases_claim: bool,
}

fn deferral_transition(
    entry: &Entry,
    command: &wire::AdmittedCommand,
    task: &TaskHandle,
    deferral: Option<Instant>,
) -> Result<DeferralTransition, Outcome<CommandAnswer>> {
    // Settled work never becomes workable again.
    if !entry.workflow().status.is_pending() {
        return Err(refusal(
            "task::settled",
            Some(task.to_string()),
            "defer or wake only pending work",
        ));
    }
    if entry.is_draft() {
        return Err(refusal(
            "task::draft-unfiled",
            Some(filing_state(entry)),
            &format!(
                "file the Draft with spice task file {} before deferring or waking it",
                entry.handle(),
            ),
        ));
    }
    let releases_claim = deferral.is_some()
        && entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Active && claim.actor == *command.actor()
        });
    let mut workflow = entry.workflow().clone();
    workflow.deferral = deferral;
    if releases_claim
        && workflow.phase.as_str() == "review"
        && workflow.status.reviewer() == Some(command.actor())
    {
        workflow.status = wire::WorkflowStatus::pending();
    }
    let post = if deferral.is_none() {
        Some(
            Entry::new(
                entry.statement().clone(),
                workflow.clone(),
                entry.claim().cloned(),
                entry.lease().cloned(),
            )
            .map_err(|error| {
                refusal(
                    "task::deferral-refused",
                    Some(error.to_string()),
                    "wake a task into a valid pending state",
                )
            })?
            .with_grouped(entry.grouped())
            .with_design(entry.design()),
        )
    } else {
        None
    };
    Ok(DeferralTransition {
        workflow,
        post,
        releases_claim,
    })
}

fn prepare_deferral_release<'a>(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    repository: Option<Host<'a>>,
    releases_claim: bool,
) -> Result<Option<spice::task_git::ParkedRelease<'a>>, Outcome<CommandAnswer>> {
    let Some(resident) = repository.filter(|_| releases_claim) else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, &resident)?;
    let claim = entry.claim().expect("release predicate requires a claim");
    let opening = snapshot
        .facts
        .at(claim.claimed)
        .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
        .and_then(|won| won.git);
    spice::task_git::prepare_free(&lane, resident.ambient, opening.as_ref())
        .map_err(task_git_outcome)
}

fn append_deferral_release(
    write: &mut JournalWrite,
    entry: &Entry,
    actor: &Actor,
    parked: Option<&spice::task_git::ParkedRelease>,
    releases_claim: bool,
) -> Result<(), Outcome<CommandAnswer>> {
    if !releases_claim {
        return Ok(());
    }
    let freed = write
        .fact(
            body::ClaimFreed {
                task: entry.born().clone(),
                actor: actor.clone(),
                parked: parked.map(|prepared| prepared.parked().clone()),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("task::defer-release-build", &error.to_string()))?;
    let mut released = entry
        .claim()
        .expect("release predicate requires a claim")
        .clone();
    released.disposition = ClaimDisposition::Freed;
    write.claim_head(entry.born().clone(), freed, move |position| ClaimHead {
        fact: position,
        ..released
    });
    Ok(())
}

fn finish_deferral<B: serde::Serialize>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    intent: B,
    mut write: JournalWrite,
    answer: wire::intent::TaskAnswer,
    parked: Option<spice::task_git::ParkedRelease>,
    prepared_claim: Option<spice::task_git::ClaimPreparation>,
) -> Outcome<CommandAnswer> {
    if let Err(error) =
        write.command_result(request(command, intent), command.epoch(), answer.clone())
    {
        let outcome = store_fault("task::deferral-receipt", &error.to_string());
        let outcome = rollback_release(parked, outcome);
        if let Some(prepared) = prepared_claim
            && let Err(rollback) = prepared.rollback()
        {
            return task_git_outcome(rollback);
        }
        return outcome;
    }
    let body = CommandAnswer::Task(Box::new(answer));
    if let Some(parked) = parked {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Release(parked))
    } else if let Some(prepared) = prepared_claim {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Claim(prepared))
    } else {
        committed(bus, write, body)
    }
}

/// What one defer or wake asks of its task: the new deferral, or none to
/// wake, whether the caller claims the task in the same write, and the
/// steering policy a deferral stops a held open Design's holder under.
struct DeferralRequest<'a> {
    task: &'a TaskHandle,
    deferral: Option<Instant>,
    claim_requested: bool,
    stop: Option<&'a steer::Config>,
}

/// Whether this deferral of `entry` stops a lane holding it: an open Design
/// deferred by anyone but its holder, whose own deferral releases it instead.
/// Ordinary work deferred by another actor stays held.
fn stops_holder(entry: &Entry, deferral: Option<Instant>, releases_claim: bool) -> bool {
    deferral.is_some()
        && !releases_claim
        && wire::intent::is_open_design(entry.statement().intent, entry.workflow())
}

fn change_deferral<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
    request: DeferralRequest<'_>,
    intent: B,
    repository: Option<Host<'_>>,
) -> (Outcome<CommandAnswer>, Option<wire::CanonicalUuid>) {
    let DeferralRequest {
        task,
        deferral,
        claim_requested,
        stop,
    } = request;
    let Some(entry) = task_entry(&snapshot.entries, task) else {
        let unknown = refusal(
            "task::unknown",
            Some(task.to_string()),
            "read board and name one current task handle",
        );
        return (unknown, None);
    };
    let DeferralTransition {
        workflow,
        post,
        releases_claim,
    } = match deferral_transition(entry, command, task, deferral) {
        Ok(transition) => transition,
        Err(outcome) => return (outcome, None),
    };
    let selected = post.as_ref().unwrap_or(entry);
    let atomic_claim = match prepare_mutation_claim(
        bus,
        command,
        repository,
        snapshot,
        selected,
        claim_requested,
    ) {
        Ok(claim) => claim,
        Err(outcome) => return (outcome, None),
    };
    let claim_answer = atomic_claim.as_ref().map(|claim| claim.answer.clone());
    let next = claim_answer
        .as_ref()
        .and_then(|claim| claim.next.clone())
        .or_else(|| post.as_ref().and_then(|post| next_action(snapshot, post)));
    let mut write = match prepare_mutation_write(command, entry, snapshot, atomic_claim.as_ref()) {
        Ok(write) => write,
        Err(outcome) => return (outcome, None),
    };
    let parked =
        match prepare_deferral_release(snapshot, command, entry, repository, releases_claim) {
            Ok(parked) => parked,
            Err(outcome) => return (outcome, None),
        };
    let fact = match append_deferral_fact(&mut write, entry, deferral) {
        Ok(fact) => fact,
        Err(outcome) => return (rollback_release(parked, outcome), None),
    };
    write.workflow_head(entry.born().clone(), fact, move |_| workflow);
    let stopped = match stop.filter(|_| stops_holder(entry, deferral, releases_claim)) {
        Some(config) => match Deactivation::of(bus, snapshot, entry)
            .and_then(|deactivation| deactivation.stage(bus, command, config, &mut write))
        {
            Ok(stopped) => stopped,
            Err(outcome) => return (rollback_release(parked, outcome), None),
        },
        None => None,
    };
    if let Err(outcome) = append_deferral_release(
        &mut write,
        entry,
        command.actor(),
        parked.as_ref(),
        releases_claim,
    ) {
        return (rollback_release(parked, outcome), None);
    }
    let prepared_claim = match atomic_claim {
        Some(claim) => match claim.prepared.append(&mut write, command, selected) {
            Ok(prepared) => prepared,
            Err(outcome) => return (rollback_release(parked, outcome), None),
        },
        None => None,
    };
    let answer = wire::intent::TaskAnswer {
        taste: None,
        completion_contract: None,
        task: entry.handle().clone(),
        status: if deferral.is_some() {
            "deferred".to_owned()
        } else {
            "pending".to_owned()
        },
        completion: None,
        claim: claim_answer,
        closed: Vec::new(),
        next,
    };
    let outcome = finish_deferral(bus, command, intent, write, answer, parked, prepared_claim);
    let committed = matches!(
        &outcome,
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(_),
            ..
        })
    );
    (outcome, stopped.filter(|_| committed))
}
