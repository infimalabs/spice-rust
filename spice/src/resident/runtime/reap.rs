//! Seat sweep for endings whose supervisor notice never arrived.
//!
//! A detached supervisor reports its agent ending with one unbound datagram,
//! sent once and never confirmed. When that datagram is lost the durable row
//! stays occupied: every wake for the lane is refused as already occupied,
//! the Canvas keeps showing a running occupant, and only a resident restart
//! would sweep the supervisor state and record the exit. This sweep reads
//! that same state for every seated lane on each reconciliation, so the
//! running resident accounts the ending itself and the wakes the lane was
//! owed fire from the reconciliation that follows.
//!
//! The closing transcript sweep lives here too: whichever path closes a seat,
//! what its occupant wrote last is presented from the same loop turn.

use super::*;

/// Account one occupant exit against renewal, lifecycle holds, and residency.
///
/// The supervisor notice and the seat sweep both land here, so an exit is
/// accounted the same way whichever of them reports it first.
#[allow(
    clippy::too_many_arguments,
    reason = "one exit crosses renewal, lifecycle, residency, and steering"
)]
pub(in crate::resident) fn account_exit(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    config: &lane::Config,
    exit: lane::capture::CaptureExit,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    if !lane_surface::renewal_requested_exit(bus, epoch, exit.lane, exit.occupant)? {
        lifecycle.observe_death(bus, epoch, seats, exit.lane, exit.occupant, config)?;
    }
    lane_surface::occupant_exited(bus, paths, epoch, seats, exit, diagnostic)
}

/// Account one supervisor failure the way its own notice would have.
#[allow(
    clippy::too_many_arguments,
    reason = "one failure crosses the same accounting surfaces as one exit"
)]
pub(super) fn account_failure(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    config: &lane::Config,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    failure: &str,
) -> CommandResult<()> {
    if !lane_surface::renewal_requested_exit(bus, epoch, lane, occupant)? {
        lifecycle.observe_death(bus, epoch, seats, lane, occupant, config)?;
    }
    if seats
        .occupant(lane)
        .is_some_and(|sitting| sitting.occupant() == occupant)
    {
        let _detached = seats.vacate(lane);
    }
    lane_surface::supervisor_failed(bus, epoch, lane, occupant, failure)
}

/// Account every seated occupant whose supervisor already published an ending.
pub(super) fn reap_ended_seats(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    sweep_seats(
        bus,
        command_surface.epoch(),
        command_surface.paths,
        seats,
        lifecycle,
        &command_surface.config.lane,
        diagnostic,
    )
}

/// Judge every seat against its supervisor state and account the ended ones.
///
/// Each seat is judged from its own durable state, so one unreadable record
/// leaves the other seats accounted and is reported for the next sweep to
/// read again. A refused commit defers that seat the way a late notice is
/// deferred: the state on disk does not change, so the next reconciliation
/// reaches the same judgement.
pub(in crate::resident) fn sweep_seats(
    bus: &Bus,
    epoch: CanonicalUuid,
    paths: &RuntimePaths,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    config: &lane::Config,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for lane in seats.lanes() {
        let Some(occupant) = seats.occupant(lane).map(lane::occupant::Occupant::occupant) else {
            continue;
        };
        let ending = match supervisor::ending::published_ending(paths, lane, occupant) {
            Ok(Some(ending)) => ending,
            Ok(None) => continue,
            Err(outcome) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {lane} occupant {occupant} supervisor state unreadable; seat kept: {outcome:?}"
                );
                continue;
            }
        };
        let _ = writeln!(
            diagnostic,
            "lane {lane} occupant {occupant} ended without a supervisor notice; accounting from its published state"
        );
        let accounted = match ending {
            supervisor::ending::PublishedEnding::Exited => account_exit(
                bus,
                paths,
                epoch,
                seats,
                lifecycle,
                config,
                lane::capture::CaptureExit { lane, occupant },
                diagnostic,
            ),
            supervisor::ending::PublishedEnding::Failed(failure) => account_failure(
                bus, epoch, seats, lifecycle, config, lane, occupant, &failure,
            ),
        };
        super::deferral::defer_store_fault(accounted, diagnostic, "reaped seat accounting")?;
    }
    Ok(())
}

/// Present what every seat vacated this turn left on its tape.
///
/// Every path that closes a seat, a supervisor exit or failure notice, the
/// seat sweep for a lost ending, an operator stop, records the occupant in the
/// seat ledger. The closing sweep runs here, after the turn, where the taste,
/// steering, and Canvas surfaces all exist; a refused commit is deferred the
/// way exit accounting defers it.
#[allow(
    clippy::too_many_arguments,
    reason = "the closing sweep crosses the same live capture surfaces as a noticed record"
)]
pub(in crate::resident) fn present_closed_transcripts(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    mut taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    message_state: &mut message_surface::MessageState,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for (lane, occupant) in seats.take_vacated() {
        let surfaces = CaptureSurfaces::new(
            command_surface.paths,
            &command_surface.config.taste,
            &command_surface.config.steering,
            message_publisher,
        );
        let recognition = taste_judge.as_deref_mut().map_or(
            message_surface::TasteRecognition::Direct,
            message_surface::TasteRecognition::Deferred,
        );
        let presented = message_surface::present_closed_transcript(
            bus,
            epoch,
            &surfaces,
            recognition,
            lane,
            occupant,
            message_surface::CaptureViews::new(message_state, diagnostic),
        );
        super::deferral::defer_store_fault(presented, diagnostic, "closing transcript sweep")?;
    }
    Ok(())
}
