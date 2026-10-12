use super::*;

/// Refuse an allocator write when its exact witnesses, residency, compact
/// projection cursor, or indexed one-active-claim invariant has moved.
pub(in crate::board_command) fn guard_allocation(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationObservation,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let residency = read.residency_head()?.ok_or_else(|| {
            GuardError::Invalid("allocator current residency head is missing".to_owned())
        })?;
        if residency.position() != observed.residency_position {
            return Err(GuardError::refused(
                "board::witness-moved: allocator subscription head changed",
            ));
        }
        if read.cursor()? != Some(observed.cursor) {
            return Err(GuardError::refused(
                "board::witness-moved: allocator projection changed",
            ));
        }
        refuse_active_claim_conflict(read, &observed.actor)
    }));
}

pub(super) fn refuse_active_claim_conflict(
    read: &JournalRead<'_>,
    actor: &Actor,
) -> Result<(), GuardError> {
    let Some(active) = read.active_claim_for_actor(actor)? else {
        return Ok(());
    };
    let first = TaskHandle::for_project(
        &active.statement().body().project,
        &active.statement().body().born,
    );
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
        Witness::Group { group, head } => {
            Ok(read.group_head(group)?.map(|head| head.position()) == *head)
        }
        Witness::Directive { .. }
        | Witness::Recoverable { .. }
        | Witness::Consent { .. }
        | Witness::Rule { .. }
        | Witness::Budget { .. }
        | Witness::Trust { .. }
        | Witness::Taste { .. }
        | Witness::Observation { .. }
        | Witness::Sealed { .. }
        | Witness::Source { .. }
        | Witness::Proposal { .. }
        | Witness::Authorization { .. }
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
            occupant: Some(occupant),
            epoch,
        } => read.residency_head()?.is_some_and(|observed| {
            observed.body().epoch == *epoch
                && observed.body().lanes.iter().any(|seat| {
                    seat.lane == *lane
                        && seat.occupant == Some(*occupant)
                        && seat.disposition == "occupied"
                })
        }),
        // Board work is somebody's: a lane nobody sits in holds no claim.
        Witness::Residency { occupant: None, .. } => false,
        Witness::Eligibility { rule } => matches!(
            rule.as_str(),
            "next" | "capture" | CAPTURE_WORK_RULE | "steal" | "recover"
        ),
        Witness::Preview { .. } => true,
        Witness::Barrier { name, digest } => {
            name.as_str() == "board"
                && task_projection_barrier(read.task_projection_cursor()?) == *digest
        }
        _ => unreachable!("board witness partition is closed"),
    })
}
