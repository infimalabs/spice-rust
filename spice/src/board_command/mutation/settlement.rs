//! Operator-decided terminal settlement of a task that need not be held.
//!
//! Withdrawal (`deleted`), closure (`completed`), and a Design's conclusion
//! (`completed`, done or collapsed) share one shape: the journal keeps the
//! statement, so each is a terminal workflow status rather than a removal. The
//! row stops being allocatable, stops blocking its dependents, and keeps its
//! history and its stated text readable.
//!
//! A live claim refuses each by default, because taking work away from a
//! lane mid-flight is a decision an operator makes deliberately, not one they
//! discover afterwards. `force_claimed` makes it for withdrawal and closure,
//! and then the fact names the actor whose claim was overridden and the claim
//! is freed in the same transaction -- a lane left holding a settled task
//! would renew its lease against a row nothing can ever advance. A Design is
//! never concluded over its holder: the holder checkpoints and frees it first.

use super::*;

/// The one settlement a command asks for.
#[derive(Clone, Copy)]
enum Settlement<'a> {
    Delete(&'a TaskDelete),
    Close(&'a TaskClose),
    Conclude(&'a TaskConclude),
}

impl Settlement<'_> {
    fn verb(self) -> &'static str {
        match self {
            Self::Delete(_) => "delete",
            Self::Close(_) => "close",
            Self::Conclude(_) => "conclude",
        }
    }

    fn handle(&self) -> &TaskHandle {
        match self {
            Self::Delete(delete) => &delete.task,
            Self::Close(close) => &close.task,
            Self::Conclude(conclude) => &conclude.task,
        }
    }

    /// The stated reason this settlement requires, if it requires one.
    fn text(&self) -> Option<&str> {
        match self {
            Self::Delete(delete) => Some(&delete.reason),
            Self::Close(close) => Some(&close.validation),
            Self::Conclude(conclude) => match &conclude.conclusion {
                body::DesignConclusion::Done {} => None,
                body::DesignConclusion::Collapsed { reason } => Some(reason),
            },
        }
    }

    fn force_claimed(self) -> bool {
        match self {
            Self::Delete(delete) => delete.force_claimed,
            Self::Close(close) => close.force_claimed,
            Self::Conclude(_) => false,
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Delete(_) => "deleted",
            Self::Close(_) | Self::Conclude(_) => "completed",
        }
    }

    /// Whether this settlement approves nothing the task controls.
    fn negative(self) -> bool {
        !matches!(
            self,
            Self::Conclude(TaskConclude {
                conclusion: body::DesignConclusion::Done {},
                ..
            })
        )
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
            Self::Conclude(_) => (
                "task::collapse-reason-empty",
                "state why the Design's premise did not hold",
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
            Self::Conclude(_) => (
                "task::conclude-claimed",
                "the holder checkpoints its work and runs `spice task free`; conclude the Design \
                 once it is unheld",
            ),
        }
    }

    fn fact(
        self,
        write: &mut JournalWrite,
        entry: &Entry,
        claimed: Option<Actor>,
        cause: FactCause,
    ) -> Result<PendingFact, Outcome<CommandAnswer>> {
        let task = entry.born().clone();
        let built = match self {
            Self::Delete(delete) => write.fact(
                body::TaskDeleted {
                    task,
                    reason: delete.reason.clone(),
                    claimed,
                },
                cause,
            ),
            Self::Close(close) => write.fact(
                body::TaskClosed {
                    task,
                    validation: close.validation.clone(),
                    phase: entry.workflow().phase.clone(),
                    claimed,
                },
                cause,
            ),
            Self::Conclude(conclude) => write.fact(
                body::TaskConcluded {
                    task,
                    conclusion: conclude.conclusion.clone(),
                },
                cause,
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
            Self::Conclude(conclude) => {
                write.command_result(request(command, conclude.clone()), command.epoch(), answer)
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

/// Conclude an open, unheld Design done or collapsed on the operator's word.
///
/// Every pending output that still retains its Design dependency closes in
/// the same transaction; conclusion never releases unselected work.
pub(in crate::board_command) fn task_conclude(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    conclude: &TaskConclude,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    settle(
        bus,
        command,
        Settlement::Conclude(conclude),
        projects,
        allocation,
    )
}

/// Stage an accepted pass recommendation into the decision's own `write`,
/// through the same admission and heads as `task conclude`.
#[allow(
    clippy::too_many_arguments,
    reason = "the decision's write, cause, and instant join the conclusion's own inputs"
)]
pub(in crate::board_command) fn stage_conclusion(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    design: TaskHandle,
    conclusion: body::DesignConclusion,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    write: &mut JournalWrite,
    cause: PendingFact,
    at: Instant,
) -> Result<Vec<TaskHandle>, Outcome<CommandAnswer>> {
    let conclude = TaskConclude {
        task: design,
        conclusion,
    };
    let settlement = Settlement::Conclude(&conclude);
    let snapshot = validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(settlement.handle()),
        projects,
        allocation,
    )?;
    guard_task_projection(write, snapshot.task_barrier.clone());
    let entry = admitted_entry(&snapshot, settlement)?;
    let oops_generation = prepare_oops_settlement(write, &snapshot, entry)?;
    let cause = FactCause::Pending(cause);
    stage_with_controlled(
        write,
        &snapshot,
        entry,
        settlement,
        cause,
        at,
        projects,
        oops_generation,
    )
    .map(|(_, closed)| closed)
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
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if cascades_controlled(entry, settlement) {
        guard_task_projection(&mut write, snapshot.task_barrier.clone());
    }
    let oops_generation = match prepare_oops_settlement(&mut write, &snapshot, entry) {
        Ok(generation) => generation,
        Err(outcome) => return outcome,
    };
    let (post, closed) = match stage_with_controlled(
        &mut write,
        &snapshot,
        entry,
        settlement,
        FactCause::Root,
        at,
        projects,
        oops_generation,
    ) {
        Ok(staged) => staged,
        Err(outcome) => return outcome,
    };
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: settlement.status().to_owned(),
        completion: None,
        claim: None,
        closed,
        next: next_action(&snapshot, &post),
    };
    if let Err(outcome) = settlement.receipt(&mut write, command, answer.clone()) {
        return outcome;
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}

/// Stage one terminal task and every pending subtree its disposition can no
/// longer release.
#[expect(
    clippy::too_many_arguments,
    reason = "one settlement stages its entry, cause, instant, policy, and oops generation in one write"
)]
fn stage_with_controlled(
    write: &mut JournalWrite,
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    settlement: Settlement<'_>,
    cause: FactCause,
    at: Instant,
    projects: &ProjectPolicy,
    oops_generation: Option<wire::OopsGenerationHead>,
) -> Result<(Entry, Vec<TaskHandle>), Outcome<CommandAnswer>> {
    let controlled = if cascades_controlled(entry, settlement) {
        board::controlled_pending(&snapshot.entries, entry.born())
    } else {
        Default::default()
    };
    require_controlled_unheld(&controlled, settlement)?;
    let (post, settled) = stage(
        write,
        entry,
        settlement,
        cause,
        at,
        projects,
        oops_generation,
    )?;
    if controlled.is_empty() {
        return Ok((post, Vec::new()));
    }
    let validation = controlled_validation(entry, settlement);
    let mut closed = Vec::with_capacity(controlled.len());
    for controlled in controlled {
        let close = TaskClose {
            task: controlled.handle().clone(),
            validation: validation.clone(),
            force_claimed: settlement.force_claimed(),
        };
        stage(
            write,
            controlled,
            Settlement::Close(&close),
            FactCause::Pending(settled),
            at,
            projects,
            None,
        )?;
        closed.push(controlled.handle().clone());
    }
    Ok((post, closed))
}

