//! Disposition of steering an occupant answered in its transcript.

use std::collections::BTreeSet;
use std::io;

use super::transcript::TranscriptEvidence;
use super::*;
use crate::resident::ingest_vendor;

/// Retire every live directive the ended occupant answered in its transcript.
///
/// An agent that ends its turn on an ACK or NAK header may exit before the
/// live sweep hears it, so its answer sits in the transcript alone. The exit
/// reads that transcript under the same recognition rules, journals the
/// disposition with the transcript locus as evidence, and notifies the lane
/// supervisor so its next authenticated inbox read follows the journal the
/// app card already reads. A key the transcript does not answer stays live
/// and still wakes a successor, and nothing here disturbs the exit that was
/// already accounted: every failure is written down and leaves the key
/// pending.
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

/// Recover spoken answers before startup publishes or resends pending steering.
///
/// Accounting may already cover an answer whose live notice was lost during
/// the outage, so the transcript checkpoint is not a retirement checkpoint.
pub(in crate::resident) fn dispose_at_startup(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let residency = current_residency(bus, epoch)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    for row in &residency.body().lanes {
        if std::time::Instant::now() >= deadline {
            let _ = writeln!(
                diagnostic,
                "steering recovery reached its startup time limit; remaining directives stay pending"
            );
            break;
        }
        if ingest_vendor(&row.vendor).is_some() {
            dispose_pending(
                bus,
                paths,
                epoch,
                row.lane,
                &row.vendor,
                &Reading::AtStartup,
                diagnostic,
            );
        }
    }
    Ok(())
}

/// Retire the live directives the occupant's newest prose answered, as that
/// prose lands.
///
/// What a lane says is its answer, and the sweep that presents the prose hears it here,
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
    /// Recent transcript evidence, once the occupant has ended.
    AtExit,
    /// Recovered transcript evidence, independent of accounting progress.
    AtStartup,
    /// The keys the prose that just landed headed.
    OnLanding(&'a BTreeSet<steer::Key>),
}

impl Reading<'_> {
    fn moment(&self) -> &'static str {
        match self {
            Self::AtExit => "at exit",
            Self::AtStartup => "at startup",
            Self::OnLanding(_) => "on landing",
        }
    }

    fn answered(&self, key: &str) -> bool {
        match self {
            Self::AtExit | Self::AtStartup => true,
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
    let mut pending = pending
        .iter()
        .filter(|head| reading.answered(&head.body().key))
        .peekable();
    let Some(first) = pending.peek() else {
        return;
    };
    let evidence = match TranscriptEvidence::read(paths, lane, vendor, &first.body().key) {
        Ok(evidence) => evidence,
        Err(outcome) => {
            let _ = writeln!(
                diagnostic,
                "lane {lane} steering left pending {moment}: {}",
                failure_line(&outcome)
            );
            return;
        }
    };
    let mut retired = false;
    let mut replied = BTreeSet::new();
    for head in pending {
        if evidence.exhausted() {
            let _ = writeln!(
                diagnostic,
                "lane {lane} steering search reached its time limit {moment}; remaining directives stay pending"
            );
            break;
        }
        let key = head.body().key.as_str();
        match dispose_head(bus, epoch, lane, head, &evidence) {
            Ok((disposition, reply)) => {
                retired = true;
                replied.extend(reply);
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
    notify_inbox(paths, lane);
    for sender in replied {
        notify_inbox(paths, sender);
    }
}

/// Journal the disposition the transcript carries for one live directive,
/// with the reply its answer owes a seated peer sender, whom it names.
fn dispose_head(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    head: &journal::JournalHead<wire::SteeringHead>,
    evidence: &TranscriptEvidence,
) -> Result<(Disposition, Option<CanonicalUuid>), Outcome<CommandAnswer>> {
    let token = head.body().key.as_str();
    let parsed = token
        .parse::<steer::Key>()
        .map_err(|error| typed_protocol_fault("steer::key-invalid", &error.to_string()))?;
    let standing = IndexedStanding::new(Some(parsed), Some(head));
    let recognition = evidence.recognize(token, &standing)?;
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
    let reply = reply::append_peer_reply(
        bus,
        &mut proposal,
        epoch,
        head,
        pending,
        disposition,
        recognition.answer.as_deref(),
    )?;
    commit(bus, proposal).map_err(typed_outcome)?;
    Ok((disposition, reply))
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
