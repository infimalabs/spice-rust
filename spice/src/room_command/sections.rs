//! Room history sections beside the publication page: the page's typed
//! contributions grouped by kind, and every operator decision on the anchor's
//! candidates in decision order.
//!
//! A decision is the anchor's own immutable record, never a room publication,
//! so it pages with neither publications nor catch-up. The newest return's
//! reason is the anchor's newest input; a decision mutates no candidate it
//! names.

use std::collections::VecDeque;
use std::fmt;

use serde::Serialize;
use wire::rooms::{DecisionOutcome, PreparationDecided, RoomAuthorityStamp, RoomContributionKind};
use wire::{Actor, Instant, Seq, SequenceCode, TaskStamp};

use super::{Content, Failure, Publication, fault, store};

/// Most decisions one room read shows, newest kept.
pub(super) const DECISION_ROWS: usize = 32;

/// The shown contributions of one kind, in journal order.
#[derive(Debug, Serialize)]
pub(crate) struct ContributionGroup {
    kind: RoomContributionKind,
    publications: Vec<RoomAuthorityStamp>,
}

/// `rows`' contributions grouped by kind in [`RoomContributionKind::ALL`]
/// order, each group in journal order.
pub(super) fn group(rows: &[Publication]) -> Vec<ContributionGroup> {
    RoomContributionKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let publications: Vec<_> = rows
                .iter()
                .filter(|row| {
                    matches!(&row.content, Content::Contribution { kind: shown, .. } if *shown == kind)
                })
                .map(|row| row.publication.clone())
                .collect();
            (!publications.is_empty()).then_some(ContributionGroup { kind, publications })
        })
        .collect()
}

impl fmt::Display for ContributionGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "\n{}:", self.kind)?;
        for publication in &self.publications {
            write!(formatter, " {publication}")?;
        }
        Ok(())
    }
}

/// The anchor's newest decisions, oldest first, and how many older ones the
/// read withholds.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Decisions {
    rows: VecDeque<Decision>,
    #[serde(skip_serializing_if = "is_zero")]
    withheld: usize,
}

/// One operator decision as the journal records it.
#[derive(Debug, Serialize)]
struct Decision {
    decision: SequenceCode,
    seq: Seq,
    at: Instant,
    actor: Actor,
    outcome: DecisionOutcome,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl Decisions {
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Every decision recorded on `anchor`'s candidates, keeping the newest
/// [`DECISION_ROWS`] in decision order.
pub(super) fn decisions(
    read: &journal::JournalRead<'_>,
    anchor: &TaskStamp,
) -> Result<Decisions, Failure> {
    let mut shown = Decisions::default();
    let mut after = None;
    loop {
        let page = read
            .preparation_decisions(anchor, after, journal::FactReadBudget::RETAINED)
            .map_err(store)?;
        for envelope in page.rows {
            let decided = envelope.fact.read::<PreparationDecided>().ok_or_else(|| {
                fault(
                    wire::FaultClass::Store,
                    "room::decision-invalid",
                    format!("decision {} is not a recorded decision", envelope.seq),
                )
            })?;
            shown.rows.push_back(Decision {
                decision: decided.decision.decision.decision,
                seq: envelope.seq,
                at: envelope.at,
                actor: envelope.actor,
                outcome: decided.decision.outcome,
            });
            if shown.rows.len() > DECISION_ROWS {
                shown.rows.pop_front();
                shown.withheld += 1;
            }
        }
        match page.continuation {
            Some(continuation) => after = Some(continuation),
            None => return Ok(shown),
        }
    }
}

impl fmt::Display for Decisions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.withheld > 0 {
            write!(formatter, "\ndecisions: {} older withheld", self.withheld)?;
        }
        for row in &self.rows {
            write!(
                formatter,
                "\ndecision {} seq={} at={} from={} ",
                row.decision, row.seq, row.at, row.actor
            )?;
            match &row.outcome {
                DecisionOutcome::Selected {
                    candidate,
                    selected_outputs,
                } => write!(
                    formatter,
                    "selected {} output(s) of candidate {}",
                    selected_outputs.len(),
                    candidate.candidate
                )?,
                DecisionOutcome::Returned { candidate, reason } => {
                    write!(formatter, "returned candidate {}", candidate.candidate)?;
                    write_reason(formatter, reason)?;
                }
                DecisionOutcome::Collapse { candidate, reason } => {
                    write!(formatter, "collapsed on candidate {}", candidate.candidate)?;
                    write_reason(formatter, reason)?;
                }
            }
        }
        Ok(())
    }
}

fn write_reason(formatter: &mut fmt::Formatter<'_>, reason: &str) -> fmt::Result {
    for line in reason.lines() {
        write!(formatter, "\n  | {line}")?;
    }
    Ok(())
}
