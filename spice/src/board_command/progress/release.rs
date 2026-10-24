use super::*;
struct ReleaseTransition {
    post: Entry,
    claimed: Seq,
    review_released: bool,
}

fn release_transition(
    entry: &Entry,
    actor: &Actor,
    projects: &ProjectPolicy,
) -> Result<ReleaseTransition, Outcome<CommandAnswer>> {
    let Some(claim) = entry.claim() else {
        return Err(refusal(
            "claim::head-missing",
            Some(entry.handle().to_string()),
            "free only a task carrying an active claim",
        ));
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *actor {
        return Err(refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "free the task through the capability that currently owns it",
        ));
    }
    let mut post_claim = claim.clone();
    post_claim.disposition = ClaimDisposition::Freed;
    let mut workflow = entry.workflow().clone();
    let review_released =
        workflow.phase.as_str() == "review" && workflow.status.reviewer() == Some(actor);
    if review_released {
        workflow.status = wire::WorkflowStatus::pending();
    }
    let post = Entry::new(
        entry.statement().clone(),
        workflow,
        Some(post_claim),
        entry.lease().cloned(),
        projects,
    )
    .expect("releasing a claim preserves a valid task entry")
    .with_explicit_release(true)
    .with_grouped(entry.grouped())
    .with_design(entry.design());
    Ok(ReleaseTransition {
        post,
        claimed: claim.claimed,
        review_released,
    })
}

pub(in crate::board_command) fn prepare_release<'a>(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    claimed: Seq,
    repository: Option<Host<'a>>,
    planning_phase: Option<&str>,
) -> Result<Option<spice::task_git::ParkedRelease<'a>>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, &resident)?;
    let opening = snapshot
        .facts
        .at(claimed)
        .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
        .and_then(|won| won.git);
    if let Some(phase) = planning_phase {
        spice::task_git::prepare_planning_free(&lane, resident.ambient, phase, opening.as_ref())
            .map_err(task_git_outcome)
    } else {
        spice::task_git::prepare_free(&lane, resident.ambient, opening.as_ref())
            .map_err(task_git_outcome)
    }
}

fn append_release(
    write: &mut JournalWrite,
    entry: &Entry,
    transition: &ReleaseTransition,
    actor: &Actor,
    parked: Option<&spice::task_git::ParkedRelease>,
) -> Result<(), Outcome<CommandAnswer>> {
    let freed = write
        .fact(
            body::ClaimFreed {
                task: entry.born().clone(),
                actor: actor.clone(),
                parked: parked.map(|prepared| prepared.parked().clone()),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("claim::free-build", &error.to_string()))?;
    let next_claim = transition
        .post
        .claim()
        .expect("released task retains its claim history")
        .clone();
    write.claim_head(entry.born().clone(), freed, move |fact| ClaimHead {
        fact,
        ..next_claim
    });
    if transition.review_released {
        let workflow = transition.post.workflow().clone();
        write.workflow_head(entry.born().clone(), freed, move |_| workflow);
    }
    Ok(())
}

fn finish_release(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskRef,
    mut write: JournalWrite,
    answer: wire::intent::ClaimAnswer,
    parked: Option<spice::task_git::ParkedRelease>,
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.command_result(
        request(command, task.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return rollback_release(
            parked,
            store_fault("claim::free-receipt", &error.to_string()),
        );
    }
    let body = CommandAnswer::Claim(Box::new(answer));
    match parked {
        Some(parked) => committed_with_task_git(bus, write, body, PreparedTaskGit::Release(parked)),
        None => committed(bus, write, body),
    }
}

pub(in crate::board_command) fn claim_free(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskRef,
    repository: Option<Host<'_>>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&task.task),
        projects,
        allocation,
    ) {
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
    let transition = match release_transition(entry, command.actor(), projects) {
        Ok(transition) => transition,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let next = next_action(&snapshot, &transition.post);
    let parked = match prepare_release(&snapshot, command, transition.claimed, repository, None) {
        Ok(parked) => parked,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(outcome) = append_release(
        &mut write,
        entry,
        &transition,
        command.actor(),
        parked.as_ref(),
    ) {
        return rollback_release(parked, outcome);
    }
    let answer = wire::intent::ClaimAnswer {
        task: entry.handle().clone(),
        statement: None,
        holder: None,
        expiry: None,
        acknowledged: None,
        held: false,
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        room_context: None,
        next,
        phase: None,
        nothing_stolen: false,
        transplant: None,
    };
    finish_release(bus, command, task, write, answer, parked)
}
