use super::*;

pub(in crate::board_command) fn dependency_claim_authority(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    at: Instant,
) -> Result<(), Outcome<CommandAnswer>> {
    let Some(claim) = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
    else {
        return match command.actor() {
            Actor::Operator => Ok(()),
            Actor::Lane(_) => Err(refusal(
                "claim::ownership-refused",
                entry
                    .claim()
                    .map(|claim| claim.actor.to_string())
                    .or_else(|| Some("absent".to_owned())),
                "win the task's current active claim before editing its dependencies",
            )),
            Actor::Spice => Err(protocol_fault(
                "task::dependency-principal-invalid",
                "resident authority cannot edit task dependencies",
            )),
        };
    };
    let Actor::Lane(lane) = command.actor() else {
        return Err(refusal(
            "claim::lane-required",
            Some(command.actor().to_string()),
            "append dependencies through the lane capability that owns the active claim",
        ));
    };
    if claim.actor != *command.actor() || claim.lane != *lane {
        return Err(refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "append dependencies through the lane capability that owns the active claim",
        ));
    }
    board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            "reclaim the current active task before editing its dependencies",
        )
    })?;
    let has_lease = command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Lease { task, .. } if task == entry.born()));
    let has_residency = command.expected().iter().any(|witness| {
        matches!(witness, Witness::Residency { lane: witnessed, .. } if witnessed == lane)
    });
    let has_assignment = command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Assignment { task, .. } if task == entry.born()));
    if !has_lease
        || !has_residency
        || (entry.workflow().phase.as_str() == "review" && !has_assignment)
    {
        return Err(protocol_fault(
            "task::dependency-claim-witness-missing",
            "an active claimed dependency edit must bind lease, residency, and review assignment authority",
        ));
    }
    let residency = &snapshot.answer.residency.body;
    let occupant = residency
        .lanes
        .iter()
        .find(|seat| seat.lane == *lane && seat.disposition == "occupied")
        .and_then(|seat| seat.occupant);
    if residency.epoch != command.epoch()
        || claim.epoch != command.epoch()
        || occupant != Some(claim.occupant)
    {
        return Err(refusal(
            "claim::residency-moved",
            Some(entry.handle().to_string()),
            "reclaim the task under the current lane occupant before editing dependencies",
        ));
    }
    Ok(())
}

pub(super) fn refuse_active_claim_conflict(
    heads: &[journal::TaskHeads],
    actor: &Actor,
) -> Result<(), GuardError> {
    let mut active = heads.iter().filter_map(|task| {
        let claim = task.claim()?;
        (claim.body().actor == *actor && claim.body().disposition == ClaimDisposition::Active).then(
            || {
                TaskHandle::for_project(
                    &task.statement().body().project,
                    &task.statement().body().born,
                )
            },
        )
    });
    let Some(first) = active.next() else {
        return Ok(());
    };
    if active.next().is_some() {
        return Err(GuardError::Invalid(format!(
            "one actor already holds multiple active claims before claim mint: {actor}"
        )));
    }
    Err(GuardError::refused(format!(
        "claim::active-exists: actor {actor} already holds active claim {first}"
    )))
}

pub(super) fn witnesses_hold(
    read: &JournalRead<'_>,
    expected: &[Witness],
) -> Result<(), GuardError> {
    for witness in expected {
        let holds = witness_holds(read, witness)?;
        if !holds {
            return Err(GuardError::refused(format!(
                "board::witness-moved: {witness:?}"
            )));
        }
    }
    Ok(())
}

fn witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    match witness {
        Witness::Task { .. }
        | Witness::Absence { .. }
        | Witness::Existence { .. }
        | Witness::Edge { .. } => task_witness_holds(read, witness),
        Witness::Workflow { .. }
        | Witness::Claim { .. }
        | Witness::Lease { .. }
        | Witness::Deferral { .. }
        | Witness::Assignment { .. } => workflow_witness_holds(read, witness),
        Witness::Residency { .. }
        | Witness::Eligibility { .. }
        | Witness::Preview { .. }
        | Witness::Barrier { .. } => board_witness_holds(read, witness),
        Witness::Directive { .. }
        | Witness::Recoverable { .. }
        | Witness::Consent { .. }
        | Witness::Rule { .. }
        | Witness::Budget { .. }
        | Witness::Trust { .. }
        | Witness::Taste { .. }
        | Witness::Observation { .. }
        | Witness::Sealed { .. }
        | Witness::Transition { .. } => Err(GuardError::Invalid(
            "board transaction carried a witness outside its authority plane".to_owned(),
        )),
    }
}

fn task_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Task { task, head } => read
            .task_head(task)?
            .is_some_and(|observed| observed.position() == *head),
        Witness::Absence { task } => read.task_head(task)?.is_none(),
        Witness::Existence { task } => read.task_head(task)?.is_some(),
        Witness::Edge { task, after } => {
            let subject = read.task_head(task)?;
            let dependency = read.task_head(after)?;
            subject.is_some_and(|head| !head.body().dependencies.contains(after))
                && dependency.is_some()
        }
        _ => unreachable!("task witness partition is closed"),
    })
}

fn workflow_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Workflow { task, head, phase } => {
            read.workflow_head(task)?.is_some_and(|observed| {
                observed.position() == *head && observed.body().phase == *phase
            })
        }
        Witness::Claim { task, head } => {
            read.claim_head(task)?.map(|observed| observed.position()) == *head
        }
        Witness::Lease { task, head } => read
            .lease_head(task)?
            .is_some_and(|observed| observed.position() == *head),
        Witness::Deferral {
            task,
            head,
            deferred,
        } => read.workflow_head(task)?.is_some_and(|observed| {
            observed.position() == *head && observed.body().deferral.is_some() == *deferred
        }),
        Witness::Assignment {
            task,
            head,
            reviewer,
        } => read.workflow_head(task)?.is_some_and(|observed| {
            observed.position() == *head && observed.body().assignment.as_ref() == Some(reviewer)
        }),
        _ => unreachable!("workflow witness partition is closed"),
    })
}

fn board_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Residency {
            lane,
            occupant,
            epoch,
        } => read.residency_head()?.is_some_and(|observed| {
            observed.body().epoch == *epoch
                && observed.body().lanes.iter().any(|seat| {
                    seat.lane == *lane
                        && seat.occupant == Some(*occupant)
                        && seat.disposition == "occupied"
                })
        }),
        Witness::Eligibility { rule } => {
            matches!(rule.as_str(), "next" | "capture" | "steal" | "recover")
        }
        Witness::Preview { .. } => true,
        Witness::Barrier { name, digest } => {
            name.as_str() == "board" && board_barrier_from_read(read)? == *digest
        }
        _ => unreachable!("board witness partition is closed"),
    })
}

fn board_barrier_from_read(read: &JournalRead<'_>) -> Result<Digest, GuardError> {
    let heads = read
        .task_heads()?
        .into_iter()
        .map(|task| {
            (
                TaskHandle::for_project(
                    &task.statement().body().project,
                    &task.statement().body().born,
                ),
                task.statement().position(),
                task.workflow().position(),
            )
        })
        .collect::<Vec<_>>();
    let bytes = wire::encode(&heads)
        .map_err(|error| GuardError::Invalid(format!("board barrier did not encode: {error}")))?;
    Ok(Digest::compute(Domain::BoardBarrier, &bytes))
}
