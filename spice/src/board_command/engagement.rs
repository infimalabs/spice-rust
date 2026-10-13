//! Working-group engagement authorizes a Design's controlled outputs.
//!
//! Anchoring a Design in a working group is the operator's activation of it:
//! the storing write authorizes every controlled output the Design already
//! has, and an output that becomes controlled while some working group still
//! anchors its controller carries that group's operator fact from birth.

use super::*;

use journal::{FactReadBudget, MAX_RETAINED_FACT_PAGE_BYTES};

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

/// Authorize `statement` by its controller's current engagement when it is
/// an unauthorized controlled output whose controller a working group anchors.
pub(super) fn engage(bus: &Bus, statement: &mut TaskHead) -> Result<(), Outcome<CommandAnswer>> {
    let Some(controller) = unauthorized_controller(statement).cloned() else {
        return Ok(());
    };
    statement.engagement = bus
        .read(move |read| {
            let budget = FactReadBudget::new(1, MAX_RETAINED_FACT_PAGE_BYTES)
                .expect("a one-row page forms a journal budget");
            let Some(group) = read
                .anchor_group_page(&controller, None, budget)?
                .rows
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            Ok(read.group_head(&group)?.map(|head| head.position()))
        })
        .map_err(|error| store_fault("preparation::engagement-unreadable", &error.to_string()))?;
    Ok(())
}
