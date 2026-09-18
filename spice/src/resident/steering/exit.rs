//! Disposition of steering an occupant answered in its transcript.

use std::collections::BTreeSet;
use std::io;

use super::*;

/// Retire every live directive the ended occupant answered in its transcript.
///
/// An agent that ends its turn on an ACK or NACK header exits before anything
/// can run the ack command, so its answer sits in the transcript alone until
/// a successor wakes to cite it. The exit reads that transcript under the
/// rules the ack command applies, journals the disposition the successor
/// would have journaled with the transcript locus as evidence, and refolds
/// the lane inbox so the delivered rendering follows the journal the Canvas
/// card already reads. A key the transcript does not answer stays live and
/// still wakes a successor, and nothing here disturbs the exit that was
/// already accounted: every failure is written down and leaves the key
/// pending for the command path.
pub(in crate::resident) fn dispose_at_exit(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    diagnostic: &mut dyn io::Write,
) {
    dispose_pending(
        bus,
        paths,
        epoch,
        lane,
        vendor,
        &Reading::AtExit,
        diagnostic,
    );
}

/// Retire the live directives the occupant's newest prose answered, as that
/// prose lands.
///
/// A lane that hears steering by interjection runs no ack command: what it
/// says is its answer, and the sweep that presents the prose hears it here,
/// so a resend ladder never climbs past an answer already given. Only the
/// keys the prose headed are read, so a record that answers nothing costs no
/// scan of the tape.
pub(in crate::resident) fn dispose_heard(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    keys: &BTreeSet<steer::Key>,
    diagnostic: &mut dyn io::Write,
) {
    let reading = Reading::OnLanding(keys);
    dispose_pending(bus, paths, epoch, lane, vendor, &reading, diagnostic);
}

/// When a transcript is read for its answers, and which keys it is read for.
enum Reading<'a> {
    /// The whole transcript, once the occupant has ended.
    AtExit,
    /// The keys the prose that just landed headed.
    OnLanding(&'a BTreeSet<steer::Key>),
}

impl Reading<'_> {
    fn moment(&self) -> &'static str {
        match self {
            Self::AtExit => "at exit",
            Self::OnLanding(_) => "on landing",
        }
    }

    fn answered(&self, key: &str) -> bool {
        match self {
            Self::AtExit => true,
            Self::OnLanding(keys) => keys.iter().any(|heard| heard.as_str() == key),
        }
    }
}

fn dispose_pending(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    reading: &Reading<'_>,
    diagnostic: &mut dyn io::Write,
) {
    let moment = reading.moment();
    let pending = match bus.read(move |read| crate::steering_read::pending(read, Some(lane))) {
        Ok(pending) => pending,
        Err(error) => {
            let _ = writeln!(
                diagnostic,
                "lane {lane} steering left pending {moment}: steer::index-unreadable: {error}"
            );
            return;
        }
    };
    let mut retired = false;
    for head in pending
        .iter()
        .filter(|head| reading.answered(&head.body().key))
    {
        let key = head.body().key.as_str();
        match dispose_head(bus, paths, epoch, lane, vendor, head) {
            Ok(disposition) => {
                retired = true;
                let _ = writeln!(
                    diagnostic,
                    "lane {lane} steering {key} {} {moment} from its transcript",
                    format!("{disposition:?}").to_lowercase()
                );
            }
            Err(outcome) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {lane} steering {key} left pending {moment}: {}",
                    failure_line(&outcome)
                );
            }
        }
    }
    if !retired {
        return;
    }
    let refolded = current_residency(bus, epoch)
        .and_then(|residency| publish_inbox(bus, paths, residency.body(), lane));
    if let Err(outcome) = refolded {
        let _ = writeln!(
            diagnostic,
            "lane {lane} inbox not refolded {moment}: {}",
            failure_line(&outcome)
        );
    }
}

/// Journal the disposition the transcript carries for one live directive.
fn dispose_head(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    vendor: &wire::Name,
    head: &journal::JournalHead<wire::SteeringHead>,
) -> Result<Disposition, Outcome<CommandAnswer>> {
    let token = head.body().key.as_str();
    let parsed = token
        .parse::<steer::Key>()
        .map_err(|error| typed_protocol_fault("steer::key-invalid", &error.to_string()))?;
    let standing = IndexedStanding::new(Some(parsed), Some(head));
    let recognition = transcript_recognition(paths, lane, vendor, token, None, &standing)?;
    let (key, disposition) = match recognition.outcome {
        ScanOutcome::Retired { key, disposition } => (key, disposition),
        ScanOutcome::Missed { token, miss } => {
            return Err(refusal(
                "steer::ack-missed",
                Some("one live directive recognition".to_owned()),
                Some(format!("{token}:{}", miss.as_str())),
                "the latest transcript mention of the key is not a placed header",
            ));
        }
    };
    let at = current_instant()?;
    let mut proposal = JournalWrite::new(
        Actor::Lane(lane),
        head.body().origin.clone(),
        at,
        Producer::AckScanner,
    );
    proposal.guard(live_guard(head.body().directive, head.position()));
    let (pending, next) = transitions::append_disposition(
        &mut proposal,
        head,
        &key,
        disposition,
        recognition.locus,
        epoch,
        at,
    )?;
    proposal.steering_head(pending, move |_| next);
    commit(bus, proposal).map_err(typed_outcome)?;
    Ok(disposition)
}

fn failure_line<B>(outcome: &Outcome<B>) -> String {
    match outcome {
        Outcome::Refusal(refusal) => format!(
            "{}: {}",
            refusal.reason(),
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .unwrap_or("refused")
        ),
        Outcome::Fault(fault) => format!("{}: {}", fault.reason, fault.detail),
        Outcome::Reply(_) => unreachable!("a steering failure is never a reply"),
    }
}
