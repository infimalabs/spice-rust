//! The one acceptance admission every successful completion shares.
//!
//! A phase advance, a final completion, a clean review verdict, and a Design
//! pass proposal each need an acceptance contract: substantive acceptance on
//! the task's own statement, or on a task directly linked to it by a
//! dependency in either direction. Linked work lends its acceptance while it
//! is pending or positively concluded, whatever its intent or however it came
//! to be linked. The rule reads current statements and links when the task
//! completes; no statement, acceptance, or link change rewrites a flow.

use super::*;

/// Whether any criterion says something.
pub(super) fn substantive_acceptance(acceptance: &[String]) -> bool {
    acceptance
        .iter()
        .any(|criterion| !criterion.trim().is_empty())
}

/// Where one task's acceptance contract stands.
pub(super) enum Acceptance<'a> {
    /// The task's own statement carries substantive acceptance.
    Own,
    /// Only directly linked work does: the task's own prerequisites in
    /// statement order, then the tasks that depend on it in board order.
    Linked(Vec<&'a BoardTask>),
    /// Neither the task nor any directly linked work does.
    Missing,
}

/// Project where `statement`'s acceptance contract stands on `board`.
pub(super) fn acceptance<'a>(
    board: &'a BoardAnswer,
    statement: &wire::intent::BoardStatement,
) -> Result<Acceptance<'a>, Outcome<CommandAnswer>> {
    if substantive_acceptance(&statement.acceptance) {
        return Ok(Acceptance::Own);
    }
    let mut linked = Vec::new();
    for dependency in &statement.dependencies {
        let prerequisite = board
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *dependency)
            .ok_or_else(|| {
                protocol_fault(
                    "task::board-incoherent",
                    "a stored dependency has no visible board task",
                )
            })?;
        if lends_acceptance(prerequisite) {
            linked.push(prerequisite);
        }
    }
    linked.extend(board.tasks.iter().filter(|dependent| {
        dependent
            .statement
            .body
            .dependencies
            .contains(&statement.born)
            && lends_acceptance(dependent)
    }));
    Ok(if linked.is_empty() {
        Acceptance::Missing
    } else {
        Acceptance::Linked(linked)
    })
}

/// Whether linked work in this state lends its acceptance to its neighbors.
pub(super) fn lends(acceptance: &[String], workflow: &wire::WorkflowHead) -> bool {
    (workflow.status.is_pending() || workflow.concluded_positively())
        && substantive_acceptance(acceptance)
}

fn lends_acceptance(task: &BoardTask) -> bool {
    lends(&task.statement.body.acceptance, &task.workflow.body)
}

/// Push the witnesses guarding the linked work `task`'s acceptance rests on;
/// a task carrying its own acceptance is guarded by its own statement witness.
pub(super) fn acceptance_witnesses(
    board: &BoardAnswer,
    task: &BoardTask,
    expected: &mut Vec<Witness>,
) -> Result<(), Outcome<CommandAnswer>> {
    if let Acceptance::Linked(linked) = acceptance(board, &task.statement.body)? {
        for work in linked {
            expected.push(statement_witness(work));
            expected.push(workflow_witness(work));
        }
    }
    Ok(())
}

/// Admit a successful completion of `task` only on an acceptance contract:
/// its own statement, which every completion witnesses, or one directly linked
/// task lending acceptance whose statement and workflow the command witnessed.
/// A refusal moves no flow, phase, or claim.
pub(super) fn require_acceptance(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    task: &wire::TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    let row = visible_task(snapshot, task)?;
    let witnessed = |work: &BoardTask| {
        let expected = command.expected();
        expected.contains(&statement_witness(work)) && expected.contains(&workflow_witness(work))
    };
    let handle = row.statement.body.handle.to_string();
    match acceptance(&snapshot.answer, &row.statement.body)? {
        Acceptance::Own => Ok(()),
        Acceptance::Linked(linked) if linked.iter().any(|work| witnessed(work)) => Ok(()),
        Acceptance::Linked(_) => Err(refusal(
            "task::acceptance-witness-missing",
            Some(handle),
            "read the current task and the linked work carrying its acceptance, then retry",
        )),
        Acceptance::Missing => Err(refusal(
            "task::acceptance-missing",
            Some(handle.clone()),
            &format!(
                "a task completes only on an acceptance contract: add one with `spice task \
                 modify {handle} --acceptance TEXT`, or link work carrying acceptance with \
                 `spice task depends`; a Plan whose need warrants no work records why with \
                 `spice task done --disposition no-work|duplicate|covered|deferred [--cover \
                 HANDLE|COMMIT]`"
            ),
        )),
    }
}
