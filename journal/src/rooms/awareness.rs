//! Resident Design awareness records: background notices that carry no room,
//! task, steering, or claim authority.

use wire::rooms::{DESIGN_NOTICE_TERMS, DesignNoticed};
use wire::{Actor, AnyFact};

use super::{Authority, Refusal, require_task, unsound};

/// Admit one Design awareness record, or `None` for another fact.
pub(super) fn fold(
    authority: &impl Authority,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    fact.read::<DesignNoticed>()
        .map(|body| notice(authority, actor, &body))
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
