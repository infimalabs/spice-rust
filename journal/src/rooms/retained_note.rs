//! Task notes retained from before notes became room observations: each
//! retained `task::noted` fact that is not a taste-review audit is an authored
//! observation in its task's room.
//!
//! Schema 106 and earlier stored a note as its own fact; schema 107 crossed
//! each into [`wire::body::JournalRetained`] evidence in place. The fold
//! indexes a retained note into its task's room at its own position, under the
//! publication identity its millisecond instant and position mint, and every
//! room read presents it as that authored publication. A taste-review audit was
//! policy-read noise, so it joins no room. Schema 76 first indexed every
//! retained note through this fold once.

use wire::body::RetainedTaskNote;
use wire::rooms::{RoomAuthorityStamp, RoomPublicationContent, RoomPublished};
use wire::{AnyFact, Envelope, Instant, Seq};

use super::{Authority, Refusal, publish, unsound};

/// Prefix every retired policy-read audit note carried.
pub(crate) const AUDIT_PREFIX: &str = "taste-review audit ";

/// The authored publication `note`, retained at `seq` and `at`, is in its
/// task's room; `None` for a taste-review audit. An instant is whole
/// milliseconds, so the position's last three digits fill its microseconds:
/// notes one transaction committed together stay distinct and in order.
fn observation(
    note: RetainedTaskNote,
    seq: Seq,
    at: Instant,
) -> Option<Result<RoomPublished, String>> {
    if note.note.starts_with(AUDIT_PREFIX) {
        return None;
    }
    let micros = at.get().as_microsecond() + (seq.get() % 1_000) as i64;
    let publication = RoomAuthorityStamp::for_micros(micros).map_err(|error| {
        format!(
            "retained note on {} mints no publication: {error}",
            note.task
        )
    });
    Some(publication.map(|publication| RoomPublished {
        room: note.task,
        publication,
        content: RoomPublicationContent::Authored { text: note.note },
    }))
}

/// Index the retained note `fact` is into its task's room, or `None` when it
/// is no note. A note naming a task the authority does not hold has no room.
pub(super) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    at: Instant,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    let note = RetainedTaskNote::read(fact)?;
    Some(index(authority, seq, at, note))
}

fn index(
    authority: &mut impl Authority,
    seq: Seq,
    at: Instant,
    note: RetainedTaskNote,
) -> Result<(), Refusal> {
    if authority.task(&note.task)?.is_none() {
        return Ok(());
    }
    match observation(note, seq, at) {
        None => Ok(()),
        Some(Err(detail)) => unsound(detail),
        Some(Ok(published)) => publish(authority, seq, &published.room, &published.publication),
    }
}

/// `envelope`, indexed in a room, as a room read presents it: a retained note
/// as the authored publication the fold indexed, any other unchanged.
pub(crate) fn presented(envelope: Envelope<AnyFact>) -> Result<Envelope<AnyFact>, String> {
    let Some(note) = RetainedTaskNote::read(&envelope.fact) else {
        return Ok(envelope);
    };
    let published = observation(note, envelope.seq, envelope.at)
        .ok_or_else(|| format!("room index names taste-review audit {}", envelope.seq))??;
    let encoded = wire::encode(&wire::Fact::new(published))
        .map_err(|error| format!("retained note {} does not encode: {error}", envelope.seq))?;
    let fact = wire::decode(&encoded)
        .map_err(|error| format!("retained note {} does not decode: {error}", envelope.seq))?;
    Ok(Envelope { fact, ..envelope })
}
