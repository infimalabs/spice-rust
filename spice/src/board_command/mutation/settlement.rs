//! Operator-decided terminal settlement of a task that need not be held.
//!
//! Withdrawal (`deleted`) and closure (`completed`) share one shape: the
//! journal keeps the statement, so either is a terminal workflow status rather
//! than a removal. The row stops being allocatable, stops blocking its
//! dependents, and keeps its history and its stated text readable.
//!
//! A live claim refuses either by default, because taking work away from a
//! lane mid-flight is a decision an operator makes deliberately, not one they
//! discover afterwards. `force_claimed` makes it, and then the fact names the
//! actor whose claim was overridden and the claim is freed in the same
//! transaction -- a lane left holding a settled task would renew its lease
//! against a row nothing can ever advance.

use super::*;

/// The one settlement a command asks for.
#[derive(Clone, Copy)]
enum Settlement<'a> {
    Delete(&'a TaskDelete),
    Close(&'a TaskClose),
}

impl Settlement<'_> {
    fn verb(self) -> &'static str {
        match self {
            Self::Delete(_) => "delete",
            Self::Close(_) => "close",
        }
    }

    fn handle(&self) -> &TaskHandle {
        match self {
            Self::Delete(delete) => &delete.task,
            Self::Close(close) => &close.task,
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Delete(delete) => &delete.reason,
            Self::Close(close) => &close.validation,
        }
    }

    fn force_claimed(self) -> bool {
        match self {
            Self::Delete(delete) => delete.force_claimed,
            Self::Close(close) => close.force_claimed,
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Delete(_) => "deleted",
            Self::Close(_) => "completed",
        }
    }

    fn empty_refusal(self) -> (&'static str, &'static str) {
        match self {
            Self::Delete(_) => (
                "task::delete-reason-empty",
                "state why the task leaves the board",
            ),
            Self::Close(_) => (
                "task::close-validation-empty",
                "state the evidence or reason the task is complete",
            ),
        }
    }

    fn claimed_refusal(self) -> (&'static str, &'static str) {
        match self {
            Self::Delete(_) => (
                "task::delete-claimed",
                "rerun with --force-claimed to withdraw work a lane is holding",
            ),
            Self::Close(_) => (
                "task::close-claimed",
                "close with force_claimed to take the task from the lane holding it",
            ),
        }
    }

    fn fact(
        self,
        write: &mut JournalWrite,
        entry: &Entry,
        claimed: Option<Actor>,
    ) -> Result<PendingFact, Outcome<CommandAnswer>> {
        let task = entry.born().clone();
        let built = match self {
            Self::Delete(delete) => write.fact(
                body::TaskDeleted {
                    task,
                    reason: delete.reason.clone(),
                    claimed,
                },
                FactCause::Root,
            ),
            Self::Close(close) => write.fact(
                body::TaskClosed {
                    task,
                    validation: close.validation.clone(),
                    phase: entry.workflow().phase.clone(),
                    claimed,
                },
                FactCause::Root,
            ),
        };
        built.map_err(|error| {
            store_fault(&format!("task::{}-build", self.verb()), &error.to_string())
        })
    }

    fn receipt(
        self,
        write: &mut JournalWrite,
        command: &wire::AdmittedCommand,
        answer: wire::intent::TaskAnswer,
    ) -> Result<(), Outcome<CommandAnswer>> {
        let recorded = match self {
            Self::Delete(delete) => {
                write.command_result(request(command, delete.clone()), command.epoch(), answer)
            }
            Self::Close(close) => {
                write.command_result(request(command, close.clone()), command.epoch(), answer)
            }
        };
        recorded.map_err(|error| {
            store_fault(
                &format!("task::{}-receipt", self.verb()),
                &error.to_string(),
            )
        })
    }
}

