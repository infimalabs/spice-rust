//! Design pass proposal and decision authority.

use std::collections::BTreeSet;

use wire::rooms::{
    CandidateWithdrawal, DecisionOutcome, DesignPassProposal, PreparationDecision, ProposalRef,
};
use wire::{Actor, Seq, TaskStamp};

use super::{
    Authority, AuthorizationRow, Change, Refusal, evidence, record, require_proposal,
    require_source, require_task, subject, unsound,
};
pub(super) fn retain(
    authority: &mut impl Authority,
    seq: Seq,
    candidate: DesignPassProposal,
) -> Result<(), Refusal> {
    let controller = &candidate.candidate.controller;
    require_task(authority, controller, "candidate controller")?;
    if authority.task_statement(controller)? != Some(candidate.statement) {
        return unsound(format!(
            "Design pass {} does not name controller {controller}'s current statement",
            candidate.candidate.candidate
        ));
    }
    // Historical proposals crossed from the predecessor protocol carry no
    // claim when their immutable receipts did not prove one. Current command
    // admission rejects that shape before it reaches the journal.
    if let Some(claim) = candidate.claim
        && authority.active_claim(controller)? != Some(claim)
    {
        return unsound(format!(
            "Design pass {} does not name controller {controller}'s active claim",
            candidate.candidate.candidate
        ));
    }
    for input in &candidate.governed_inputs {
        require_task(authority, input, "governed input")?;
    }
    for output in &candidate.outputs {
        match authority.task_dependencies(output)? {
            Some(dependencies) if dependencies.contains(controller) => {}
            Some(_) => {
                return unsound(format!(
                    "candidate output {output} is not gated by {controller}"
                ));
            }
            None => return unsound(format!("candidate output {output} does not exist")),
        }
    }
    for reference in &candidate.references {
        require_source(authority, seq, &reference.source)?;
    }
    evidence::require_candidate_evidence(authority, &candidate)?;
    let proposal = ProposalRef::Candidate {
        candidate: candidate.candidate.clone(),
    };
    record(
        authority,
        seq,
        &proposal,
        controller.to_string(),
        &candidate,
    )
}

pub(super) fn decide(
    authority: &mut impl Authority,
    seq: Seq,
    decision: PreparationDecision,
) -> Result<(), Refusal> {
    let controller = &decision.decision.controller;
    require_task(authority, controller, "decision controller")?;
    let candidate: DesignPassProposal = wire::decode(&require_proposal(
        authority,
        &ProposalRef::Candidate {
            candidate: decision.outcome.candidate().clone(),
        },
    )?)?;
    if let DecisionOutcome::Resolved { outputs, .. } = &decision.outcome
        && (outputs.len() != candidate.outputs.len()
            || outputs
                .iter()
                .zip(&candidate.outputs)
                .any(|(decision, candidate)| decision.output != *candidate))
    {
        return unsound("output resolution must name every candidate output in retained order");
    }
    match (&candidate.recommendation, &decision.outcome) {
        (_, DecisionOutcome::Returned { .. })
        | (wire::rooms::DesignPassRecommendation::Work, DecisionOutcome::Resolved { .. })
        | (
            wire::rooms::DesignPassRecommendation::Conclude
            | wire::rooms::DesignPassRecommendation::Collapse,
            DecisionOutcome::Accepted { .. },
        ) => {}
        _ => {
            return unsound(
                "decision disposition does not match the proposal recommendation".to_owned(),
            );
        }
    }
    // Returned decisions historically did not settle a candidate and remain
    // replayable when an old release later recorded a terminal decision for
    // that same candidate. Current terminal decisions remain unique.
    if matches!(
        &decision.outcome,
        DecisionOutcome::Resolved { .. } | DecisionOutcome::Accepted { .. }
    ) && authority.candidate_decided(decision.outcome.candidate())?
    {
        return unsound(format!(
            "candidate {} already has an operator decision",
            decision.outcome.candidate().candidate
        ));
    }
    let withdrawal = subject(&ProposalRef::Withdrawal {
        candidate: decision.outcome.candidate().clone(),
    })?;
    if authority.proposal(&withdrawal)?.is_some() {
        return unsound(format!(
            "candidate {} was withdrawn by its author before this decision",
            decision.outcome.candidate().candidate
        ));
    }
    require_source(authority, seq, &decision.source)?;
    let proposal = ProposalRef::Decision {
        decision: decision.decision.clone(),
    };
    record(authority, seq, &proposal, controller.to_string(), &decision)
}

/// Only a proposal's authenticated author withdraws it, once, while no
/// decision has disposed of it. The proposal stays untouched.
pub(super) fn withdraw(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    withdrawal: CandidateWithdrawal,
) -> Result<(), Refusal> {
    let candidate = ProposalRef::Candidate {
        candidate: withdrawal.candidate.clone(),
    };
    require_proposal(authority, &candidate)?;
    if authority.proposal_author(&subject(&candidate)?)?.as_ref() != Some(actor) {
        return unsound(format!(
            "candidate {} is withdrawn by {actor}, not by its author",
            withdrawal.candidate.candidate
        ));
    }
    let controller = withdrawal.candidate.controller.to_string();
    if authority.candidate_decided(&withdrawal.candidate)? {
        return unsound(format!(
            "candidate {} is already decided and cannot be withdrawn",
            withdrawal.candidate.candidate
        ));
    }
    let proposal = ProposalRef::Withdrawal {
        candidate: withdrawal.candidate.clone(),
    };
    record(authority, seq, &proposal, controller, &withdrawal)
}

pub(super) fn authorize(
    authority: &mut impl Authority,
    seq: Seq,
    authorization: wire::rooms::ExecutionAuthorization,
) -> Result<(), Refusal> {
    if !authorization.coverage.is_empty() {
        return unsound("current authorization cannot carry review coverage");
    }
    let decision_ref = ProposalRef::Decision {
        decision: authorization.decision.clone(),
    };
    let decision: PreparationDecision = wire::decode(&require_proposal(authority, &decision_ref)?)?;
    if decision.outcome.candidate() != &authorization.candidate {
        return unsound("authorization requires the exact candidate recorded by its decision");
    }
    let selected = decision
        .outcome
        .accepted_outputs()
        .cloned()
        .collect::<BTreeSet<_>>();
    if selected.is_empty() {
        return unsound("authorization requires a decision that accepts an output");
    }
    let candidate: DesignPassProposal = wire::decode(&require_proposal(
        authority,
        &ProposalRef::Candidate {
            candidate: authorization.candidate.clone(),
        },
    )?)?;
    let set = |tasks: &[TaskStamp]| tasks.iter().cloned().collect::<BTreeSet<_>>();
    if set(&authorization.governed_inputs) != set(&candidate.governed_inputs) {
        return unsound("authorization governed inputs differ from the candidate's");
    }
    if set(&authorization.selected_outputs) != selected {
        return unsound("authorization outputs differ from the decision's exact acceptance");
    }
    if let Some(output) = authorization
        .selected_outputs
        .iter()
        .find(|output| !candidate.outputs.contains(output))
    {
        return unsound(format!(
            "selected output {output} is not an output of the candidate"
        ));
    }
    let decision_key = subject(&decision_ref)?;
    if authority.decision_authorized(&decision_key)? {
        return unsound("decision already authorized a selection");
    }
    let outputs = authorization
        .selected_outputs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    authority.change(Change::Authorization {
        outputs,
        row: AuthorizationRow {
            decision: decision_key,
            seq: seq.get(),
            body: wire::encode(&authorization)?,
        },
    })
}
