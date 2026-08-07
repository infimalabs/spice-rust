//! Durable taste recognition over the sole admitted lane-capture path.

use std::collections::BTreeSet;

use bus::{Bus, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use steer::Standing as _;
use wire::body::TranscriptLocus;
use wire::{Actor, CanonicalUuid, Digest, Instant, Outcome, Producer, ResidencyHead, Seq};

use super::{
    CommandResult, RuntimePaths, commit, current_residency, environment_fault, new_uuid, steering,
    store_fault, write,
};

const COMMIT_RETRIES: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    taste: String,
    content: Digest,
    locus: TranscriptLocus,
    text: String,
}

#[derive(Clone, Debug)]
struct Opening {
    candidate: Candidate,
    directive: CanonicalUuid,
    key: steer::Key,
}

/// Recognize and durably raise every unsuppressed reminder in one admitted
/// capture record.
///
/// The caller has already proved the record belongs to the current supervised
/// occupant and committed any compaction boundary it carries. An empty bag or
/// a record without eligible prose performs no journal read at all.
pub(super) fn recognize(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    bag: &taste::Bag,
    steering_config: &steer::Config,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<()> {
    recognize_with(
        bus,
        paths,
        epoch,
        bag,
        steering_config,
        record,
        |bus, _, proposal| commit(bus, proposal),
        steering::publish_inbox,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the test seam names both durable commit and disposable publication"
)]
fn recognize_with<C, P>(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    bag: &taste::Bag,
    steering_config: &steer::Config,
    record: &lane::capture::CaptureRecord,
    mut commit_write: C,
    mut publish: P,
) -> CommandResult<()>
where
    C: FnMut(&Bus, &[Opening], JournalWrite) -> CommandResult<JournalCommit>,
    P: FnMut(&Bus, &RuntimePaths, &ResidencyHead, CanonicalUuid) -> CommandResult<()>,
{
    let candidates = candidates(bag, record)?;
    if candidates.is_empty() {
        return Ok(());
    }

    for attempt in 0..COMMIT_RETRIES {
        let observed = current_residency(bus, epoch)?;
        require_current_occupant(observed.body(), record.lane, record.occupant)?;
        let snapshot = bus
            .fact_snapshot()
            .map_err(|error| store_fault("taste::journal-unreadable", &error.to_string()))?;
        let anchor = context_anchor(&snapshot, record.lane, record.occupant)
            .map_err(|detail| store_fault("taste::epoch-missing", &detail))?;
        let mut unsuppressed = Vec::new();
        for candidate in &candidates {
            if !was_reminded(&snapshot, &candidate.content, anchor) {
                unsuppressed.push(candidate.clone());
            }
        }
        if unsuppressed.is_empty() {
            return Ok(());
        }

        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| store_fault("taste::steering-invalid", &error.to_string()))?;
        let at = current_instant()?;
        let expires = expiry(at, steering_config.expiry().get())?;
        let openings = openings(unsuppressed, &ledger, at)?;
        let mut proposal = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
        proposal.guard(reminder_guard(
            epoch,
            record.lane,
            record.occupant,
            anchor,
            openings.clone(),
        ));
        for opening in &openings {
            proposal
                .fact(
                    wire::body::TasteReminded {
                        directive: opening.directive,
                        key: opening.key.to_string(),
                        lane: record.lane,
                        taste: opening.candidate.taste.clone(),
                        content: opening.candidate.content.clone(),
                        occupant: record.occupant,
                        compaction: anchor.get(),
                        locus: opening.candidate.locus,
                        text: opening.candidate.text.clone(),
                        expires,
                    },
                    FactCause::Existing(anchor),
                )
                .map_err(|error| store_fault("taste::reminder-invalid", &error.to_string()))?;
        }

        match commit_write(bus, &openings, proposal) {
            Ok(_) => {
                // The inbox is a disposable replay rendering. Once the fact
                // committed, an unavailable path cannot retract or duplicate
                // the reminder; startup and the next steering transition can
                // render it again from the journal.
                let _rendering = publish(bus, paths, observed.body(), record.lane);
                return Ok(());
            }
            Err(Outcome::Refusal(_)) if attempt + 1 < COMMIT_RETRIES => {
                // A command or duplicate capture may have won between the
                // optimistic read and the guarded commit. Rebuild from the
                // new authority: a duplicate disappears, while a key collision
                // receives a fresh key.
            }
            Err(outcome) => return Err(outcome),
        }
    }
    unreachable!("the final commit attempt returns instead of retrying")
}