/// Stage the settlement fact and the settled workflow, and free a forced
/// claim, returning the settled entry and its causal fact.
fn stage(
    write: &mut JournalWrite,
    entry: &Entry,
    settlement: Settlement<'_>,
    cause: FactCause,
    at: Instant,
    projects: &ProjectPolicy,
    oops_generation: Option<wire::OopsGenerationHead>,
) -> Result<(Entry, PendingFact), Outcome<CommandAnswer>> {
    let claimed = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active)
        .map(|claim| claim.actor.clone());
    let post = projected_settled_entry(entry, settlement, at, claimed.is_some(), projects);
    let settled = settlement.fact(write, entry, claimed.clone(), cause)?;
    let workflow = post.workflow().clone();
    write.workflow_head(entry.born().clone(), settled, move |_| workflow);
    if let Some(generation) = oops_generation {
        let revision = generation.revision.checked_add(1).ok_or_else(|| {
            store_fault(
                "task::oops-generation-exhausted",
                "the OOPS generation revision cannot advance",
            )
        })?;
        write
            .fact(
                body::OopsGenerationAdvanced {
                    revision,
                    from: generation.design,
                    to: None,
                },
                FactCause::Pending(settled),
            )
            .map_err(|error| store_fault("task::oops-generation-build", &error.to_string()))?;
    }
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
    Ok((post, settled))
}

fn prepare_oops_settlement(
    write: &mut JournalWrite,
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
) -> Result<Option<wire::OopsGenerationHead>, Outcome<CommandAnswer>> {
    if entry.statement().intent != Some(wire::TaskIntent::OpportunisticDesign) {
        return Ok(None);
    }
    let Some(observed) = snapshot.answer.oops_generation.as_ref() else {
        return Err(protocol_fault(
            "task::oops-generation-missing",
            "the current opportunistic Design has no generation authority",
        ));
    };
    if observed.body.design.as_ref() != Some(entry.born()) {
        return Err(refusal(
            "task::oops-generation-moved",
            Some(entry.handle().to_string()),
            "read the current OOPS generation and retry its settlement",
        ));
    }
    let position = observed.position;
    let design = observed.body.design.clone();
    write.guard(JournalGuard::new(move |read| {
        let current = read.oops_generation()?;
        if current.as_ref().map(journal::JournalHead::position) == Some(position)
            && current.and_then(|head| head.into_body().design) == design
        {
            Ok(())
        } else {
            Err(GuardError::refused(
                "task::oops-generation-moved: OOPS generation changed before settlement",
            ))
        }
    }));
    Ok(Some(observed.body.clone()))
}

