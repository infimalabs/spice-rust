//! Private claimant notices derived from task authority.
//!
//! The task journal is the authority. A resident-minted claim is announced to
//! its exact occupant, and each active claim records the task generation it
//! acknowledged so later edits can require reclaim. These projections create
//! no steering directive or acknowledgment key; the detached occupant driver
//! deduplicates repeated resident observations by their authority identities.

use journal::{JournalHead, TaskHeads};
use wire::TaskHandle;

use super::*;
use crate::resident::supervisor::command::{
    TaskAssignmentEnd, TaskAssignmentNotice, TaskGenerationNotice,
};

/// Index rows one read returns while a claim trails its task.
const ADVANCE_PAGE: usize = 64;

pub(super) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    snapshot: &bus::TaskSnapshot,
    findings: super::review_feedback::FindingWindow<'_>,
) -> CommandResult<bool> {
    for (lane, notice) in assignment_notices(snapshot) {
        super::super::supervisor::command::task_assignment(paths, lane, notice);
    }
    for (lane, ended) in assignment_ends(snapshot, findings.cursor) {
        super::super::supervisor::command::task_assignment_ended(paths, lane, ended);
    }
    for (lane, notice) in notices(bus, snapshot)? {
        super::super::supervisor::command::task_generation(paths, lane, notice);
    }
    super::review_feedback::reconcile(bus, paths, epoch, snapshot, findings)
}

fn assignment_notices(snapshot: &bus::TaskSnapshot) -> Vec<(CanonicalUuid, TaskAssignmentNotice)> {
    snapshot
        .tasks()
        .iter()
        .filter_map(|heads| {
            let claim = heads.claim()?.body();
            if claim.disposition != ClaimDisposition::Active
                || !crate::board_command::is_automatic_claim(claim, snapshot.facts())
            {
                return None;
            }
            let statement = heads.statement().body();
            Some((
                claim.lane,
                TaskAssignmentNotice {
                    occupant: claim.occupant,
                    task: TaskHandle::for_project(&statement.project, &statement.born).to_string(),
                    claim: claim.claimed.get(),
                    phase: heads.workflow().body().phase.to_string(),
                },
            ))
        })
        .collect()
}

/// Claims that ended after `cursor`, the position the previous pass observed.
///
/// An assignment notice waits in the occupant's queue until its next step
/// boundary, so the claim it announces can end first. Each ending since the
/// last pass is told to the occupant that held the claim, whose driver
/// withdraws a notice for it that has not been delivered yet. The first pass
/// has no earlier observation and announces no endings.
fn assignment_ends(
    snapshot: &bus::TaskSnapshot,
    cursor: Option<Seq>,
) -> Vec<(CanonicalUuid, TaskAssignmentEnd)> {
    let Some(cursor) = cursor else {
        return Vec::new();
    };
    snapshot
        .tasks()
        .iter()
        .filter_map(|heads| {
            let claim = heads.claim()?.body();
            if claim.disposition == ClaimDisposition::Active || claim.fact <= cursor {
                return None;
            }
            let statement = heads.statement().body();
            Some((
                claim.lane,
                TaskAssignmentEnd {
                    occupant: claim.occupant,
                    task: TaskHandle::for_project(&statement.project, &statement.born).to_string(),
                    claim: claim.claimed.get(),
                },
            ))
        })
        .collect()
}

fn notices(
    bus: &Bus,
    snapshot: &bus::TaskSnapshot,
) -> CommandResult<Vec<(CanonicalUuid, TaskGenerationNotice)>> {
    let mut notices = Vec::new();
    for heads in snapshot.tasks() {
        notices.extend(task_notices(bus, snapshot.facts(), heads)?);
    }
    Ok(notices)
}

fn task_notices(
    bus: &Bus,
    facts: &bus::TaskFacts,
    heads: &TaskHeads,
) -> CommandResult<Vec<(CanonicalUuid, TaskGenerationNotice)>> {
    let Some(claim) = heads
        .claim()
        .map(JournalHead::body)
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
    else {
        return Ok(Vec::new());
    };
    if claim.acknowledged >= heads.workflow().body().generation {
        return Ok(Vec::new());
    }
    let statement = heads.statement().body();
    let handle = TaskHandle::for_project(&statement.project, &statement.born).to_string();
    let mut after = claim.acknowledged;
    let mut notices = Vec::new();
    loop {
        let task = statement.born.clone();
        let advances = bus
            .read(move |read| read.task_generations_after(&task, after, ADVANCE_PAGE))
            .map_err(|error| {
                store_fault("task-generation::index-unreadable", &error.to_string())
            })?;
        for advance in &advances {
            let envelope = facts.at(advance.position).ok_or_else(|| {
                store_fault(
                    "task-generation::fact-missing",
                    &format!(
                        "task {} generation {} names missing position {}",
                        statement.born, advance.generation, advance.position
                    ),
                )
            })?;
            notices.push((
                claim.lane,
                TaskGenerationNotice {
                    occupant: claim.occupant,
                    task: handle.clone(),
                    claim: claim.claimed.get(),
                    generation: advance.generation,
                    change: envelope.fact.kind().to_string(),
                    editor: envelope.actor.to_string(),
                    position: advance.position.get(),
                },
            ));
        }
        match advances.last() {
            Some(last) if advances.len() == ADVANCE_PAGE => after = last.generation,
            _ => return Ok(notices),
        }
    }
}

#[cfg(test)]
mod tests;
