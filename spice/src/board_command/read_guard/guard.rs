use super::*;

/// Refuse the allocator's write if the board it selected over has moved.
///
/// Four comparisons run in order and the caller sees the first one only: the
/// client's own witnesses, the residency position, the head set, the winner.
/// The last cannot refuse in production, and it is kept anyway.
///
/// It is unreachable because the head set precedes it. `AllocationHeads` carries
/// every task's statement, workflow, claim and lease position, so a head set
/// comparing equal means identical facts, and the entries rebuilt below are the
/// entries the snapshot selected over. The `board` crate reads no clock at all,
/// and eligibility turns on `workflow.deferral`, a head field only a commit can
/// clear, so waiting changes nothing the selection reads. Actor, subscription
/// and band all arrive from that same snapshot. Recomputing the selection here
/// therefore returns what it returned there, against itself.
///
/// It is kept because it is the only place the invariant is written down: the
/// head set is a conservative proxy that refuses on any movement anywhere, while
/// this says the lane is granted the task the allocator picked for it. Narrowing
/// the head set is what would make it live -- fewer fields, a digest, or only the
/// subscribed projects -- and a lease or workflow change that flips the selection
/// would then arrive here. Until that happens its only proof is fixture-level:
/// `a_winner_that_was_never_the_selection_refuses_the_allocator_write` hands it
/// an observation by hand, and reds when this comparison is severed while the
/// other six tests in that file stay green.
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
        let heads = current_task_heads(read)?;
        let trust = current_trust_heads(read)?;
        let environment = wire::Environment::journal_schema(journal::SCHEMA_VERSION)
            .expect("the compiled journal schema is positive")
            .with_trust_heads(trust.into_iter().map(journal::JournalHead::into_body));
        let current = heads
            .iter()
            .map(|task| {
                (
                    TaskHandle::for_project(
                        &task.statement().body().project,
                        &task.statement().body().born,
                    ),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current != observed.heads {
            return Err(GuardError::refused(
                "board::witness-moved: allocator head set changed",
            ));
        }
        refuse_active_claim_conflict(&heads, &observed.actor)?;
        let entries = heads
            .iter()
            .map(|task| {
                let creator_lineage = observed
                    .creator_lineages
                    .get(&task.statement().body().born)
                    .copied()
                    .ok_or_else(|| {
                        GuardError::Invalid(format!(
                            "allocator task {} has no observed creator lineage",
                            task.statement().body().born,
                        ))
                    })?;
                Entry::new(
                    task.statement().body().clone(),
                    task.workflow().body().clone(),
                    task.claim().map(|head| head.body().clone()),
                    task.lease().map(|head| head.body().clone()),
                    &observed.projects,
                )
                .map_err(|error| {
                    GuardError::Invalid(format!("allocator current heads are invalid: {error}"))
                })
                .map(|entry| entry.with_creator_lineage(creator_lineage))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if board::select(
            &entries,
            &observed.actor,
            &observed.subscription,
            observed.band,
            observed.recent_cell.as_ref(),
            &environment,
        )
        .is_some_and(|selected| selected.entry().handle() == &observed.winner)
        {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: allocator winner changed",
            ))
        }
    }));
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
    let heads = current_task_heads(read)?
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

pub(super) fn current_task_heads(
    read: &JournalRead<'_>,
) -> Result<Vec<journal::TaskHeads>, GuardError> {
    let mut heads = Vec::new();
    let mut after = None;
    loop {
        let page = read.task_head_page(after.as_ref(), journal::FactReadBudget::RETAINED)?;
        heads.extend(page.rows);
        let Some(continuation) = page.continuation else {
            return Ok(heads);
        };
        after = Some(continuation);
    }
}

pub(super) fn current_trust_heads(
    read: &JournalRead<'_>,
) -> Result<Vec<journal::JournalHead<wire::TrustHead>>, GuardError> {
    let mut heads = Vec::new();
    let mut after = None;
    loop {
        let page = read.trust_head_page(after.as_ref(), journal::FactReadBudget::RETAINED)?;
        heads.extend(page.rows);
        let Some(continuation) = page.continuation else {
            return Ok(heads);
        };
        after = Some(continuation);
    }
}
