//! Resident Design notice and return delivery records: background notices
//! that carry no room, task, steering, or claim authority.

use rusqlite::{Connection, OptionalExtension};
use wire::rooms::{
    DESIGN_NOTICE_TERMS, DesignNoticeProvenance, DesignNoticed, DesignReturnDelivered,
    PreparationDecision, ProposalRef,
};
use wire::{Actor, AnyFact, ClaimDisposition, ClaimHead};

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

/// Every return-delivery record still names the exact active claim it served.
///
/// The room fold proves the decision. This commit-wide check runs against the
/// current claim head in the same immediate transaction, immediately before
/// task heads can move.
pub(crate) fn require_deliveries<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    for body in facts
        .into_iter()
        .filter_map(|fact| fact.read::<DesignReturnDelivered>())
    {
        let design = &body.decision.controller;
        let claim = connection
            .query_row(
                "SELECT body FROM claim_heads WHERE stamp = ?1",
                [design.as_str()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|body| wire::decode::<ClaimHead>(&body))
            .transpose()?;
        if claim.is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Active
                && claim.claimed.get() == body.claim.get()
                && claim.actor == Actor::Lane(body.lane)
        }) {
            continue;
        }
        return unsound(format!(
            "return delivery for {design} names no active claim won at {} by lane {}",
            body.claim, body.lane
        ));
    }
    Ok(())
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
    if !decision.outcome.carries_feedback() {
        return unsound(format!(
            "return delivery for {design} names a decision carrying no holder feedback"
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
    let current = authority.task_statement(&body.design)?.ok_or_else(|| {
        Refusal::Unsound(format!("noticed Design {} has no statement", body.design))
    })?;
    if current != body.statement {
        return unsound(format!(
            "design awareness for {} names statement {} while its current head is {current}",
            body.design, body.statement
        ));
    }
    let terms_valid = match body.provenance {
        DesignNoticeProvenance::Organic => {
            !body.terms.is_empty() && body.terms.len() <= DESIGN_NOTICE_TERMS
        }
        DesignNoticeProvenance::Seeded => body.terms.is_empty(),
    };
    if !terms_valid {
        return unsound(format!(
            "{} design awareness for {} carries {} terms",
            body.provenance,
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
