//! Resident-owned measure of tool calls an occupant runs without visible prose.
//!
//! The predecessor counted assistant messages that carried tool calls and no
//! text and spoke up after twelve in a row. This fold keeps that shape with a
//! roomier bound: every tool call an occupant issues inside its turn counts
//! toward a streak that only authored prose resets, and each time the streak
//! crosses another multiple of the bound the resident records one prod. The
//! prod is a durable fact the occupant's own `spice run` and hook meters read
//! back, not a directive, so nothing here asks for an acknowledgment.

use std::collections::BTreeMap;
use std::io;

use bus::Bus;
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use wire::body::{NarrationCleared, NarrationProdded, NarrationRelief};
use wire::{Actor, CanonicalUuid, Instant, Outcome, Producer};

use super::{CommandResult, commit, environment_fault, store_fault};

#[cfg(test)]
mod tests;

/// Per-lane silent streaks, folded from live capture records in tape order.
#[derive(Debug, Default)]
pub(super) struct Streaks {
    lanes: BTreeMap<CanonicalUuid, Streak>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Streak {
    occupant: CanonicalUuid,
    moments: u32,
    prods: u32,
}

/// One durable step the fold asks the resident to record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Step {
    Prodded(NarrationProdded),
    Cleared(NarrationCleared),
}

impl Streaks {
    /// Fold one record under `bound` tool calls per prod, in event order.
    ///
    /// A turn start or completion ends whatever streak the lane carried; an
    /// occupant change forgets it without a clearance, because the meter only
    /// reads prods addressed to the seated occupant. A record whose events the
    /// listener could not type changes nothing. A bound of zero is the closing
    /// sweep of a vacated seat: the lane's streak is forgotten outright.
    pub(super) fn observe(
        &mut self,
        bound: u32,
        record: &lane::capture::CaptureRecord,
    ) -> Vec<Step> {
        let mut steps = Vec::new();
        if bound == 0 {
            self.lanes.remove(&record.lane);
            return steps;
        }
        let Some(context) = record.decoded.context() else {
            return steps;
        };
        let streak = self.lanes.entry(record.lane).or_insert_with(|| Streak {
            occupant: record.occupant,
            moments: 0,
            prods: 0,
        });
        if streak.occupant != record.occupant {
            *streak = Streak {
                occupant: record.occupant,
                moments: 0,
                prods: 0,
            };
        }
        if context.turn == Some(listen::TurnMoment::Started) {
            streak.clear(record, NarrationRelief::Turn, &mut steps);
        }
        for event in record.decoded.events() {
            match event {
                listen::Event::Prose(prose)
                    if prose.role == listen::ProseRole::Assistant
                        && !prose.text.trim().is_empty() =>
                {
                    streak.clear(record, NarrationRelief::Prose, &mut steps);
                }
                listen::Event::Tool(tool)
                    if matches!(tool.moment, listen::ToolMoment::Call { .. }) =>
                {
                    streak.moments = streak.moments.saturating_add(1);
                    let due = u64::from(bound) * (u64::from(streak.prods) + 1);
                    if u64::from(streak.moments) >= due {
                        streak.prods = streak.prods.saturating_add(1);
                        steps.push(Step::Prodded(NarrationProdded {
                            lane: record.lane,
                            occupant: record.occupant,
                            moments: streak.moments,
                            count: streak.prods,
                        }));
                    }
                }
                _ => {}
            }
        }
        if context.turn == Some(listen::TurnMoment::Completed) {
            streak.clear(record, NarrationRelief::Turn, &mut steps);
        }
        steps
    }
}

impl Streak {
    fn clear(
        &mut self,
        record: &lane::capture::CaptureRecord,
        relief: NarrationRelief,
        steps: &mut Vec<Step>,
    ) {
        if self.prods > 0 {
            steps.push(Step::Cleared(NarrationCleared {
                lane: record.lane,
                occupant: record.occupant,
                moments: self.moments,
                prods: self.prods,
                relief,
            }));
        }
        self.moments = 0;
        self.prods = 0;
    }
}

/// Record every step one record produced, each guarded on the occupant still
/// holding its seat under this epoch.
///
/// A refusal means the seat moved between the fold and the commit; the
/// streak it described is over either way, so it is diagnosed and dropped.
pub(super) fn record(
    bus: &Bus,
    epoch: CanonicalUuid,
    steps: Vec<Step>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for step in steps {
        let (lane, occupant) = match &step {
            Step::Prodded(prodded) => (prodded.lane, prodded.occupant),
            Step::Cleared(cleared) => (cleared.lane, cleared.occupant),
        };
        let origin = format!("lane:{lane}")
            .parse::<wire::Origin>()
            .map_err(|error| store_fault("narration::origin-invalid", &error.to_string()))?;
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))?;
        let mut proposal =
            JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
        proposal.guard(JournalGuard::new(move |read| {
            let residency = read
                .residency_head()?
                .ok_or_else(|| GuardError::refused("narration::residency-missing"))?;
            if residency.body().epoch != epoch
                || !residency
                    .body()
                    .lanes
                    .iter()
                    .any(|row| row.lane == lane && row.occupant == Some(occupant))
            {
                return Err(GuardError::refused("narration::occupant-moved"));
            }
            Ok(())
        }));
        let built = match step {
            Step::Prodded(prodded) => proposal.fact(prodded, FactCause::Root),
            Step::Cleared(cleared) => proposal.fact(cleared, FactCause::Root),
        };
        built.map_err(|error| store_fault("narration::step-invalid", &error.to_string()))?;
        match commit(bus, proposal) {
            Ok(_) => {}
            Err(Outcome::Refusal(refusal)) => {
                let _ = writeln!(
                    diagnostic,
                    "narration step not recorded for lane {lane}: {}",
                    refusal.reason()
                );
            }
            Err(outcome) => return Err(outcome),
        }
    }
    Ok(())
}
