use super::*;
pub(in crate::board_command) fn task_defer(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    defer: &TaskDefer,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    change_deferral(
        bus,
        command,
        &defer.task,
        Some(defer.wait),
        defer.clone(),
        repository,
        false,
        projects,
        allocation,
    )
}

pub(in crate::board_command) fn task_wake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    wake: &TaskWake,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Some(project) = wake.into.as_deref() {
        return promote_oops(
            bus, command, wake, project, repository, projects, allocation,
        );
    }
    change_deferral(
        bus,
        command,
        &wake.task,
        None,
        wake.clone(),
        repository,
        wake.claim,
        projects,
        allocation,
    )
}

fn validate_promotion_entry(
    entry: &Entry,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let current_statement = Witness::Task {
        task: entry.born().clone(),
        head: entry.statement().revision,
    };
    if !command.expected().contains(&current_statement) {
        return Err(refusal(
            "task::promotion-statement-witness-moved",
            Some(format!("{} current={current_statement:?}", entry.handle())),
            "read the board again and retry with the current target statement witness",
        ));
    }
    if !is_oops(entry) {
        return Err(refusal(
            "task::promotion-refused",
            Some(format!("{} project={}", entry.handle(), entry.project())),
            "use --into only to promote a task currently parked in .oops",
        ));
    }
    if entry.workflow().completion.is_some() {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "promote only pending .oops work",
        ));
    }
    Ok(())
}

fn append_promotion(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    post: &Entry,
    workflow: WorkflowHead,
    statement: TaskHead,
    atomic_claim: Option<AtomicClaim>,
) -> Result<Option<spice::task_git::ClaimPreparation>, Outcome<CommandAnswer>> {
    let woken = write
        .fact(
            body::TaskWoken {
                task: entry.born().clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("task::promotion-wake-build", &error.to_string()))?;
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                task: entry.born().clone(),
                revision,
                notice: None,
            },
            FactCause::Pending(woken),
        )
        .map_err(|error| store_fault("task::promotion-edit-build", &error.to_string()))?;
    write.workflow_head(entry.born().clone(), woken, move |_| workflow);
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    match atomic_claim {
        Some(claim) => claim.prepared.append(write, command, post),
        None => Ok(None),
    }
}

