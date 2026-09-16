//! Presentation of every record one live ingest sweep newly accounted.
//!
//! A supervisor notice is a wake-up, not the unit of presentation. The kernel
//! queue behind the notice socket is finite and the supervisor cannot wait on
//! it, so a burst of records during one journal commit loses most of its
//! notices. The sweep the surviving notice triggers accounts every record on
//! the tape regardless, and this module presents exactly that accounted set,
//! so a lost notice costs nothing the Canvas would miss until a restart.

use super::*;

/// Every record the sweep newly accounted for the noticed lane, in tape order,
/// with the noticed record standing in at its own locus.
///
/// The noticed record carries the declaration and decoded events the
/// supervisor produced, so it is preferred over its scanned twin. When the
/// sweep did not cover it, its bytes were accounted by an earlier pass, so it
/// leads the list: the publisher retains by locus and refuses a repeat, which
/// keeps the startup overlap (a notice queued while the catch-up sweep ran)
/// from presenting twice.
pub(super) fn swept_records<'a>(
    noticed: &'a lane::capture::CaptureRecord,
    observations: &mut [super::super::TranscriptObservation],
) -> Vec<Cow<'a, lane::capture::CaptureRecord>> {
    let mut swept = Vec::new();
    let mut noticed_swept = false;
    for observation in observations
        .iter_mut()
        .filter(|observation| observation.batch.lane == noticed.lane)
    {
        for record in std::mem::take(&mut observation.records) {
            if record.locus == noticed.locus {
                swept.push(Cow::Borrowed(noticed));
                noticed_swept = true;
            } else if let Some(record) = scanned_capture_record(noticed, record) {
                swept.push(Cow::Owned(record));
            }
        }
    }
    if !noticed_swept {
        swept.insert(0, Cow::Borrowed(noticed));
    }
    swept
}

/// One scanned record as the capture record its own notice would have carried.
///
/// Residue presents nothing and carries no prose to recognize, so it is left
/// out rather than materialized. Authorship follows the noticed record: the
/// sweep is scoped to its lane and its occupant admitted the notice.
fn scanned_capture_record(
    noticed: &lane::capture::CaptureRecord,
    record: listen::Record,
) -> Option<lane::capture::CaptureRecord> {
    let listen::RecordPayload::Events(events) = record.payload else {
        return None;
    };
    let context = record.context?;
    Some(lane::capture::CaptureRecord {
        lane: noticed.lane,
        occupant: noticed.occupant,
        stream: noticed.stream,
        boundary: sill::RecordBoundary::Complete,
        locus: record.locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context,
            events: events.into_iter().map(|sourced| sourced.event).collect(),
        },
    })
}

/// Recognize one record's taste candidates under the configured adjudication.
///
/// A saturated judge worker is a diagnostic, not a resident fault: the sweep
/// presents many records in one turn and the worker's bounded queue is sized
/// for a trickle. The candidates are forgotten and the next sweep over the
/// same tape range recognizes them again.
pub(super) fn recognize<P>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    recognition: &mut TasteRecognition<'_>,
    record: &lane::capture::CaptureRecord,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    match recognized(bus, epoch, surfaces, recognition, record) {
        Err(Outcome::Fault(fault)) if fault.reason == taste_surface::JUDGE_SATURATED => {
            let _ = writeln!(
                diagnostic,
                "taste recognition deferred at {}: {}: {}",
                record.locus, fault.reason, fault.detail
            );
            Ok(())
        }
        outcome => outcome,
    }
}

fn recognized<P>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    recognition: &mut TasteRecognition<'_>,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<()> {
    match recognition {
        TasteRecognition::Deferred(judge) => judge.recognize(bus, epoch, surfaces.tastes, record),
        TasteRecognition::Immediate(judge) => taste_surface::recognize(
            bus,
            surfaces.paths,
            epoch,
            surfaces.tastes,
            surfaces.steering,
            Some(&mut **judge),
            record,
        ),
        // Disabled adjudication is deliberately direct: it starts no process
        // or worker and publishes the deterministic reminder immediately.
        TasteRecognition::Direct => taste_surface::recognize(
            bus,
            surfaces.paths,
            epoch,
            surfaces.tastes,
            surfaces.steering,
            None,
            record,
        ),
    }
}

/// Present one record, naming an unavailable Canvas instead of raising it.
pub(super) fn present<P: AgentMessagePublisher>(
    surfaces: &CaptureSurfaces<'_, P>,
    record: &lane::capture::CaptureRecord,
    steering: Option<&PresentationDirectives>,
    worktree: Option<&Path>,
    views: &mut CaptureViews<'_>,
) -> CommandResult<()> {
    match publish_agent_messages_in(surfaces.publisher, record, views.state, steering, worktree) {
        Ok(()) => Ok(()),
        Err(Outcome::Fault(fault)) => {
            let _ = writeln!(
                views.diagnostic,
                "canvas message unavailable: {}: {}",
                fault.reason, fault.detail
            );
            Ok(())
        }
        Err(outcome) => Err(outcome),
    }
}