/// Withdraw a task from the board with a recorded reason.
pub(in crate::board_command) fn task_delete(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    delete: &TaskDelete,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    settle(
        bus,
        command,
        Settlement::Delete(delete),
        projects,
        allocation,
    )
}

/// Close a task as completed on the operator's word, whatever phase remains.
///
/// Completion ordinarily rests on the claimant's own proof; this is the
/// operator's direct lever, so the stated validation stands in for it and the
/// fact records the phase the closure cut short.
pub(in crate::board_command) fn task_close(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    close: &TaskClose,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    settle(bus, command, Settlement::Close(close), projects, allocation)
}

fn settle(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    settlement: Settlement<'_>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(settlement.handle()),
        projects,
        allocation,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match admitted_entry(&snapshot, settlement) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let claimed = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
        .map(|claim| claim.actor.clone());
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let post = projected_settled_entry(entry, settlement.status(), at, claimed.is_some(), projects);
    let next = next_action(&snapshot, &post);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let settled = match settlement.fact(&mut write, entry, claimed.clone()) {
        Ok(fact) => fact,
        Err(outcome) => return outcome,
    };
    let workflow = post.workflow().clone();
    write.workflow_head(entry.born().clone(), settled, move |_| workflow);
    if claimed.is_some() {
        let claim = post
            .claim()
            .expect("a held claim remains as freed history")
            .clone();
        write.claim_head(entry.born().clone(), settled, move |fact| ClaimHead {
            fact,
            ..claim
        });
    }
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: settlement.status().to_owned(),
        completion: None,
        claim: None,
        next,
    };
    if let Err(outcome) = settlement.receipt(&mut write, command, answer.clone()) {
        return outcome;
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

/// The named, unsettled entry, refusing empty text and an unforced live claim.
fn admitted_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    settlement: Settlement<'_>,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    let Some(entry) = task_entry(&snapshot.entries, settlement.handle()) else {
        return Err(refusal(
            "task::unknown",
            Some(settlement.handle().to_string()),
            "read board and name one current task handle",
        ));
    };
    if settlement.text().trim().is_empty() {
        let (reason, remedy) = settlement.empty_refusal();
        return Err(refusal(reason, Some(entry.handle().to_string()), remedy));
    }
    match entry.workflow().status.as_str() {
        "completed" => {
            return Err(refusal(
                "task::settled",
                Some(entry.handle().to_string()),
                "a completed task is history and stays on the board",
            ));
        }
        "deleted" => {
            return Err(refusal(
                "task::already-deleted",
                Some(entry.handle().to_string()),
                "the task is already withdrawn; read its recorded reason",
            ));
        }
        _ => {}
    }
    if entry.is_draft() && matches!(settlement, Settlement::Close(_)) {
        return Err(refusal(
            "task::draft-unfiled",
            Some(filing_state(entry)),
            &format!(
                "file the Draft with spice task file {handle}, or withdraw it with spice task delete {handle}",
                handle = entry.handle(),
            ),
        ));
    }
    if let Some(claim) = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
        && !settlement.force_claimed()
    {
        let (reason, remedy) = settlement.claimed_refusal();
        return Err(refusal(reason, Some(claim.actor.to_string()), remedy));
    }
    Ok(entry)
}

fn projected_settled_entry(
    entry: &Entry,
    status: &str,
    at: Instant,
    release_claim: bool,
    projects: &ProjectPolicy,
) -> Entry {
    let mut workflow = entry.workflow().clone();
    workflow.status = status.to_owned();
    workflow.completion = Some(at);
    workflow.review = "closed".to_owned();
    workflow.assignment = None;
    let mut claim = entry.claim().cloned();
    if let Some(claim) = claim.as_mut()
        && release_claim
    {
        claim.disposition = ClaimDisposition::Freed;
    }
    Entry::new(
        entry.statement().clone(),
        workflow,
        claim,
        entry.lease().cloned(),
        projects,
    )
    .expect("settling an admitted task preserves a valid historical entry")
}
