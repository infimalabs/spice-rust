//! Resident Design notice and return delivery records: background notices
//! that carry no room, task, steering, or claim authority.

use rusqlite::{Connection, OptionalExtension};
use wire::rooms::{
    DESIGN_NOTICE_TERMS, DesignNoticeEvidence, DesignNoticeOpportunity, DesignNoticed,
    DesignReturnDelivered, PreparationDecision, ProposalRef,
};
use wire::{Actor, AnyFact, ClaimDisposition, ClaimHead, Seq};

use super::{Authority, Refusal, require_task, subject, unsound};

/// Admit one Design notice record, or `None` for another fact.
pub(super) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(body) = fact.read::<DesignReturnDelivered>() {
        return Some(returned(authority, actor, &body));
    }
    fact.read::<DesignNoticed>()
        .map(|body| notice(authority, seq, actor, &body))
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

fn notice(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &DesignNoticed,
) -> Result<(), Refusal> {
    if *actor != Actor::Spice {
        return unsound(format!(
            "design awareness for {} is resident-owned, not written by {actor}",
            body.design
        ));
    }
    if let Some(compaction) = body.compaction
        && compaction >= seq
    {
        return unsound(format!(
            "design awareness for {} names compaction {compaction} at or after its own position {seq}",
            body.design
        ));
    }
    let referenced_tasks = [
        match &body.opportunity {
            DesignNoticeOpportunity::TaskCompletion { task, .. }
            | DesignNoticeOpportunity::TaskClaim { task, .. } => Some(task),
            DesignNoticeOpportunity::Conversation { .. }
            | DesignNoticeOpportunity::DesignStatement
            | DesignNoticeOpportunity::ProfileReady { .. }
            | DesignNoticeOpportunity::SessionResumed { .. } => None,
        },
        match &body.evidence {
            DesignNoticeEvidence::Task { task, .. }
            | DesignNoticeEvidence::TaskClaim { task, .. } => Some(task),
            DesignNoticeEvidence::Conversation { .. } => None,
        },
    ];
    if referenced_tasks
        .into_iter()
        .flatten()
        .any(|task| task == &body.design)
    {
        return unsound(format!(
            "design awareness names {} as both the Design and invitation evidence",
            body.design
        ));
    }
    require_task(authority, &body.design, "noticed design")?;
    for task in referenced_tasks.into_iter().flatten() {
        require_task(authority, task, "invitation evidence task")?;
    }
    let current = authority.task_statement(&body.design)?.ok_or_else(|| {
        Refusal::Unsound(format!("noticed Design {} has no statement", body.design))
    })?;
    if current != body.statement {
        return unsound(format!(
            "design awareness for {} names statement {} while its current head is {current}",
            body.design, body.statement
        ));
    }
    if body.session.identity.trim().is_empty() {
        return unsound(format!(
            "design awareness for {} names an empty vendor session identity",
            body.design
        ));
    }
    if body.excerpt.is_empty() || wire::rooms::design_excerpt(&body.excerpt) != body.excerpt {
        return unsound(format!(
            "design awareness for {} carries an empty or unbounded excerpt",
            body.design
        ));
    }
    let conversation_sources = [
        match &body.opportunity {
            DesignNoticeOpportunity::Conversation { source } => Some(source),
            _ => None,
        },
        match &body.evidence {
            DesignNoticeEvidence::Conversation { source } => Some(source),
            _ => None,
        },
    ];
    if conversation_sources
        .into_iter()
        .flatten()
        .any(|source| source.locus.source.is_none())
    {
        return unsound(format!(
            "design awareness for {} names conversation evidence without a tape generation",
            body.design
        ));
    }
    terms_and_ask(body)?;
    authority.change(super::Change::MaturityNotice {
        seq: seq.get(),
        notice: Box::new(body.clone()),
    })
}

/// Refuse a notice whose matched terms are not one bounded ascending set of
/// distinct lowercase words, or that asks about a concurred item.
fn terms_and_ask(body: &DesignNoticed) -> Result<(), Refusal> {
    let terms_valid = !body.terms.is_empty() && body.terms.len() <= DESIGN_NOTICE_TERMS;
    if !terms_valid {
        return unsound(format!(
            "design awareness for {} carries {} terms",
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
    if body
        .asks
        .iter()
        .any(|ask| ask.state == wire::rooms::DesignItemState::Concurred)
    {
        return unsound(format!(
            "design awareness for {} asks about a concurred question item",
            body.design
        ));
    }
    Ok(())
}
