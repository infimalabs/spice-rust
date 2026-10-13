//! Retained working-group markers from the former authorization model.
//!
//! Current working-group writes organize Designs only and never add one of
//! these markers. Statements that already carry a marker keep its exact
//! position across later writes so historical authorization remains eligible.
//! Replay still verifies that retained position against the operator group fact
//! that originally established it.

use rusqlite::{Connection, OptionalExtension};
use wire::rooms::{WorkingGroupMoved, WorkingGroupPartition, WorkingGroupsArranged};
use wire::{Actor, AnyFact, TaskHead, TaskStamp};

use super::{Refusal, unsound};

/// Controller whose authorization gates `head`, when `head` is a controlled
/// output: preparation it constructs itself, without Plan or Design intent.
#[must_use]
pub(crate) fn gated_controller(head: &TaskHead) -> Option<&TaskStamp> {
    head.preparation
        .as_ref()
        .filter(|preparation| head.intent.is_none() && preparation.construction == head.born)
        .map(|preparation| &preparation.controller)
}

/// Whether `fact`, written by `actor`, is the historical operator partition
/// change named by a retained marker for `controller`.
#[must_use]
pub(crate) fn engages(actor: &Actor, fact: &AnyFact, controller: &TaskStamp) -> bool {
    *actor == Actor::Operator
        && stated_partition(fact).is_some_and(|partition| partition.holder(controller).is_some())
}

/// The whole partition a working-group fact states.
fn stated_partition(fact: &AnyFact) -> Option<WorkingGroupPartition> {
    fact.read::<WorkingGroupMoved>()
        .map(|moved| moved.partition)
        .or_else(|| {
            fact.read::<WorkingGroupsArranged>()
                .map(|arranged| arranged.partition)
        })
}

/// Admit one task statement row before it replaces the stored statement. A
/// retained marker is exclusive with selected authorization and survives every
/// later write unchanged. Current history cannot grant a new marker.
pub(crate) fn require_transition(
    connection: &Connection,
    stamp: &str,
    body: &[u8],
) -> Result<(), Refusal> {
    let head = wire::decode::<TaskHead>(body)?;
    if head.authorization.is_some() && head.engagement.is_some() {
        return unsound(format!(
            "task {stamp} carries both selected authorization and a retained group marker"
        ));
    }
    let prior = connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [stamp],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|body| wire::decode::<TaskHead>(&body))
        .transpose()?
        .and_then(|prior| prior.engagement);
    match (prior, head.engagement) {
        (prior, retained) if prior == retained => Ok(()),
        (None, Some(_)) => unsound(format!("task {stamp} cannot gain a retained group marker")),
        _ => unsound(format!(
            "task {stamp} retained group marker cannot move or drop"
        )),
    }
}