fn candidates(
    bag: &taste::Bag,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<Vec<Candidate>> {
    if bag.tastes().is_empty() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let listen::Event::Prose(prose) = decoded else {
            continue;
        };
        // Echo classification precedes bag matching. No trigger can make a
        // resident-authored reminder become a candidate again.
        if taste::is_reminder_echo(&prose.text) {
            continue;
        }
        let event = checked_locus(event)?;
        for hit in bag.hits(decoded) {
            let content = taste::reminder_identity(hit.taste(), hit.context());
            if candidates
                .iter()
                .any(|candidate: &Candidate| candidate.content == content)
            {
                continue;
            }
            candidates.push(Candidate {
                taste: hit.taste().to_string(),
                content,
                locus: TranscriptLocus {
                    member: hit.locus().member,
                    offset: hit.locus().offset,
                    length: hit.locus().length,
                    event,
                    line: checked_locus(hit.line())?,
                    column: checked_locus(hit.column())?,
                },
                text: taste::reminder_text(hit.taste(), hit.message()),
            });
        }
    }
    Ok(candidates)
}

fn checked_locus(value: usize) -> CommandResult<u32> {
    u32::try_from(value)
        .map_err(|error| store_fault("taste::transcript-locus-invalid", &error.to_string()))
}

fn require_current_occupant(
    residency: &ResidencyHead,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<()> {
    if residency
        .lanes
        .iter()
        .any(|row| row.lane == lane && row.occupant == Some(occupant))
    {
        Ok(())
    } else {
        Err(store_fault(
            "taste::occupant-moved",
            "the reminder capture no longer belongs to the current occupant",
        ))
    }
}

fn openings(
    candidates: Vec<Candidate>,
    ledger: &steer::Ledger,
    at: Instant,
) -> CommandResult<Vec<Opening>> {
    let mut millis = at.get().as_millisecond();
    let mut keys = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut opened = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let directive = loop {
            let candidate = new_uuid();
            if ledger.directive(candidate).is_none() && identities.insert(candidate) {
                break candidate;
            }
        };
        let key = loop {
            let candidate = steer::Key::for_millis(millis).map_err(|error| {
                environment_fault("taste::steering-key-invalid", &error.to_string())
            })?;
            millis = millis.checked_add(1).ok_or_else(|| {
                environment_fault(
                    "taste::steering-key-invalid",
                    "epoch millisecond overflowed while advancing past a collision",
                )
            })?;
            if ledger.disposition(&candidate).is_none() && keys.insert(candidate.clone()) {
                break candidate;
            }
        };
        opened.push(Opening {
            candidate,
            directive,
            key,
        });
    }
    Ok(opened)
}

fn current_instant() -> CommandResult<Instant> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))
}

fn expiry(at: Instant, duration: std::time::Duration) -> CommandResult<Instant> {
    let duration = jiff::SignedDuration::try_from(duration)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))?;
    let expires = at
        .get()
        .checked_add(duration)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))?;
    Instant::new(expires)
        .map_err(|error| environment_fault("taste::expiry-invalid", &error.to_string()))
}

fn reminder_guard(
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    openings: Vec<Opening>,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("taste::residency-missing"))?;
        if residency.body().epoch != epoch
            || !residency
                .body()
                .lanes
                .iter()
                .any(|row| row.lane == lane && row.occupant == Some(occupant))
        {
            return Err(GuardError::refused("taste::occupant-moved"));
        }

        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = context_anchor(&snapshot, lane, occupant).map_err(GuardError::Invalid)?;
        if current != anchor {
            return Err(GuardError::refused("taste::epoch-moved"));
        }
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        for opening in &openings {
            if was_reminded(&snapshot, &opening.candidate.content, anchor) {
                return Err(GuardError::refused("taste::reminder-recorded"));
            }
            if ledger.directive(opening.directive).is_some()
                || ledger.disposition(&opening.key).is_some()
            {
                return Err(GuardError::refused("taste::steering-identity-taken"));
            }
        }
        Ok(())
    })
}

fn context_anchor(
    snapshot: &journal::FactSnapshot,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<Seq, String> {
    for envelope in snapshot.facts().iter().rev() {
        let fact = &envelope.fact;
        let matches = if let Some(body) = fact.read::<wire::body::LaneCompacted>() {
            body.lane == lane && body.occupant == occupant
        } else if let Some(body) = fact.read::<wire::body::LaneRenewed>() {
            body.lane == lane && body.occupant == occupant
        } else if let Some(body) = fact.read::<wire::body::LaneStarted>() {
            body.lane == lane && body.occupant == occupant
        } else {
            false
        };
        if matches {
            return Ok(envelope.seq);
        }
    }
    Err(format!(
        "lane {lane} occupant {occupant} has no start, renewal, or compaction fact"
    ))
}

fn was_reminded(snapshot: &journal::FactSnapshot, content: &Digest, anchor: Seq) -> bool {
    snapshot.facts().iter().any(|envelope| {
        envelope
            .fact
            .read::<wire::body::TasteReminded>()
            .is_some_and(|body| &body.content == content && body.compaction == anchor.get())
    })
}

#[cfg(test)]
mod tests;