fn promote_oops(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    wake: &TaskWake,
    requested: &str,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &wake.task) else {
        return refusal(
            "task::unknown",
            Some(wake.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if let Err(outcome) = validate_promotion_entry(entry, command) {
        return outcome;
    }
    let project = match projects.admit_manual_creation(requested) {
        Ok(project) => project,
        Err(error) => {
            return refusal(
                "task::promotion-project-refused",
                Some(error.to_string()),
                "name a public project this board admits for manual filing",
            );
        }
    };
    let mut statement = entry.statement().clone();
    statement.project = project.as_str().to_owned();
    let mut workflow = entry.workflow().clone();
    workflow.deferral = None;
    let post = match Entry::new(
        statement.clone(),
        workflow.clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    ) {
        Ok(post) => post,
        Err(error) => {
            return refusal(
                "task::promotion-refused",
                Some(error.to_string()),
                "promote the parked row into a valid public project",
            );
        }
    };
    let atomic_claim =
        match prepare_mutation_claim(bus, command, repository, &snapshot, &post, wake.claim) {
            Ok(claim) => claim,
            Err(outcome) => return outcome,
        };
    let claim_answer = atomic_claim.as_ref().map(|claim| claim.answer.clone());
    let next = claim_answer
        .as_ref()
        .and_then(|claim| claim.next.clone())
        .or_else(|| next_action(&snapshot, &post));
    let mut write = match prepare_mutation_write(command, entry, &snapshot, atomic_claim.as_ref()) {
        Ok(write) => write,
        Err(outcome) => return outcome,
    };
    let prepared_claim_git = match append_promotion(
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
        task: post.handle().clone(),
        status: "promoted".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        claim: claim_answer,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, wake.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        if let Some(prepared) = prepared_claim_git {
            if let Err(rollback) = prepared.rollback() {
                return task_git_outcome(rollback);
            }
        }
        return store_fault("task::promotion-receipt", &error.to_string());
    }
    let body = CommandAnswer::Task(answer);
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
    projects: &ProjectPolicy,
) -> Result<DeferralTransition, Outcome<CommandAnswer>> {
    if deferral.is_none() && is_oops(entry) {
        return Err(refusal(
            "task::promotion-required",
            Some(oops_state(entry)),
            &format!(
                "promote the parked row first with spice task wake {} --into PUBLIC_PROJECT",
                entry.handle(),
            ),
        ));
    }
    // Settled is the question, not completed. A withdrawn task carries the same
    // completion instant and never becomes workable again.
    if entry.workflow().completion.is_some() {
        return Err(refusal(
            "task::settled",
            Some(task.to_string()),
            "defer or wake only pending work",
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
        && workflow.assignment.as_ref() == Some(command.actor())
    {
        workflow.assignment = None;
        workflow.review = "unassigned".to_owned();
    }
    let post = if deferral.is_none() {
        Some(
            Entry::new(
                entry.statement().clone(),
                workflow.clone(),
                entry.claim().cloned(),
                entry.lease().cloned(),
                projects,
            )
            .map_err(|error| {
                refusal(
                    "task::deferral-refused",
                    Some(error.to_string()),
                    "wake a task into a valid pending state",
                )
            })?,
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

fn prepare_deferral_release(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    repository: Option<&Repository>,
    releases_claim: bool,
) -> Result<Option<spice::task_git::ParkedRelease>, Outcome<CommandAnswer>> {
    let Some(resident) = repository.filter(|_| releases_claim) else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let claim = entry.claim().expect("release predicate requires a claim");
    let opening = snapshot
        .facts
        .at(claim.claimed)
        .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
        .and_then(|won| won.git);
    spice::task_git::prepare_free(&lane, entry.born(), opening.as_ref()).map_err(task_git_outcome)
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

fn rollback_release(
    parked: Option<spice::task_git::ParkedRelease>,
    outcome: Outcome<CommandAnswer>,
) -> Outcome<CommandAnswer> {
    match parked {
        Some(parked) => match parked.rollback() {
            Ok(()) => outcome,
            Err(error) => task_git_outcome(error),
        },
        None => outcome,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the receipt closes one guarded deferral and its two exclusive Git transitions"
)]
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
    let body = CommandAnswer::Task(answer);
    if let Some(parked) = parked {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Release(parked))
    } else if let Some(prepared) = prepared_claim {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Claim(prepared))
    } else {
        committed(bus, write, body)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "deferral mutation keeps optional atomic claim authority explicit"
)]
fn change_deferral<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskHandle,
    deferral: Option<Instant>,
    intent: B,
    repository: Option<&Repository>,
    claim_requested: bool,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, task) else {
        return refusal(
            "task::unknown",
            Some(task.to_string()),
            "read board and name one current task handle",
        );
    };
    let DeferralTransition {
        workflow,
        post,
        releases_claim,
    } = match deferral_transition(entry, command, task, deferral, projects) {
        Ok(transition) => transition,
        Err(outcome) => return outcome,
    };
    let selected = post.as_ref().unwrap_or(entry);
    let atomic_claim = match prepare_mutation_claim(
        bus,
        command,
        repository,
        &snapshot,
        selected,
        claim_requested,
    ) {
        Ok(claim) => claim,
        Err(outcome) => return outcome,
    };
    let claim_answer = atomic_claim.as_ref().map(|claim| claim.answer.clone());
    let next = claim_answer
        .as_ref()
        .and_then(|claim| claim.next.clone())
        .or_else(|| post.as_ref().and_then(|post| next_action(&snapshot, post)));
    let mut write = match prepare_mutation_write(command, entry, &snapshot, atomic_claim.as_ref()) {
        Ok(write) => write,
        Err(outcome) => return outcome,
    };
    let parked =
        match prepare_deferral_release(&snapshot, command, entry, repository, releases_claim) {
            Ok(parked) => parked,
            Err(outcome) => return outcome,
        };
    let fact = match append_deferral_fact(&mut write, entry, deferral) {
        Ok(fact) => fact,
        Err(outcome) => return rollback_release(parked, outcome),
    };
    write.workflow_head(entry.born().clone(), fact, move |_| workflow);
    if let Err(outcome) = append_deferral_release(
        &mut write,
        entry,
        command.actor(),
        parked.as_ref(),
        releases_claim,
    ) {
        return rollback_release(parked, outcome);
    }
    let prepared_claim = match atomic_claim {
        Some(claim) => match claim.prepared.append(&mut write, command, selected) {
            Ok(prepared) => prepared,
            Err(outcome) => return rollback_release(parked, outcome),
        },
        None => None,
    };
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: if deferral.is_some() {
            "deferred".to_owned()
        } else {
            "pending".to_owned()
        },
        guidance: None,
        completion: None,
        learning: None,
        claim: claim_answer,
        next,
    };
    finish_deferral(bus, command, intent, write, answer, parked, prepared_claim)
}
