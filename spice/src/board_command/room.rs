use super::*;

use spice::rooms::{
    Declared, LaneAvailability, RoomProjectionBudget, RoomProjectionError, RoomProjectionSources,
    RoomWork, derive_room_attention,
};
use wire::rooms::RoomAuthorityStamp;

const ROOM_ACTIVITY_SECONDS: i64 = 24 * 60 * 60;
const ROOM_PUBLICATION_PAGES: usize =
    spice::rooms::MAX_ROOM_ATTENTION_SIGNALS.div_ceil(journal::MAX_RETAINED_FACT_PAGE_ROWS);

pub(super) fn room_post(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    post: &RoomPost,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if post.text.is_empty() {
        return refusal(
            "room::post-empty",
            Some(post.room.to_string()),
            "supply the complete operator-authored room message",
        );
    }
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&post.room),
        projects,
        allocation,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(anchor) = task_entry(&snapshot.entries, &post.room) else {
        return refusal(
            "room::anchor-unknown",
            Some(post.room.to_string()),
            "read the board and name one current task handle",
        );
    };
    let room = anchor.born().clone();
    let publications = match room_publications(bus, &room) {
        Ok(publications) => publications,
        Err(outcome) => return outcome,
    };
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let attention = match project_attention(
        &snapshot.entries,
        &snapshot.answer.residency.body,
        &room,
        &publications,
        at,
    ) {
        Ok(attention) => attention,
        Err(outcome) => return outcome,
    };
    let publication = match mint_publication(bus, &room, now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let answer = RoomPostAnswer {
        room: anchor.handle().clone(),
        publication: publication.clone(),
        outcome: post_outcome(attention),
    };
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        RoomPublished {
            room,
            publication,
            content: RoomPublicationContent::Authored {
                text: post.text.clone(),
            },
        },
        FactCause::Root,
    ) {
        return store_fault("room::post-build", &error.to_string());
    }
    if let Err(error) = write.command_result(
        request(command, post.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("room::post-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomPost(answer))
}

pub(crate) fn room_publications(
    bus: &Bus,
    room: &wire::TaskStamp,
) -> Result<Vec<wire::Envelope<wire::AnyFact>>, Outcome<CommandAnswer>> {
    let room = room.clone();
    let (rows, more) = bus
        .read(move |read| {
            let mut rows = Vec::new();
            let mut after = None;
            for _ in 0..ROOM_PUBLICATION_PAGES {
                let page =
                    read.room_publications(&room, after, journal::FactReadBudget::RETAINED)?;
                rows.extend(page.rows);
                let Some(continuation) = page.continuation else {
                    return Ok((rows, false));
                };
                after = Some(continuation);
            }
            Ok((rows, true))
        })
        .map_err(|error| store_fault("room::publications-unreadable", &error.to_string()))?;
    if more {
        return Err(refusal(
            "room::attention-budget",
            Some(format!(
                "room publications exceed {} rows or {} bounded pages",
                spice::rooms::MAX_ROOM_ATTENTION_SIGNALS,
                ROOM_PUBLICATION_PAGES
            )),
            "reduce retained room traffic before publishing another message",
        ));
    }
    Ok(rows)
}

pub(crate) fn project_attention(
    entries: &[Entry],
    residency: &wire::ResidencyHead,
    room: &wire::TaskStamp,
    publications: &[wire::Envelope<wire::AnyFact>],
    at: Instant,
) -> Result<spice::rooms::RoomAttention, Outcome<CommandAnswer>> {
    let work = entries
        .iter()
        .filter_map(|entry| RoomWork::from_task(room, entry))
        .collect::<Vec<_>>();
    let since = at
        .get()
        .checked_sub(jiff::SignedDuration::from_secs(ROOM_ACTIVITY_SECONDS))
        .map_err(|error| environment_fault("room::activity-window-invalid", &error.to_string()))?;
    let since = Instant::new(since)
        .map_err(|error| environment_fault("room::activity-window-invalid", &error.to_string()))?;
    derive_room_attention(
        room,
        since,
        &RoomProjectionSources {
            publications: Declared::Available(publications),
            work: Declared::Available(&work),
            duties: Declared::Available(&[]),
            responsibilities: Declared::Available(&[]),
            residency: Declared::Available(residency),
        },
        RoomProjectionBudget::STANDARD,
    )
    .map_err(projection_refusal)
}

fn projection_refusal(error: RoomProjectionError) -> Outcome<CommandAnswer> {
    refusal(
        "room::attention-unavailable",
        Some(error.to_string()),
        "repair the named room authority source, then retry the post",
    )
}

/// The first unused publication identity in `room` at or after `micros`.
///
/// Each occupied candidate is a distinct earlier publication, so the probe
/// ends within the room's size; the clock range is the only other limit.
pub(super) fn mint_publication(
    bus: &Bus,
    room: &wire::TaskStamp,
    mut micros: i64,
) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    loop {
        let candidate = RoomAuthorityStamp::for_micros(micros).map_err(|error| {
            environment_fault("room::publication-clock-invalid", &error.to_string())
        })?;
        let (probe_room, probe) = (room.clone(), candidate.clone());
        let occupied = bus
            .read(move |read| read.room_publication_seq(&probe_room, &probe))
            .map_err(|error| store_fault("room::publication-unreadable", &error.to_string()))?;
        if occupied.is_none() {
            return Ok(candidate);
        }
        micros = micros.checked_add(1).ok_or_else(|| {
            environment_fault(
                "room::publication-clock-exhausted",
                "room publication clock exceeded the native stamp range",
            )
        })?;
    }
}

fn post_outcome(attention: spice::rooms::RoomAttention) -> RoomPostOutcome {
    let listeners = attention
        .lanes
        .into_iter()
        .filter(|lane| lane.listens())
        .map(|lane| {
            let (availability, delivery) = match lane.availability {
                LaneAvailability::Occupied => (
                    RoomListenerAvailability::Occupied,
                    RoomListenerDelivery::AtSafeBoundary,
                ),
                LaneAvailability::Vacant => (
                    RoomListenerAvailability::Vacant,
                    RoomListenerDelivery::WaitsForNextStart,
                ),
                LaneAvailability::Unavailable { disposition } => (
                    RoomListenerAvailability::Unavailable { disposition },
                    RoomListenerDelivery::PendingAvailability,
                ),
                LaneAvailability::Unknown => (
                    RoomListenerAvailability::Unknown,
                    RoomListenerDelivery::PendingAvailability,
                ),
            };
            RoomPostListener {
                lane: lane.lane,
                availability,
                delivery,
            }
        })
        .collect::<Vec<_>>();
    if listeners.is_empty() {
        RoomPostOutcome::RecordedNoCurrentListener {
            next: RoomPostNext::StartOrDirect,
        }
    } else {
        RoomPostOutcome::RecordedWithListeners { listeners }
    }
}
