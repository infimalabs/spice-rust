//! Working-group engagement authorizes a Design's controlled outputs.
//!
//! Moving a Design into a working group is the operator's activation of it:
//! the move authorizes every controlled output the Design already has, and an
//! output that becomes controlled while the partition still groups its
//! controller carries that partition head's operator fact from birth.

use super::*;

/// Controller whose authorization gates `statement`, when it is a controlled
/// output still lacking every authorization.
fn unauthorized_controller(statement: &TaskHead) -> Option<&wire::TaskStamp> {
    statement
        .preparation
        .as_ref()
        .filter(|preparation| {
            statement.intent.is_none()
                && preparation.construction == statement.born
                && statement.authorization.is_none()
                && statement.engagement.is_none()
        })
        .map(|preparation| &preparation.controller)
}

/// Every unauthorized controlled output of the Designs `anchors` names, each
/// with the statement witness that keeps a concurrent edit from being
/// overwritten by its authorization.
pub(super) fn anchored_outputs(
    bus: &Bus,
    anchors: &[wire::rooms::RoomAnchor],
) -> Result<(Vec<TaskHead>, Vec<Witness>), Outcome<CommandAnswer>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("group::heads-unreadable", &error.to_string()))?;
    let outputs = snapshot
        .tasks()
        .iter()
        .map(|heads| heads.statement())
        .filter(|statement| {
            unauthorized_controller(statement.body())
                .is_some_and(|controller| anchors.contains(controller))
        })
        .collect::<Vec<_>>();
    let witnesses = outputs
        .iter()
        .map(|statement| Witness::Task {
            task: statement.body().born.clone(),
            head: statement.position(),
        })
        .collect();
    Ok((
        outputs
            .into_iter()
            .map(|statement| statement.body().clone())
            .collect(),
        witnesses,
    ))
}

/// Engage through `fact` every output in `outputs` that a Design of
/// `anchors` controls, leaving the rest for another group fact of the write.
pub(super) fn grant(
    write: &mut JournalWrite,
    fact: PendingFact,
    outputs: &mut Vec<TaskHead>,
    anchors: &[wire::rooms::RoomAnchor],
) {
    let (engaged, rest): (Vec<_>, Vec<_>) =
        std::mem::take(outputs).into_iter().partition(|output| {
            unauthorized_controller(output).is_some_and(|controller| anchors.contains(controller))
        });
    *outputs = rest;
    for output in engaged {
        write.task_head(fact, move |seq| TaskHead {
            engagement: Some(seq),
            revision: seq,
            ..output
        });
    }
}

/// Authorize `statement` by the current partition head when it is an
/// unauthorized controlled output whose controller the partition groups.
pub(super) fn engage(bus: &Bus, statement: &mut TaskHead) -> Result<(), Outcome<CommandAnswer>> {
    let Some(controller) = unauthorized_controller(statement).cloned() else {
        return Ok(());
    };
    statement.engagement = bus
        .read(move |read| {
            Ok(read.group_partition()?.and_then(|head| {
                head.body()
                    .holder(&controller)
                    .is_some()
                    .then(|| head.position())
            }))
        })
        .map_err(|error| store_fault("preparation::engagement-unreadable", &error.to_string()))?;
    Ok(())
}
