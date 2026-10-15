use super::*;
use wire::rooms::{
    CandidateCoverageRecord, CandidateCovered, CandidateCritiqueRecord, CandidateCritiqued,
    CandidateRetain, CandidateRetained, CoverageAuthority, DecisionOutcome, ExecutionAuthorization,
    ExecutionAuthorized, OutputAdopt, OutputControlled, PreparationDecide, PreparationDecided,
    ProposalRef, RetainedCandidate,
};

mod references;
mod withdrawal;

pub(super) fn dispatch_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Option<Outcome<CommandAnswer>> {
    Some(match command.body() {
        wire::intent::Intent::CandidateRetain(retain) => retain_candidate(bus, command, retain),
        wire::intent::Intent::CandidateCritiqueRecord(critique) => {
            record_critique(bus, command, critique)
        }
        wire::intent::Intent::CandidateCoverageRecord(coverage) => {
            record_coverage(bus, command, coverage)
        }
        wire::intent::Intent::PreparationDecide(decision) => {
            decide_preparation(bus, command, decision, projects, allocation)
        }
        wire::intent::Intent::OutputAdopt(adopt) => adopt_output(bus, command, adopt),
        wire::intent::Intent::CandidateWithdraw(withdraw) => {
            withdrawal::withdraw_candidate(bus, command, withdraw)
        }
        _ => return None,
    })
}

/// Refuse concluding a Design done while any live output it constructed
/// directly still lacks that Design's exact execution authorization.
///
/// A Plan concludes like any reviewed task; its outputs then wait only for
/// that positive boundary and ordinary dependencies. Collapse, operator close,
/// and delete are separate, explicit negative dispositions. They intentionally
/// leave outputs gated and do not call this guard.
pub(super) fn require_no_pending_controlled_outputs(
    snapshot: &ValidatedSnapshot,
    controller: &Entry,
) -> Result<(), Outcome<CommandAnswer>> {
    if controller.statement().intent != Some(wire::TaskIntent::Design) {
        return Ok(());
    }
    let mut outputs = snapshot
        .entries
        .iter()
        .filter(|entry| {
            board::unselected_output_of(entry.statement(), entry.workflow())
                == Some(controller.born())
        })
        .map(|entry| entry.handle().to_string())
        .collect::<Vec<_>>();
    if outputs.is_empty() {
        return Ok(());
    }
    outputs.sort();
    Err(refusal(
        "preparation::controlled-outputs-pending",
        Some(outputs.join(", ")),
        &format!(
            "select each intended output of {} in an exact candidate decision with `spice \
             preparation decide DECISION_JSON`, or close or delete every unselected output before \
             concluding it",
            controller.handle()
        ),
    ))
}

/// Refuse candidate selection authority over anything but a Design, and any
/// gather task offered as an output: a candidate cites its evidence instead.
fn require_design(
    bus: &Bus,
    controller: &TaskStamp,
    outputs: &[TaskStamp],
) -> Result<(), Outcome<CommandAnswer>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("preparation::heads-unreadable", &error.to_string()))?;
    design_controller(task_heads(&snapshot, controller)?, controller)?;
    for output in outputs {
        if let Some(heads) = snapshot.task_heads(output)
            && board::gather_task(heads.statement().body(), heads.workflow().body())
        {
            return Err(refusal(
                "preparation::gather-output",
                Some(output.to_string()),
                "a gather task lands nothing and is never an output; cite its notes or artifacts \
                 as candidate references instead",
            ));
        }
    }
    Ok(())
}

fn design_controller(
    heads: &journal::TaskHeads,
    controller: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    if heads.statement().body().intent == Some(wire::TaskIntent::Design) {
        return Ok(());
    }
    Err(refusal(
        "preparation::controller-not-design",
        Some(controller.to_string()),
        "retain candidates and authorize selections only under a Design; a Plan's outputs follow \
         its positive review",
    ))
}

/// Admit a decision's controller as a witnessed, open Design. Decisions leave
/// the Design open, so a lane holding it keeps its claim.
fn require_open_design(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
    controller: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    require_transition_witnesses(command, heads)?;
    design_controller(heads, controller)?;
    if heads.workflow().body().status != "pending" || heads.workflow().body().completion.is_some() {
        return Err(refusal(
            "preparation::controller-not-open",
            Some(controller.to_string()),
            "decide while the Design is open",
        ));
    }
    Ok(())
}