fn require_controlled_unheld(
    controlled: &[&Entry],
    settlement: Settlement<'_>,
) -> Result<(), Outcome<CommandAnswer>> {
    let Some((entry, claim)) = controlled.iter().find_map(|entry| {
        entry
            .claim()
            .filter(|claim| claim.disposition == ClaimDisposition::Active)
            .map(|claim| (*entry, claim))
    }) else {
        return Ok(());
    };
    if settlement.force_claimed() {
        return Ok(());
    }
    let remedy = if matches!(settlement, Settlement::Conclude(_)) {
        "the holder checkpoints its work and runs `spice task free`; retry the Design conclusion once every controlled task is unheld"
    } else {
        "rerun with --force-claimed to settle the controller and take every controlled task from its holder"
    };
    Err(refusal(
        "task::controlled-descendant-claimed",
        Some(format!("{} held by {}", entry.handle(), claim.actor)),
        remedy,
    ))
}

fn cascades_controlled(entry: &Entry, settlement: Settlement<'_>) -> bool {
    entry.statement().intent.is_some_and(|intent| {
        intent.is_design() || (intent == wire::TaskIntent::Plan && settlement.negative())
    })
}

fn controlled_validation(controller: &Entry, settlement: Settlement<'_>) -> String {
    let detail = match settlement {
        Settlement::Delete(delete) => format!("was deleted: {}", delete.reason.trim()),
        Settlement::Close(close) => format!("was closed: {}", close.validation.trim()),
        Settlement::Conclude(TaskConclude {
            conclusion: body::DesignConclusion::Done {},
            ..
        }) => "was concluded before selecting this output".to_owned(),
        Settlement::Conclude(TaskConclude {
            conclusion: body::DesignConclusion::Collapsed { reason },
            ..
        }) => format!("was collapsed: {}", reason.trim()),
    };
    format!("Controller {} {detail}", controller.handle())
}

/// The named, unsettled entry, refusing empty text, a settlement that does not
/// fit its intent, and an unforced live claim.
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
    if settlement.text().is_some_and(|text| text.trim().is_empty()) {
        let (reason, remedy) = settlement.empty_refusal();
        return Err(refusal(reason, Some(entry.handle().to_string()), remedy));
    }
    if entry.is_draft() && !matches!(settlement, Settlement::Delete(_)) {
        return Err(refusal(
            "task::draft-unfiled",
            Some(filing_state(entry)),
            &format!(
                "file the Draft with spice task file {handle}, or withdraw it with spice task delete {handle}",
                handle = entry.handle(),
            ),
        ));
    }
    let design = entry
        .statement()
        .intent
        .is_some_and(wire::TaskIntent::is_design);
    match settlement {
        Settlement::Conclude(_) if !design => {
            return Err(refusal(
                "task::conclude-not-design",
                Some(entry.handle().to_string()),
                "conclude only a Design; close or delete other work",
            ));
        }
        _ => {}
    }
    if entry.workflow().status.is_completed() {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "a completed task is history and stays on the board",
        ));
    }
    if entry.workflow().status.is_deleted() {
        return Err(refusal(
            "task::already-deleted",
            Some(entry.handle().to_string()),
            "the task is already withdrawn; read its recorded reason",
        ));
    }
    if matches!(settlement, Settlement::Conclude(_))
        && !wire::intent::is_open_design(entry.statement().intent, entry.workflow())
    {
        return Err(refusal(
            "task::conclude-not-open",
            Some(entry.handle().to_string()),
            "conclude a Design only after it reaches its open design phase",
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
    settlement: Settlement<'_>,
    at: Instant,
    release_claim: bool,
    projects: &ProjectPolicy,
) -> Entry {
    let mut workflow = entry.workflow().clone();
    let conclusion = match settlement {
        Settlement::Delete(_) => {
            wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Deleted)
        }
        Settlement::Close(_)
        | Settlement::Conclude(TaskConclude {
            conclusion: body::DesignConclusion::Collapsed { .. },
            ..
        }) => wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed),
        Settlement::Conclude(TaskConclude {
            conclusion: body::DesignConclusion::Done {},
            ..
        }) => {
            let review = match workflow.status.review() {
                Some(wire::WorkflowReview::Clean) => wire::WorkflowCompletionReview::Clean,
                Some(wire::WorkflowReview::Unassigned | wire::WorkflowReview::Assigned { .. }) => {
                    wire::WorkflowCompletionReview::Unassigned
                }
                None => unreachable!("admission accepts only a pending workflow"),
            };
            wire::WorkflowConclusion::Positive { review }
        }
    };
    workflow.status = wire::WorkflowStatus::Settled { at, conclusion };
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
    .with_grouped(entry.grouped())
    .with_design(entry.design())
}
