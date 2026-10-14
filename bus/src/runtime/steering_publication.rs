//! The room publication that opened one steering directive.

use journal::GuardError;
use journal::crossing::{AnyFact, Envelope, FactKind, Seq, SteeringHead, rooms};

/// Positions after a directive's creating fact within which the room
/// publication its own write staged must appear. A write stages each
/// directive before the one publication naming it, and one publication
/// opens at most a bounded fanout of directives plus its room records.
const PUBLICATION_WINDOW: u64 = 256;

/// The first room publication after a directive's creating fact, kept only
/// when its `directed` list names that directive. Directive identities are
/// unique and the journal admits a publication only beside the directives
/// it names, so the containment is the proof; position merely bounds the
/// indexed lookup.
pub(super) fn opening_publication(
    read: &journal::JournalRead<'_>,
    head: &SteeringHead,
) -> Result<Option<Envelope<AnyFact>>, GuardError> {
    let Some(cursor) = read.cursor()? else {
        return Ok(None);
    };
    if cursor <= head.created {
        return Ok(None);
    }
    let upper = Seq::new(
        head.created
            .get()
            .saturating_add(PUBLICATION_WINDOW)
            .min(cursor.get()),
    )
    .expect("a bounded position after a durable fact is positive");
    let page = read.selected_fact_page(
        &[FactKind::RoomPosted, FactKind::RoomConversed],
        Some(head.created),
        Some(upper),
        journal::FactReadBudget::new(1, journal::MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("one room publication is a repository budget"),
    )?;
    Ok(page.rows.into_iter().next().filter(|envelope| {
        let directed = envelope
            .fact
            .read::<rooms::RoomPosted>()
            .map(|posted| posted.directed)
            .or_else(|| {
                envelope
                    .fact
                    .read::<rooms::RoomConversed>()
                    .map(|conversed| conversed.directed)
            });
        directed.is_some_and(|directed| directed.contains(&head.directive))
    }))
}
