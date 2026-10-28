//! The one acceptance admission every successful completion shares.
//!
//! A phase advance, a final completion, a clean review verdict, and a Design
//! pass proposal each need an acceptance contract: substantive acceptance on
//! the task's own statement, or on a task directly linked to it by a
//! dependency in either direction. Linked work lends its acceptance while it
//! is pending, whatever its intent or however it came
//! to be linked. The rule reads current statements and links when the task
//! completes; no statement, acceptance, or link change rewrites a flow.

use super::*;

/// Whether any criterion says something.
pub(crate) fn substantive_acceptance(acceptance: &[String]) -> bool {
    acceptance
        .iter()
        .any(|criterion| !criterion.trim().is_empty())
}

/// Where one task's acceptance contract stands.
pub(crate) enum Acceptance<'a, T = BoardTask> {
    /// The task's own statement carries substantive acceptance.
    Own,
    /// Only directly linked work does: the task's own prerequisites in
    /// statement order, then the tasks that depend on it in board order.
    Linked(Vec<&'a T>),
    /// Neither the task nor any directly linked work does.
    Missing,
}

/// Project where `statement`'s acceptance contract stands on `board`.
pub(super) fn acceptance<'a>(
    board: &'a BoardAnswer,
    statement: &wire::intent::BoardStatement,
) -> Result<Acceptance<'a>, Outcome<CommandAnswer>> {
    if !substantive_acceptance(&statement.acceptance) {
        for dependency in &statement.dependencies {
            if !board
                .tasks
                .iter()
                .any(|task| task.statement.body.born == *dependency)
            {
                return Err(protocol_fault(
                    "task::board-incoherent",
                    "a stored dependency has no visible board task",
                ));
            }
        }
    }
    Ok(project_acceptance(
        &statement.acceptance,
        &statement.born,
        &statement.dependencies,
        board.tasks.iter(),
    ))
}

/// Borrowed current authority shared by completion and claimant disclosure.
pub(crate) trait AcceptanceRow {
    fn acceptance(&self) -> &[String];
    fn born(&self) -> &wire::TaskStamp;
    fn dependencies(&self) -> &[wire::TaskStamp];
    fn workflow(&self) -> &wire::WorkflowHead;
    fn acceptance_handle(&self) -> String;
}

impl AcceptanceRow for BoardTask {
    fn acceptance(&self) -> &[String] {
        &self.statement.body.acceptance
    }
    fn born(&self) -> &wire::TaskStamp {
        &self.statement.body.born
    }
    fn dependencies(&self) -> &[wire::TaskStamp] {
        &self.statement.body.dependencies
    }
    fn workflow(&self) -> &wire::WorkflowHead {
        &self.workflow.body
    }
    fn acceptance_handle(&self) -> String {
        self.statement.body.handle.to_string()
    }
}

impl AcceptanceRow for Entry {
    fn acceptance(&self) -> &[String] {
        &self.statement().acceptance
    }
    fn born(&self) -> &wire::TaskStamp {
        self.born()
    }
    fn dependencies(&self) -> &[wire::TaskStamp] {
        &self.statement().dependencies
    }
    fn workflow(&self) -> &wire::WorkflowHead {
        self.workflow()
    }
    fn acceptance_handle(&self) -> String {
        self.handle().to_string()
    }
}

impl AcceptanceRow for journal::TaskHeads {
    fn acceptance(&self) -> &[String] {
        &self.statement().body().acceptance
    }
    fn born(&self) -> &wire::TaskStamp {
        &self.statement().body().born
    }
    fn dependencies(&self) -> &[wire::TaskStamp] {
        &self.statement().body().dependencies
    }
    fn workflow(&self) -> &wire::WorkflowHead {
        self.workflow().body()
    }
    fn acceptance_handle(&self) -> String {
        let statement = self.statement().body();
        wire::TaskHandle::for_project(&statement.project, &statement.born).to_string()
    }
}

pub(super) fn project_acceptance<'a, T: AcceptanceRow + 'a>(
    own: &[String],
    born: &wire::TaskStamp,
    dependencies: &[wire::TaskStamp],
    rows: impl Iterator<Item = &'a T> + Clone,
) -> Acceptance<'a, T> {
    if substantive_acceptance(own) {
        return Acceptance::Own;
    }
    let mut linked = Vec::new();
    for dependency in dependencies {
        if let Some(row) = rows.clone().find(|row| row.born() == dependency)
            && lends(row.acceptance(), row.workflow())
        {
            linked.push(row);
        }
    }
    linked.extend(rows.filter(|row| {
        row.dependencies().contains(born) && lends(row.acceptance(), row.workflow())
    }));
    if linked.is_empty() {
        Acceptance::Missing
    } else {
        Acceptance::Linked(linked)
    }
}

