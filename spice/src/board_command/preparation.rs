use super::*;
use wire::rooms::{
    CandidateCoverageRecord, CandidateCovered, CandidateCritiqueRecord, CandidateCritiqued,
    CandidateRetain, CandidateRetained, CoverageAuthority, ExecutionAuthorize, ExecutionAuthorized,
    OutputAdopt, OutputControlled, PreparationDecide, PreparationDecided, ProposalRef,
};
pub(super) fn dispatch_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
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
            decide_preparation(bus, command, decision)
        }
        wire::intent::Intent::ExecutionAuthorize(authorization) => {
            authorize_execution(bus, command, authorization)
        }
        wire::intent::Intent::OutputAdopt(adopt) => adopt_output(bus, command, adopt),
        _ => return None,
    })
}

/// Refuse a positive controller conclusion while any live controlled output
/// still lacks that controller's exact execution authorization.
///
/// Operator close and delete are separate, explicit negative dispositions.
/// They intentionally leave controlled outputs gated and do not call this
/// guard.
pub(super) fn require_no_pending_controlled_outputs(
    snapshot: &ValidatedSnapshot,
    controller: &Entry,
) -> Result<(), Outcome<CommandAnswer>> {
    let mut outputs = snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.workflow().status == "pending"
                && entry.statement().authorization.is_none()
                && entry
                    .statement()
                    .preparation
                    .as_ref()
                    .is_some_and(|preparation| preparation.controller == *controller.born())
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
            "authorize the exact accepted selection with `spice preparation authorize \
             AUTHORIZATION_JSON`, or close or delete each unselected output before concluding {}",
            controller.handle()
        ),
    ))
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

pub(super) fn decide_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &PreparationDecide,
) -> Outcome<CommandAnswer> {
    let subject = ProposalRef::Decision {
        decision: request_body.decision.decision.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    record(
        bus,
        command,
        request_body,
        &request_body.decision.decision.controller,
        PreparationDecided {
            decision: request_body.decision.clone(),
        },
        Vec::new(),
        format!(
            "recorded decision {}",
            request_body.decision.decision.decision
        ),
    )
}

pub(super) fn authorize_execution(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &ExecutionAuthorize,
) -> Outcome<CommandAnswer> {
    let authorization = &request_body.authorization;
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let controller = match task_heads(&snapshot, &authorization.candidate.controller) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = require_transition_witnesses(command, controller) {
        return outcome;
    }
    if controller.workflow().body().status != "pending"
        || controller.workflow().body().completion.is_some()
        || active_claim(controller)
    {
        return refusal(
            "preparation::controller-not-open",
            Some(authorization.candidate.controller.to_string()),
            "free any active claim and authorize while the preparation controller is pending",
        );
    }
    let mut outputs = Vec::with_capacity(authorization.selected_outputs.len());
    for output in &authorization.selected_outputs {
        let heads = match task_heads(&snapshot, output) {
            Ok(heads) => heads,
            Err(outcome) => return outcome,
        };
        if let Err(outcome) = require_transition_witnesses(command, heads) {
            return outcome;
        }
        let absence = Witness::Authorization {
            output: output.clone(),
            head: None,
        };
        if let Err(outcome) = require_witness(command, &absence) {
            return outcome;
        }
        if heads.workflow().body().status != "pending"
            || heads.workflow().body().completion.is_some()
            || active_claim(heads)
            || heads.statement().body().authorization.is_some()
            || heads
                .statement()
                .body()
                .preparation
                .as_ref()
                .is_none_or(|preparation| {
                    preparation.controller != authorization.candidate.controller
                })
        {
            return refusal(
                "preparation::output-not-authorizable",
                Some(output.to_string()),
                "select a pending, unclaimed, unauthorized output controlled by this preparation",
            );
        }
        outputs.push(heads.statement().body().clone());
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&authorization.candidate.controller) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let fact = match write.fact(
        ExecutionAuthorized {
            authorization: authorization.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("preparation::authorization-invalid", &error.to_string()),
    };
    let workflow = controller.workflow().body().clone();
    let controller_stamp = authorization.candidate.controller.clone();
    let completion = at;
    write.workflow_head(controller_stamp, fact, move |_| WorkflowHead {
        status: "completed".to_owned(),
        completion: Some(completion),
        assignment: None,
        ..workflow
    });
    for output in outputs {
        let decision = authorization.decision.clone();
        write.task_head(fact, move |revision| TaskHead {
            authorization: Some(decision),
            revision,
            ..output
        });
    }
    let answer = TextAnswer {
        text: format!(
            "authorized {} output(s) through decision {}",
            authorization.selected_outputs.len(),
            authorization.decision.decision
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault(
            "preparation::authorization-receipt-invalid",
            &error.to_string(),
        );
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
    let head = output_heads.statement().body().clone();
    write.task_head(fact, move |revision| TaskHead {
        preparation: Some(preparation),
        revision,
        ..head
    });
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
