//! Resident Design notice and return delivery records: background notices
//! that carry no room, task, steering, or claim authority.

use wire::rooms::{
    DESIGN_NOTICE_TERMS, DecisionOutcome, DesignNoticed, DesignReturnDelivered,
    PreparationDecision, ProposalRef,
};
use wire::{Actor, AnyFact};

use super::{Authority, Refusal, require_task, subject, unsound};

/// Admit one Design notice record, or `None` for another fact.
pub(super) fn fold(
    authority: &impl Authority,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(body) = fact.read::<DesignReturnDelivered>() {
        return Some(returned(authority, actor, &body));
    }
    fact.read::<DesignNoticed>()
        .map(|body| notice(authority, actor, &body))
}

/// A delivered return names a recorded return decision of its Design.
fn returned(
    authority: &impl Authority,
    actor: &Actor,
    body: &DesignReturnDelivered,
) -> Result<(), Refusal> {
    let design = &body.decision.controller;
    if *actor != Actor::Spice {
        return unsound(format!(
            "return delivery for {design} is resident-owned, not written by {actor}"
        ));
    }
    let key = subject(&ProposalRef::Decision {
        decision: body.decision.clone(),
    })?;
    let Some(recorded) = authority.proposal(&key)? else {
        return unsound(format!("return delivery for {design} names no decision"));
    };
    let decision: PreparationDecision = wire::decode(&recorded)?;
    if !matches!(decision.outcome, DecisionOutcome::Returned { .. }) {
        return unsound(format!(
            "return delivery for {design} names a decision that returned nothing"
        ));
    }
    Ok(())
}

fn notice(authority: &impl Authority, actor: &Actor, body: &DesignNoticed) -> Result<(), Refusal> {
    if *actor != Actor::Spice {
        return unsound(format!(
            "design awareness for {} is resident-owned, not written by {actor}",
            body.design
        ));
    }
    if body.design == body.task {
        return unsound(format!(
            "design awareness names {} as both the Design and the claimed task",
            body.design
        ));
    }
    require_task(authority, &body.design, "noticed design")?;
    require_task(authority, &body.task, "noticed claim task")?;
    if body.terms.is_empty() || body.terms.len() > DESIGN_NOTICE_TERMS {
        return unsound(format!(
            "design awareness for {} carries {} terms outside 1..={DESIGN_NOTICE_TERMS}",
            body.design,
            body.terms.len()
        ));
    }
    if body.terms.windows(2).any(|pair| pair[0] >= pair[1])
        || body.terms.iter().any(|term| {
            term.is_empty() || !term.chars().all(|c| c.is_alphabetic() && !c.is_uppercase())
        })
    {
        return unsound(format!(
            "design awareness for {} terms are not distinct ascending lowercase words",
            body.design
        ));
    }
    Ok(())
}