/// Current completion contract, independent of taste, possession and flow.
pub(crate) fn completion_contract<'a, T: AcceptanceRow + 'a>(
    own: &[String],
    born: &wire::TaskStamp,
    dependencies: &[wire::TaskStamp],
    rows: impl Iterator<Item = &'a T> + Clone,
) -> String {
    match project_acceptance(own, born, dependencies, rows) {
        Acceptance::Own => "Completion contract: own AC.".to_owned(),
        Acceptance::Linked(rows) => format!(
            "Completion contract: linked AC ({}).",
            rows.iter().map(|row| row.acceptance_handle()).collect::<Vec<_>>().join(", "),
        ),
        Acceptance::Missing => "Completion contract: unspecified. Work may continue; successful completion needs own AC, directly linked pending AC-bearing work in either direction, or an applicable no-work/duplicate/covered/deferred disposition. No allocation hold or phase change.".to_owned(),
    }
}

/// One claim's current completion contract, for that claim's exact occupant.
pub(crate) struct ClaimContract {
    pub(crate) lane: wire::CanonicalUuid,
    pub(crate) occupant: wire::CanonicalUuid,
    pub(crate) task: TaskHandle,
    pub(crate) claim: u64,
    pub(crate) contract: String,
}

/// The current completion contract of every active claim a lane took by hand
/// whose holder has acknowledged its task's current generation.
///
/// Linked work settling, or new work linking, moves such a contract without
/// moving the borrower's generation, so nothing else tells the holder. A
/// resident-minted claim hears it through its assignment notice, and a claim
/// trailing its generation through its generation notice. Only pending work
/// lends acceptance and only pending work carries an active claim, so the
/// projection reads the live frontier alone.
pub(crate) fn manual_claim_contracts(snapshot: &bus::TaskSnapshot) -> Vec<ClaimContract> {
    let held = snapshot
        .allocation_frontier()
        .filter_map(|heads| {
            let claim = heads.claim()?.body();
            (claim.disposition == ClaimDisposition::Active
                && claim.acknowledged >= heads.workflow().body().generation
                && !is_automatic_claim(claim, snapshot.facts()))
            .then_some((heads, claim))
        })
        .collect::<Vec<_>>();
    if held.is_empty() {
        return Vec::new();
    }
    let rows = snapshot.allocation_frontier().collect::<Vec<_>>();
    held.into_iter()
        .map(|(heads, claim)| {
            let statement = heads.statement().body();
            ClaimContract {
                lane: claim.lane,
                occupant: claim.occupant,
                task: TaskHandle::for_project(&statement.project, &statement.born),
                claim: claim.claimed.get(),
                contract: completion_contract(
                    &statement.acceptance,
                    &statement.born,
                    &statement.dependencies,
                    rows.iter().copied(),
                ),
            }
        })
        .collect()
}

/// Whether linked work in this state lends its acceptance to its neighbors.
pub(super) fn lends(acceptance: &[String], workflow: &wire::WorkflowHead) -> bool {
    workflow.status.is_pending() && substantive_acceptance(acceptance)
}

/// Whether a task with this statement and workflow lends its acceptance to a
/// member of `borrowers` by depending on it. Reads that project a borrower's
/// acceptance carry these pending lenders regardless of edge direction.
pub(super) fn lends_across(
    acceptance: &[String],
    workflow: &wire::WorkflowHead,
    dependencies: &[wire::TaskStamp],
    borrowers: &std::collections::BTreeSet<wire::TaskStamp>,
) -> bool {
    lends(acceptance, workflow)
        && dependencies
            .iter()
            .any(|dependency| borrowers.contains(dependency))
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
                 modify {handle} --acceptance TEXT`, or link pending work carrying acceptance with \
                 `spice task depends`; a Plan whose need warrants no work records why with \
                 `spice task done --disposition no-work|duplicate|covered|deferred [--cover \
                 HANDLE|COMMIT]`"
            ),
        )),
    }
}