fn require_unblocked_selection(
    snapshot: &bus::TaskSnapshot,
    controller: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    for dependency in &controller.statement().body().dependencies {
        if task_heads(snapshot, dependency)?.workflow().body().status == "pending" {
            return Err(refusal(
                "preparation::controller-blocked",
                Some(dependency.to_string()),
                "settle the Design's pending dependencies before selecting its outputs",
            ));
        }
    }
    Ok(())
}

pub(super) fn retain_candidate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &CandidateRetain,
) -> Outcome<CommandAnswer> {
    let subject = ProposalRef::Candidate {
        candidate: request_body.candidate.candidate.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    if let Err(outcome) = require_design(
        bus,
        &request_body.candidate.candidate.controller,
        &request_body.candidate.outputs,
    ) {
        return outcome;
    }
    if let Err(outcome) = references::require_scoped_references(bus, &request_body.candidate) {
        return outcome;
    }
    record(
        bus,
        command,
        request_body,
        &request_body.candidate.candidate.controller,
        CandidateRetained {
            candidate: request_body.candidate.clone(),
        },
        Vec::new(),
        format!(
            "retained candidate {}",
            request_body.candidate.candidate.candidate
        ),
    )
}

pub(super) fn record_critique(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &CandidateCritiqueRecord,
) -> Outcome<CommandAnswer> {
    let subject = ProposalRef::Critique {
        critique: request_body.critique.critique.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    record(
        bus,
        command,
        request_body,
        &request_body.critique.critique.candidate.controller,
        CandidateCritiqued {
            critique: request_body.critique.clone(),
        },
        Vec::new(),
        format!(
            "recorded critique {}",
            request_body.critique.critique.critique
        ),
    )
}

pub(super) fn record_coverage(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &CandidateCoverageRecord,
) -> Outcome<CommandAnswer> {
    let subject = ProposalRef::Coverage {
        coverage: request_body.coverage.coverage.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    let assessed = ProposalRef::Candidate {
        candidate: request_body.coverage.coverage.candidate.clone(),
    };
    let assessed_read = assessed.clone();
    let observed = bus.read(move |read| {
        Ok::<_, GuardError>((
            read.proposal_head(&assessed_read)?,
            read.proposal_author(&assessed_read)?,
            read.residency_head()?,
        ))
    });
    let (candidate, author, residency) = match observed {
        Ok(observed) => observed,
        Err(error) => return store_fault("preparation::authority-unreadable", &error.to_string()),
    };
    let Some(candidate) = candidate else {
        return refusal(
            "preparation::candidate-missing",
            Some(
                request_body
                    .coverage
                    .coverage
                    .candidate
                    .candidate
                    .to_string(),
            ),
            "retain the exact candidate before recording coverage",
        );
    };
    let Some(author) = author else {
        return store_fault(
            "preparation::candidate-author-missing",
            "the retained candidate head has no authenticated writer",
        );
    };
    let mut derived = vec![Witness::Proposal {
        proposal: assessed,
        head: Some(candidate.position()),
    }];
    let authority = match command.actor() {
        Actor::Operator => CoverageAuthority::Operator {},
        Actor::Lane(lane) => {
            let Some(residency) = residency else {
                return refusal(
                    "preparation::reviewer-unseated",
                    Some(lane.to_string()),
                    "record coverage from a currently seated lane",
                );
            };
            let Some(row) = residency.body().lanes.iter().find(|row| row.lane == *lane) else {
                return refusal(
                    "preparation::reviewer-unseated",
                    Some(lane.to_string()),
                    "record coverage from a lane in the current resident generation",
                );
            };
            let Some(occupant) = row.occupant else {
                return refusal(
                    "preparation::reviewer-vacant",
                    Some(lane.to_string()),
                    "start the lane before recording its authored coverage",
                );
            };
            derived.push(Witness::Residency {
                lane: *lane,
                occupant: Some(occupant),
                epoch: residency.body().epoch,
            });
            if author == *command.actor() {
                CoverageAuthority::SelfReview {
                    lane: *lane,
                    occupant,
                }
            } else {
                CoverageAuthority::Independent {
                    lane: *lane,
                    occupant,
                }
            }
        }
        Actor::Spice => {
            return refusal(
                "preparation::reviewer-unavailable",
                Some(Actor::Spice.to_string()),
                "record coverage as the operator or a currently seated lane",
            );
        }
    };
    let mut coverage = request_body.coverage.clone();
    coverage.authority = authority;
    let controller = coverage.coverage.candidate.controller.clone();
    record(
        bus,
        command,
        request_body,
        &controller,
        CandidateCovered { coverage },
        derived,
        format!(
            "recorded coverage {}",
            request_body.coverage.coverage.coverage
        ),
    )
}

/// What one decision commits beyond its own immutable record.
enum Consequence<'a> {
    /// A selection authorizes exactly its chosen outputs.
    Authorize(Vec<TaskHead>, ExecutionAuthorization),
    /// A return is the Design's next input and moves nothing.
    Record,
    /// A collapse concludes its Design through the operator conclusion path.
    Conclude {
        design: TaskHandle,
        reason: String,
        projects: &'a ProjectPolicy,
        allocation: &'a board::NextConfig,
    },
}

pub(super) fn decide_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &PreparationDecide,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let decision = &request_body.decision;
    let subject = ProposalRef::Decision {
        decision: decision.decision.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    let candidate_ref = decision.outcome.candidate();
    let candidate = match retained_candidate(bus, command, candidate_ref) {
        Ok(candidate) => candidate,
        Err(outcome) => return outcome,
    };
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let controller = match task_heads(&snapshot, &candidate_ref.controller) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = require_open_design(command, controller, &candidate_ref.controller) {
        return outcome;
    }
    let consequence = match &decision.outcome {
        DecisionOutcome::Selected {
            selected_outputs, ..
        } => match prepare_selection(
            &snapshot,
            command,
            controller,
            decision,
            &candidate,
            selected_outputs,
        ) {
            Ok((outputs, authorization)) => Consequence::Authorize(outputs, authorization),
            Err(outcome) => return outcome,
        },
        DecisionOutcome::Returned { .. } => Consequence::Record,
        DecisionOutcome::Collapse { reason, .. } => Consequence::Conclude {
            design: TaskHandle::for_project(
                &controller.statement().body().project,
                &candidate_ref.controller,
            ),
            reason: reason.clone(),
            projects,
            allocation,
        },
    };
    commit_decision(bus, command, request_body, candidate, consequence)
}

/// The exact retained candidate a decision disposes of: present, witnessed
/// at its head, and not withdrawn by its author.
fn retained_candidate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    candidate: &wire::rooms::CandidateRef,
) -> Result<RetainedCandidate, Outcome<CommandAnswer>> {
    let subject = ProposalRef::Candidate {
        candidate: candidate.clone(),
    };
    let read_subject = subject.clone();
    let withdrawal = ProposalRef::Withdrawal {
        candidate: candidate.clone(),
    };
    let (head, withdrawn) = bus
        .read(move |read| {
            Ok::<_, GuardError>((
                read.proposal_head(&read_subject)?,
                read.proposal_head(&withdrawal)?.is_some(),
            ))
        })
        .map_err(|error| store_fault("preparation::authority-unreadable", &error.to_string()))?;
    let head = head.ok_or_else(|| {
        refusal(
            "preparation::candidate-missing",
            Some(candidate.candidate.to_string()),
            "retain the exact candidate version before deciding it",
        )
    })?;
    if withdrawn {
        return Err(refusal(
            "preparation::candidate-withdrawn",
            Some(candidate.candidate.to_string()),
            "its author withdrew this candidate; decide the corrected candidate instead",
        ));
    }
    require_witness(
        command,
        &Witness::Proposal {
            proposal: subject,
            head: Some(head.position()),
        },
    )?;
    match head.into_body() {
        journal::ProposalAuthority::Candidate(candidate) => Ok(candidate),
        _ => Err(store_fault(
            "preparation::candidate-invalid",
            "candidate authority decoded as another proposal family",
        )),
    }
}

fn prepare_selection(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    controller: &journal::TaskHeads,
    decision: &wire::rooms::PreparationDecision,
    candidate: &RetainedCandidate,
    selected_outputs: &[TaskStamp],
) -> Result<(Vec<TaskHead>, ExecutionAuthorization), Outcome<CommandAnswer>> {
    require_unblocked_selection(snapshot, controller)?;
    require_input_witnesses(snapshot, command, candidate)?;
    let outputs = selected_output_heads(
        snapshot,
        command,
        &candidate.candidate,
        candidate,
        selected_outputs,
    )?;
    let authorization = ExecutionAuthorization {
        decision: decision.decision.clone(),
        candidate: candidate.candidate.clone(),
        governed_inputs: candidate.governed_inputs.clone(),
        selected_outputs: selected_outputs.to_vec(),
        coverage: Vec::new(),
    };
    Ok((outputs, authorization))
}

fn require_input_witnesses(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate: &RetainedCandidate,
) -> Result<(), Outcome<CommandAnswer>> {
    for input in &candidate.governed_inputs {
        let heads = task_heads(snapshot, input)?;
        require_witness(
            command,
            &Witness::Task {
                task: input.clone(),
                head: heads.statement().position(),
            },
        )?;
    }
    Ok(())
}

fn selected_output_heads(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate_ref: &wire::rooms::CandidateRef,
    candidate: &RetainedCandidate,
    selected_outputs: &[TaskStamp],
) -> Result<Vec<TaskHead>, Outcome<CommandAnswer>> {
    if let Some(output) = selected_outputs
        .iter()
        .find(|output| !candidate.outputs.contains(output))
    {
        return Err(refusal(
            "preparation::selection-foreign-output",
            Some(output.to_string()),
            "select only outputs named by the exact retained candidate version",
        ));
    }
    selected_outputs
        .iter()
        .map(|output| selected_output_head(snapshot, command, candidate_ref, output))
        .collect()
}

fn selected_output_head(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate: &wire::rooms::CandidateRef,
    output: &TaskStamp,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    let heads = task_heads(snapshot, output)?;
    require_transition_witnesses(command, heads)?;
    require_witness(
        command,
        &Witness::Authorization {
            output: output.clone(),
            head: None,
        },
    )?;
    if !output_is_authorizable(heads, candidate, output) {
        return Err(refusal(
            "preparation::output-not-authorizable",
            Some(output.to_string()),
            "select a pending, unclaimed, unauthorized output this Design constructed",
        ));
    }
    Ok(heads.statement().body().clone())
}

fn output_is_authorizable(
    heads: &journal::TaskHeads,
    candidate: &wire::rooms::CandidateRef,
    output: &TaskStamp,
) -> bool {
    heads.workflow().body().status == "pending"
        && heads.workflow().body().completion.is_none()
        && !active_claim(heads)
        && heads.statement().body().authorization.is_none()
        && !board::gather_task(heads.statement().body(), heads.workflow().body())
        && heads.statement().body().engagement.is_none()
        && heads
            .statement()
            .body()
            .preparation
            .as_ref()
            .is_some_and(|preparation| {
                preparation.parent == candidate.controller && preparation.construction == *output
            })
}

fn commit_decision(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &PreparationDecide,
    candidate: RetainedCandidate,
    consequence: Consequence<'_>,
) -> Outcome<CommandAnswer> {
    let decision = &request_body.decision;
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&decision.decision.controller) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let decided = match write.fact(
        PreparationDecided {
            decision: decision.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("preparation::decision-invalid", &error.to_string()),
    };
    let action = match consequence {
        Consequence::Authorize(outputs, authorization) => {
            let authorized = match write.fact(
                ExecutionAuthorized {
                    authorization: authorization.clone(),
                },
                FactCause::Pending(decided),
            ) {
                Ok(fact) => fact,
                Err(error) => {
                    return store_fault("preparation::authorization-invalid", &error.to_string());
                }
            };
            let selected = outputs.len();
            for output in outputs {
                let decision = authorization.decision.clone();
                write.task_head(authorized, move |revision| TaskHead {
                    authorization: Some(decision),
                    revision,
                    ..output
                });
            }
            format!("selected {selected} output(s)")
        }
        Consequence::Record => "returned the candidate".to_owned(),
        Consequence::Conclude {
            design,
            reason,
            projects,
            allocation,
        } => {
            if let Err(outcome) = super::mutation::stage_collapse(
                bus, command, design, reason, projects, allocation, &mut write, decided, at,
            ) {
                return outcome;
            }
            "collapsed the Design".to_owned()
        }
    };
    let answer = TextAnswer {
        text: format!(
            "{action} through decision {} of candidate {}",
            decision.decision.decision, candidate.candidate.candidate
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::decision-receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}

pub(super) fn adopt_output(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &OutputAdopt,
) -> Outcome<CommandAnswer> {
    let (output, parent) = match adoption_stamps(request_body) {
        Ok(stamps) => stamps,
        Err(outcome) => return outcome,
    };
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let output_heads = match task_heads(&snapshot, &output) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    let parent_heads = match task_heads(&snapshot, &parent) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    for heads in [output_heads, parent_heads] {
        if let Err(outcome) = require_transition_witnesses(command, heads) {
            return outcome;
        }
    }
    if output_heads.workflow().body().status != "pending"
        || output_heads.workflow().body().completion.is_some()
        || active_claim(output_heads)
        || output_heads.statement().body().preparation.is_some()
        || output_heads.statement().body().authorization.is_some()
        || output_heads.statement().body().engagement.is_some()
        || output_heads.statement().body().intent.is_some()
    {
        return refusal(
            "preparation::adoption-output-ineligible",
            Some(request_body.output.to_string()),
            "adopt only pending, unclaimed, uncontrolled ordinary work; running or completed work remains reference-only",
        );
    }
    if parent_heads.workflow().body().status != "pending"
        || parent_heads.workflow().body().completion.is_some()
        || active_claim(parent_heads)
        || !requires_construction(
            parent_heads.statement().body().intent,
            &parent_heads.workflow().body().flow,
        )
    {
        return refusal(
            "preparation::adoption-controller-ineligible",
            Some(request_body.preparation.to_string()),
            "choose a pending, unclaimed Plan or Design context as the controller",
        );
    }
    let preparation = wire::TaskPreparation {
        parent: parent.clone(),
        controller: parent_heads
            .statement()
            .body()
            .preparation
            .as_ref()
            .map_or_else(
                || parent.clone(),
                |preparation| preparation.controller.clone(),
            ),
        construction: output.clone(),
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&output) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let fact = match write.fact(
        OutputControlled {
            output: output.clone(),
            preparation: preparation.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("preparation::adoption-invalid", &error.to_string()),
    };
    let head = TaskHead {
        preparation: Some(preparation),
        ..output_heads.statement().body().clone()
    };
    write.task_head(fact, move |revision| TaskHead { revision, ..head });
    let answer = TextAnswer {
        text: format!(
            "adopted {output} under controller {}",
            parent_heads.statement().body().born
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::adoption-receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}
fn adoption_stamps(
    request_body: &OutputAdopt,
) -> Result<(TaskStamp, TaskStamp), Outcome<CommandAnswer>> {
    let output = request_body.output.born().map_err(|error| {
        refusal(
            "preparation::output-invalid",
            Some(request_body.output.to_string()),
            &format!("name a task handle with a birth stamp: {error}"),
        )
    })?;
    let parent = request_body.preparation.born().map_err(|error| {
        refusal(
            "preparation::controller-invalid",
            Some(request_body.preparation.to_string()),
            &format!("name a preparation task handle with a birth stamp: {error}"),
        )
    })?;
    Ok((output, parent))
}

fn record<B, F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &B,
    controller: &TaskStamp,
    fact_body: F,
    derived: Vec<Witness>,
    text: String,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize + Clone,
    F: wire::Body,
{
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(controller) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if !derived.is_empty() {
        guard(&mut write, derived);
    }
    if let Err(error) = write.fact(fact_body, FactCause::Root) {
        return store_fault("preparation::fact-invalid", &error.to_string());
    }
    let answer = TextAnswer { text };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}

fn require_proposal_absence(
    command: &wire::AdmittedCommand,
    proposal: &ProposalRef,
) -> Result<(), Outcome<CommandAnswer>> {
    require_witness(
        command,
        &Witness::Proposal {
            proposal: proposal.clone(),
            head: None,
        },
    )
}

fn require_transition_witnesses(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    for witness in [
        Witness::Task {
            task: heads.statement().body().born.clone(),
            head: heads.statement().position(),
        },
        Witness::Workflow {
            task: heads.statement().body().born.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
        Witness::Claim {
            task: heads.statement().body().born.clone(),
            head: heads.claim().map(journal::JournalHead::position),
        },
    ] {
        require_witness(command, &witness)?;
    }
    Ok(())
}

fn require_witness(
    command: &wire::AdmittedCommand,
    witness: &Witness,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.expected().contains(witness) {
        return Ok(());
    }
    Err(refusal(
        "preparation::witness-missing",
        Some(format!("{witness:?}")),
        "read the current preparation authority and retry the exact command",
    ))
}

fn task_heads<'a>(
    snapshot: &'a bus::TaskSnapshot,
    task: &TaskStamp,
) -> Result<&'a journal::TaskHeads, Outcome<CommandAnswer>> {
    snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == *task)
        .ok_or_else(|| {
            refusal(
                "preparation::task-missing",
                Some(task.to_string()),
                "read the current board and name an existing task",
            )
        })
}

fn active_claim(heads: &journal::TaskHeads) -> bool {
    heads
        .claim()
        .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active)
}
