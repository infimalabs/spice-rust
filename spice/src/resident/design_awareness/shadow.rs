//! Shadow records: the best Design a session's cards selected at an
//! opportunity that fell short of its reach floor. A shadow is planned where a
//! request would otherwise have been evaluated and refused by its floor, and
//! is committed once per session per Design statement revision.

use journal::{GuardError, JournalRead};
use wire::rooms::{
    DesignNoticeEvidence, DesignNoticeOpportunity, DesignReach, DesignSession, DesignShadowed,
    FitGeneration, MatchScore,
};
use wire::{CanonicalUuid, Seq, TaskStamp};

use super::{STALE, awareness_write, commit_awareness};
use crate::resident::*;

/// The best Design one session's cards scored at an opportunity, which fell
/// short of its reach floor.
pub(super) struct Shadow {
    pub(super) design: TaskStamp,
    pub(super) statement: Seq,
    pub(super) evidence: DesignNoticeEvidence,
    pub(super) terms: Vec<String>,
    pub(super) reach: DesignReach,
    pub(super) floor: MatchScore,
    pub(super) score: MatchScore,
    pub(super) generation: FitGeneration,
}

impl Shadow {
    /// This shortfall as the record one pass commits for `session`.
    pub(super) fn planned(
        self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        session: DesignSession,
        opportunity: DesignNoticeOpportunity,
        horizon: Option<Seq>,
    ) -> PlannedShadow {
        PlannedShadow {
            lane,
            occupant,
            session,
            opportunity,
            evidence: self.evidence,
            design: self.design,
            statement: self.statement,
            terms: self.terms,
            reach: self.reach,
            floor: self.floor,
            score: self.score,
            generation: self.generation,
            horizon,
        }
    }
}

/// One shadow record and the journal position its planning read covered: the
/// best Design a session's cards selected at an opportunity fell short of its
/// reach floor, so no request is owed.
#[derive(Clone)]
pub(super) struct PlannedShadow {
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) session: DesignSession,
    pub(super) opportunity: DesignNoticeOpportunity,
    pub(super) evidence: DesignNoticeEvidence,
    pub(super) design: TaskStamp,
    pub(super) statement: Seq,
    pub(super) terms: Vec<String>,
    pub(super) reach: DesignReach,
    pub(super) floor: MatchScore,
    pub(super) score: MatchScore,
    pub(super) generation: FitGeneration,
    pub(super) horizon: Option<Seq>,
}

/// Commit one shadow record: the guard's stale cause when the Design moved or
/// the same shadow was recorded after the planning horizon.
pub(super) fn record_shadow(bus: &Bus, shadow: &PlannedShadow) -> CommandResult<Option<String>> {
    let mut write = awareness_write(shadow.lane)?;
    write
        .fact(
            DesignShadowed {
                design: shadow.design.clone(),
                statement: shadow.statement,
                lane: shadow.lane,
                occupant: shadow.occupant,
                session: shadow.session.clone(),
                opportunity: shadow.opportunity.clone(),
                evidence: shadow.evidence.clone(),
                terms: shadow.terms.clone(),
                reach: shadow.reach,
                floor: shadow.floor,
                score: shadow.score,
                generation: shadow.generation.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::awareness-build", &error.to_string()))?;
    let observed = shadow.clone();
    write.guard(journal::JournalGuard::new(move |read| {
        shadow_current(read, &observed)
    }));
    commit_awareness(bus, write)
}

/// Refuse a shadow whose Design statement moved or closed, or that another
/// pass already recorded for this session and statement.
fn shadow_current(read: &JournalRead<'_>, shadow: &PlannedShadow) -> Result<(), GuardError> {
    let stale = |why: &str| GuardError::refused(format!("{STALE}: {why}"));
    let Some(statement) = read.task_head(&shadow.design)? else {
        return Err(stale("the Design disappeared"));
    };
    if statement.body().revision != shadow.statement {
        return Err(stale("the Design statement moved"));
    }
    let Some(workflow) = read.workflow_head(&shadow.design)? else {
        return Err(stale("the Design workflow disappeared"));
    };
    if !statement.body().filing.is_filed()
        || !wire::intent::is_open_design(statement.body().intent, workflow.body())
        || workflow.body().deferral.is_some()
    {
        return Err(stale("the Design is no longer open"));
    }
    let mut after = shadow.horizon;
    loop {
        let page = read.selected_fact_page(
            &[wire::FactKind::DesignShadowed],
            after,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        for envelope in &page.rows {
            let Some(body) = envelope.fact.read::<DesignShadowed>() else {
                return Err(GuardError::Invalid(
                    "a selected Design shadow did not decode".to_owned(),
                ));
            };
            if body.lane == shadow.lane
                && body.session == shadow.session
                && body.design == shadow.design
                && body.statement == shadow.statement
            {
                return Err(stale("the session's shadow for this statement is recorded"));
            }
        }
        let Some(continuation) = page.continuation else {
            return Ok(());
        };
        after = Some(continuation);
    }
}
