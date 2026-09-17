//! Consent requests a lane's occupant raises and the answers that reach it.
//!
//! An omp occupant that opens a confirm dialog holds its turn until someone
//! answers. Its supervisor states the question as one consent request, and
//! the resident records the ask together with what policy makes of it: a
//! grant or a denial answers the dialog at once, while a queue leaves the
//! request to the operator, whose ruling answers it later. Either answer
//! crosses the lane's kick door, and an occupant that has since left took
//! its dialog with it, exactly as it takes a missed inbox kick.

use std::io::Write;

use bus::Bus;
use journal::FactCause;
use wire::body::ConsentAsked;
use wire::{Actor, CanonicalUuid, Outcome, Producer, ResidencyHead};

use super::read_materializations::CurrentReads;
use super::supervisor::command;
use super::{CommandResult, RuntimePaths, commit, store_fault, write_at_origin};

/// One confirm dialog a seated occupant opened, as its supervisor stated it.
pub(super) struct LaneAsk {
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) request: CanonicalUuid,
    pub(super) operation: String,
}

/// Record one lane's ask with its policy outcome and answer what is answered.
///
/// A refusal is written down rather than raised: the dialog then waits on
/// nothing durable, and the occupant's turn holds until it exits, which is
/// the same standing an unanswered prompt always had.
///
/// # Errors
///
/// Returns the fault of a journal that could not be read or written.
pub(super) fn ask(
    bus: &Bus,
    reads: &CurrentReads,
    paths: &RuntimePaths,
    policy: &consent::Policy,
    residency: &ResidencyHead,
    ask: LaneAsk,
    diagnostic: &mut dyn Write,
) -> CommandResult<()> {
    let seated = residency
        .lanes
        .iter()
        .any(|row| row.lane == ask.lane && row.occupant == Some(ask.occupant));
    if !seated {
        let _ = writeln!(
            diagnostic,
            "lane {} consent request {} from occupant {} after its seat closed",
            ask.lane, ask.request, ask.occupant
        );
        return Ok(());
    }
    let (lane, request) = (ask.lane, ask.request);
    match decide(bus, reads, policy, ask) {
        Ok(Some(confirmed)) => command::answer(paths, lane, request, confirmed),
        Ok(None) => {}
        Err(Outcome::Refusal(refusal)) => {
            let _ = writeln!(
                diagnostic,
                "lane {lane} consent request {request} refused: {}: {}",
                refusal.reason(),
                refusal.explanation()
            );
        }
        Err(other) => return Err(other),
    }
    Ok(())
}

/// Commit the ask and its outcome; the answer, when policy already gave one.
fn decide(
    bus: &Bus,
    reads: &CurrentReads,
    policy: &consent::Policy,
    ask: LaneAsk,
) -> CommandResult<Option<bool>> {
    let generation = reads
        .current()
        .map_err(|error| store_fault("consent::materialization-unavailable", &error))?;
    let asked = ConsentAsked {
        request: ask.request,
        lane: ask.lane,
        operation: ask.operation,
    };
    let outcome = generation
        .consent()
        .decide(policy, &asked)
        .map_err(Outcome::Refusal)?;
    let origin = format!("lane:{}", ask.lane)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("consent::origin-invalid", &error.to_string()))?;
    let mut proposal = write_at_origin(Actor::Lane(ask.lane), origin, Producer::CommandDoor)?;
    proposal.guard(super::policy_surface::consent_guard(generation.cursor()));
    let asked = proposal
        .fact(asked, FactCause::Root)
        .map_err(|error| store_fault("consent::fact-invalid", &error.to_string()))?;
    let cause = FactCause::Pending(asked);
    let recorded = match &outcome {
        consent::Outcome::Granted(granted) => proposal.fact(granted.clone(), cause),
        consent::Outcome::Denied(denied) => proposal.fact(denied.clone(), cause),
        consent::Outcome::Queued(queued) => proposal.fact(queued.clone(), cause),
    };
    recorded.map_err(|error| store_fault("consent::fact-invalid", &error.to_string()))?;
    commit(bus, proposal)?;
    Ok(match outcome {
        consent::Outcome::Granted(_) => Some(true),
        consent::Outcome::Denied(_) => Some(false),
        consent::Outcome::Queued(_) => None,
    })
}
