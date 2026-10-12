//! Room coordination records bind to the operator directions they name.
//!
//! A `room::directed` record follows the one operator direction it links into
//! the room; a `room::shelved` record follows the cancellations it lists and
//! then the stop direction it sends. Each named direction carries this room's
//! coordination control and sits at exactly that position, so commit, replay,
//! and verify admit the same records. A room keeps only its latest record of
//! each kind as a head, which is all its current coordination state needs.

use rusqlite::{Connection, OptionalExtension};
use wire::body::{SteerCanceled, SteerSent};
use wire::rooms::{CoordinationControl, RoomCoordinationStarted, RoomPutAside};
use wire::{Actor, AnyFact, Seq};

use super::{Authority, Change, Refusal, publish, require_operator, unsound};

/// Each room's latest coordination records, created with the current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE room_directions (
    room TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE room_shelvings (
    room TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

/// One operator direction that drives a room-coordination control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Direction {
    pub(crate) control: CoordinationControl,
    /// Position of the fact that sent it.
    pub(crate) sent: u64,
    /// Position of the fact that canceled it, while it has one.
    pub(crate) canceled: Option<u64>,
}

impl Direction {
    /// The direction a stored steering head and its creating actor describe,
    /// absent unless the operator sent it with a coordination control.
    pub(crate) fn stored(head: &wire::SteeringHead, creator: &str, head_seq: u64) -> Option<Self> {
        if creator != Actor::Operator.to_string() {
            return None;
        }
        Some(Self {
            control: head.control.as_deref()?.parse().ok()?,
            sent: head.created.get(),
            canceled: (head.disposition == wire::Disposition::Canceled).then_some(head_seq),
        })
    }
}

/// Which latest coordination record of a room one head holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Directed,
    Shelved,
}

impl Record {
    pub(crate) const fn table(self) -> &'static str {
        match self {
            Self::Directed => "room_directions",
            Self::Shelved => "room_shelvings",
        }
    }
}

/// Admit one coordination fact, or `None` when `fact` is not one.
pub(super) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(sent) = fact.read::<SteerSent>() {
        return Some(direct(authority, seq, actor, &sent));
    }
    if let Some(canceled) = fact.read::<SteerCanceled>() {
        return Some(cancel(authority, seq, &canceled));
    }
    if let Some(body) = fact.read::<RoomCoordinationStarted>() {
        return Some(directed(authority, seq, actor, &body));
    }
    fact.read::<RoomPutAside>()
        .map(|body| shelved(authority, seq, actor, &body))
}

fn direct(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    sent: &SteerSent,
) -> Result<(), Refusal> {
    let Some(control) = sent.control.as_deref().and_then(|text| text.parse().ok()) else {
        return Ok(());
    };
    require_operator(actor, "a room coordination direction")?;
    authority.set_direction(
        &sent.key,
        Direction {
            control,
            sent: seq.get(),
            canceled: None,
        },
    );
    Ok(())
}

fn cancel(
    authority: &mut impl Authority,
    seq: Seq,
    canceled: &SteerCanceled,
) -> Result<(), Refusal> {
    if let Some(mut direction) = authority.direction(&canceled.key)? {
        direction.canceled = Some(seq.get());
        authority.set_direction(&canceled.key, direction);
    }
    Ok(())
}

fn directed(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &RoomCoordinationStarted,
) -> Result<(), Refusal> {
    require_operator(actor, "room coordination")?;
    let named = authority.direction(&body.key)?;
    if !named.is_some_and(|direction| {
        direction.control == CoordinationControl::Coordinate(body.room.clone())
            && direction.sent.checked_add(1) == Some(seq.get())
    }) {
        return unsound(format!(
            "room coordination in {} does not follow its coordination direction {}",
            body.room, body.key
        ));
    }
    publish(authority, seq, &body.room, &body.publication)?;
    authority.change(Change::Coordination {
        record: Record::Directed,
        room: body.room.to_string(),
        seq: seq.get(),
        body: wire::encode(body)?,
    })
}

fn shelved(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &RoomPutAside,
) -> Result<(), Refusal> {
    require_operator(actor, "putting a room aside")?;
    let cited = body.canceled.len() + usize::from(body.stop.is_some());
    let first = u64::try_from(cited)
        .ok()
        .and_then(|cited| seq.get().checked_sub(cited))
        .filter(|_| cited > 0);
    let Some(first) = first else {
        return unsound(format!("putting {} aside withdraws nothing", body.room));
    };
    let coordinate = CoordinationControl::Coordinate(body.room.clone());
    for (position, key) in (first..).zip(&body.canceled) {
        if !authority.direction(key)?.is_some_and(|direction| {
            direction.control == coordinate && direction.canceled == Some(position)
        }) {
            return unsound(format!(
                "putting {} aside lists {key}, not a coordination direction canceled at {position}",
                body.room
            ));
        }
    }
    if let Some(stop) = &body.stop {
        let control = CoordinationControl::Stop {
            room: body.room.clone(),
            claim: stop.claim,
        };
        if !authority.direction(&stop.key)?.is_some_and(|direction| {
            direction.control == control && direction.sent.checked_add(1) == Some(seq.get())
        }) {
            return unsound(format!(
                "putting {} aside does not follow its stop direction {}",
                body.room, stop.key
            ));
        }
    }
    publish(authority, seq, &body.room, &body.publication)?;
    authority.change(Change::Coordination {
        record: Record::Shelved,
        room: body.room.to_string(),
        seq: seq.get(),
        body: wire::encode(body)?,
    })
}

/// Every coordination record in a write names exactly the directions that
/// write sends and cancels, and a stop applies to the anchor's active claim
/// held by the lane the stop directs. Positions alone cannot tell a write's
/// own facts from the tail of the write before it; the write can.
pub(crate) fn require_writes<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    let mut sent = Vec::new();
    let mut canceled = Vec::new();
    let mut records = Vec::new();
    for fact in facts {
        if let Some(body) = fact.read::<SteerSent>() {
            sent.push(body);
        } else if let Some(body) = fact.read::<SteerCanceled>() {
            canceled.push(body.key);
        } else if let Some(body) = fact.read::<RoomCoordinationStarted>() {
            records.push((body.room, Some(body.key), Vec::new(), None));
        } else if let Some(body) = fact.read::<RoomPutAside>() {
            records.push((body.room, None, body.canceled, body.stop));
        }
    }
    for (room, directed, listed, stop) in records {
        let sends = directed.iter().chain(stop.as_ref().map(|stop| &stop.key));
        if !sends.eq(sent.iter().map(|direction| &direction.key)) || listed != canceled {
            return unsound(format!(
                "room {room} coordination names other directions than its write sends and cancels"
            ));
        }
        if let (Some(stop), Some(direction)) = (stop, sent.first()) {
            require_active_claim(connection, &room, stop.claim.get(), direction.lane)?;
        }
    }
    Ok(())
}

fn require_active_claim(
    connection: &Connection,
    room: &wire::TaskStamp,
    claim: u64,
    lane: wire::CanonicalUuid,
) -> Result<(), Refusal> {
    let body = connection
        .query_row(
            "SELECT body FROM claim_heads WHERE stamp = ?1",
            [room.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let head = body
        .map(|body| wire::decode::<wire::ClaimHead>(&body))
        .transpose()?;
    if head.is_some_and(|head| {
        head.disposition == wire::ClaimDisposition::Active
            && head.claimed.get() == claim
            && head.actor == Actor::Lane(lane)
    }) {
        return Ok(());
    }
    unsound(format!(
        "the stop in {room} names no active claim won at {claim} by the lane it directs"
    ))
}
