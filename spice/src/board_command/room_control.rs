//! Operator room item controls and relevance assessment.
//!
//! ACK, NAK, reply, and recovery move exact guarded operator items. Assessment
//! records one verdict over an existing relevance observation. Every command
//! guards each head it read so a concurrent move refuses instead of
//! overwriting.

use super::*;

use wire::CanonicalUuid;
use wire::intent::{RoomControl, RoomControlAnswer, RoomItemMove};
use wire::rooms::{
    ROOM_CONTROL_RETURN_RESERVE_BYTES, ROOM_PUBLICATION_BYTES_LIMIT, ROOM_RELEVANCE_TEXT_LIMIT,
    RoomAnchor, RoomAuthorityStamp, RoomDestination, RoomDisposed, RoomPosted,
    RoomReceiptDisposition, RoomReceiptHead, RoomReceiptRecord, RoomReceiptRecovered,
    RoomReceiptRef, RoomReceiptState, RoomRelevanceAssessed, RoomRelevanceAssessment,
    RoomRelevanceVerdict,
};

/// Guard reason when an item or requester seat moved between this command's
/// read and write.
const MOVED: &str = "room::control-moved";

type Item = (RoomReceiptRecord, journal::JournalHead<RoomReceiptHead>);

/// Apply one operator control, with the lane an answer returned to.
pub(crate) fn room_control(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    config: &steer::Config,
) -> (Outcome<CommandAnswer>, Option<CanonicalUuid>) {
    let applied = match control {
        RoomControl::Dispose {
            disposition,
            keys,
            clause,
        } => {
            dispose(bus, command, control, *disposition, keys, clause).map(|answer| (answer, None))
        }
        RoomControl::Reply { item, text } => reply(bus, command, control, config, item, text)
            .map(|(answer, lane)| (answer, Some(lane))),
        RoomControl::Recover { item } => {
            recover(bus, command, control, config, item).map(|answer| (answer, None))
        }
        RoomControl::Assess {
            observation,
            verdict,
            explanation,
        } => assess(bus, command, control, observation, *verdict, explanation)
            .map(|answer| (answer, None)),
    };
    match applied {
        Ok((outcome, lane)) => (outcome, lane),
        Err(outcome) => (outcome, None),
    }
}

fn dispose(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    disposition: RoomReceiptDisposition,
    keys: &[RoomReceiptRef],
    clause: &str,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    bound_clause(clause)?;
    let fact = RoomDisposed {
        disposition,
        keys: keys.to_vec(),
        clause: clause.to_owned(),
    };
    let control_shape = fact.control(wire::SequenceCode::new(1)).map_err(|error| {
        refusal(
            "room::control-invalid",
            Some(error.to_owned()),
            "name each open operator item key once, as `spice room inbox` lists them",
        )
    })?;
    let at = now()?;
    let mut write = JournalWrite::new(
        command.actor().clone(),
        resident_origin(command)?,
        at,
        Producer::CommandDoor,
    );
    let mut answer = RoomControlAnswer {
        items: Vec::new(),
        publication: None,
        returned: None,
        assessed: None,
    };
    for key in control_shape.keys() {
        let (_, head) = item(bus, key)?;
        guard_item(&mut write, key, head.position());
        let state = head.body().state();
        if !matches!(state, RoomReceiptState::Pending { .. }) {
            return Err(item_refusal(key, state));
        }
        answer.items.push(RoomItemMove {
            item: key.clone(),
            state: control_shape.receipt_state().name().to_owned(),
        });
    }
    write
        .fact(fact, FactCause::Root)
        .map_err(|error| store_fault("room::control-build", &error.to_string()))?;
    Ok(commit(bus, command, control, write, answer))
}

/// Reply to one pending operator item a lane's MSG opened.
///
/// The reply is operator direction to the authoring lane, recorded as an
/// operator MSG routed to that lane in the item's room, and the same write
/// settles the item acked.
fn reply(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    config: &steer::Config,
    key: &RoomReceiptRef,
    text: &str,
) -> Result<(Outcome<CommandAnswer>, CanonicalUuid), Outcome<CommandAnswer>> {
    let (head, lane, cited, context) = repliable(bus, key, text)?;
    let at = now()?;
    let origin = match &context {
        Some(room) => task_origin(room)?,
        None => resident_origin(command)?,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard_item(&mut write, key, head.position());
    guard_residence(&mut write, lane);
    let returned = format!("RE {cited}: {text}");
    let (returned_key, _) = crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        &mut write,
        crate::resident::OperatorDirective {
            directive: command.id(),
            fanout: None,
            lane,
            text: &returned,
            control: None,
            room: context.clone(),
        },
        at,
        FactCause::Root,
    )?;
    let minted = returned_key
        .parse::<steer::Key>()
        .ok()
        .and_then(|key| key.minted_micros())
        .ok_or_else(|| store_fault("room::return-key-invalid", &returned_key))?;
    let micros = jiff::Timestamp::now()
        .as_microsecond()
        .max(minted.saturating_add(1));
    let publication = mint_stamp(bus, context.as_ref(), micros)?;
    write
        .fact(
            RoomPosted {
                publication: publication.clone(),
                text: text.to_owned(),
                operation: wire::rooms::RoomPublicationKind::Msg {},
                routes: vec![wire::rooms::RoomRoute {
                    destination: RoomDestination::Lane { lane },
                    context,
                    direction: None,
                }],
                directed: vec![command.id()],
                expires: expiry(at, config)?,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::reply-build", &error.to_string()))?;
    write
        .fact(
            RoomDisposed {
                disposition: RoomReceiptDisposition::Ack,
                keys: vec![key.clone()],
                clause: format!("replied {publication}"),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::control-build", &error.to_string()))?;
    crate::resident::steering_envelope_admits(&write)?;
    let answer = RoomControlAnswer {
        items: vec![RoomItemMove {
            item: key.clone(),
            state: "acked".to_owned(),
        }],
        publication: Some(publication),
        returned: Some(returned_key),
        assessed: None,
    };
    Ok((commit(bus, command, control, write, answer), lane))
}

/// A repliable item's receipt head, the resident lane the reply returns to,
/// how the reply cites what it answers, and the source's one room.
type Repliable = (
    journal::JournalHead<RoomReceiptHead>,
    CanonicalUuid,
    String,
    Option<RoomAnchor>,
);

/// The pending lane-opened item `text` may reply to now, with the resident
/// lane the reply returns to, how the reply cites what it answers, and the
/// source publication's one unambiguous room, when it names one.
fn repliable(
    bus: &Bus,
    key: &RoomReceiptRef,
    text: &str,
) -> Result<Repliable, Outcome<CommandAnswer>> {
    require_text(text, "reply")?;
    let (record, head) = item(bus, key)?;
    let state = head.body().state();
    if !matches!(state, RoomReceiptState::Pending { .. }) {
        return Err(item_refusal(key, state));
    }
    let probe = record.publication().clone();
    let (_, opened) = bus
        .read(move |read| read.room_conversation(&probe))
        .map_err(|error| store_fault("room::publication-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            store_fault(
                "room::item-publication-missing",
                &format!("operator item {key} names no admitted conversation"),
            )
        })?;
    let RoomDestination::Lane { lane } = *opened.author() else {
        return Err(store_fault(
            "room::item-author-invalid",
            &format!("operator item {key} was not opened by a lane"),
        ));
    };
    let context = reply_room(&record, &opened);
    let cited = opened
        .routes()
        .iter()
        .find(|route| route.destination == RoomDestination::Operator {})
        .and_then(|route| route.direction.as_ref())
        .map(|direction| format!("#{direction}"))
        .ok_or_else(|| {
            store_fault(
                "room::item-direction-missing",
                &format!("operator item {key} names no operator direction"),
            )
        })?;
    if !lane_resides(bus, lane)? {
        return Err(refusal(
            "room::requester-gone",
            Some(lane.to_string()),
            &format!(
                "the lane left residency; settle the item with `spice room ack {key} --clause TEXT`"
            ),
        ));
    }
    Ok((head, lane, cited, context))
}

/// The item context, or the one distinct room an MSG explicitly speaks in. A
/// multi-room packet has no single room for one operator post to enter.
fn reply_room(
    record: &RoomReceiptRecord,
    publication: &wire::rooms::RoomPublication,
) -> Option<RoomAnchor> {
    if let Some(room) = record.context() {
        return Some(room.clone());
    }
    let mut room = None;
    for route in publication.routes() {
        let candidate = match &route.destination {
            RoomDestination::Task { task } => Some(task),
            _ => route.context.as_ref(),
        };
        let Some(candidate) = candidate else {
            continue;
        };
        match &room {
            Some(known) if known != candidate => return None,
            Some(_) => {}
            None => room = Some(candidate.clone()),
        }
    }
    room
}

fn recover(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    config: &steer::Config,
    key: &RoomReceiptRef,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    let (_, head) = item(bus, key)?;
    let state = head.body().state();
    if !matches!(state, RoomReceiptState::Expired { .. }) {
        return Err(item_refusal(key, state));
    }
    let at = now()?;
    let mut write = JournalWrite::new(
        command.actor().clone(),
        resident_origin(command)?,
        at,
        Producer::CommandDoor,
    );
    guard_item(&mut write, key, head.position());
    write
        .fact(
            RoomReceiptRecovered {
                receipt: key.clone(),
                expires: expiry(at, config)?,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::recover-build", &error.to_string()))?;
    let answer = RoomControlAnswer {
        items: vec![RoomItemMove {
            item: key.clone(),
            state: "pending".to_owned(),
        }],
        publication: None,
        returned: None,
        assessed: None,
    };
    Ok(commit(bus, command, control, write, answer))
}

/// Record the operator's one assessment of a shadow relevance observation.
/// The assessment measures shadow quality and moves no other authority.
fn assess(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    observation: &RoomAuthorityStamp,
    verdict: RoomRelevanceVerdict,
    explanation: &str,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    if explanation.trim().is_empty() || explanation.len() > ROOM_RELEVANCE_TEXT_LIMIT {
        return Err(refusal(
            "room::assessment-invalid",
            Some(explanation.len().to_string()),
            &format!(
                "explain the verdict in 1..={ROOM_RELEVANCE_TEXT_LIMIT} bytes of non-blank text"
            ),
        ));
    }
    let probe = observation.clone();
    let record = bus
        .read(move |read| read.room_relevance_record(&probe))
        .map_err(|error| store_fault("room::relevance-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            refusal(
                "room::observation-unknown",
                Some(observation.to_string()),
                "name an observation as `spice room relevance HANDLE` lists it",
            )
        })?;
    if let Some(assessed) = &record.assessment {
        return Err(refusal(
            "room::observation-assessed",
            Some(observation.to_string()),
            &format!(
                "observation {observation} was already assessed {} at {}; each observation \
                 takes one assessment",
                assessed.body().verdict.name(),
                assessed.position()
            ),
        ));
    }
    let mut write = JournalWrite::new(
        command.actor().clone(),
        resident_origin(command)?,
        now()?,
        Producer::CommandDoor,
    );
    let probe = observation.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.room_relevance_record(&probe)? {
            Some(record) if record.assessment.is_none() => Ok(()),
            _ => Err(GuardError::refused(MOVED)),
        }
    }));
    write
        .fact(
            RoomRelevanceAssessed {
                assessment: RoomRelevanceAssessment {
                    observation: observation.clone(),
                    verdict,
                    explanation: explanation.to_owned(),
                },
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::assessment-build", &error.to_string()))?;
    let answer = RoomControlAnswer {
        items: Vec::new(),
        publication: None,
        returned: None,
        assessed: Some((observation.clone(), verdict)),
    };
    Ok(commit(bus, command, control, write, answer))
}

/// A decision clause is optional, since it reaches no lane, but it must fit
/// the control capacity its item reserved at admission, so a saturated scope
/// still admits it.
fn bound_clause(clause: &str) -> Result<(), Outcome<CommandAnswer>> {
    if clause.len() > ROOM_CONTROL_RETURN_RESERVE_BYTES {
        return Err(refusal(
            "room::clause-too-large",
            Some(clause.len().to_string()),
            &format!(
                "shorten --clause to at most {ROOM_CONTROL_RETURN_RESERVE_BYTES} bytes, the \
                 capacity reserved for this control"
            ),
        ));
    }
    Ok(())
}

/// Operator text must say something and fit one room publication.
fn require_text(text: &str, noun: &str) -> Result<(), Outcome<CommandAnswer>> {
    if text.trim().is_empty() {
        return Err(refusal(
            &format!("room::{noun}-empty"),
            None,
            &format!("pass the {noun} text as the command's TEXT operand"),
        ));
    }
    if text.len() > ROOM_PUBLICATION_BYTES_LIMIT {
        return Err(refusal(
            &format!("room::{noun}-too-large"),
            Some(text.len().to_string()),
            &format!("shorten the {noun} to at most {ROOM_PUBLICATION_BYTES_LIMIT} bytes"),
        ));
    }
    Ok(())
}

fn item_refusal(key: &RoomReceiptRef, state: &RoomReceiptState) -> Outcome<CommandAnswer> {
    let observed = Some(state.name().to_owned());
    match state {
        RoomReceiptState::Expired { .. } => refusal(
            "room::item-lapsed",
            observed,
            &format!("return it to pending first with `spice room recover {key}`"),
        ),
        RoomReceiptState::Pending { .. } => refusal(
            "room::item-live",
            observed,
            &format!("it is pending; settle it with `spice room ack {key} --clause TEXT` or `nak`"),
        ),
        _ => refusal(
            "room::item-settled",
            observed,
            "nothing remains to settle; read `spice room inbox` for open items",
        ),
    }
}

fn item(bus: &Bus, key: &RoomReceiptRef) -> Result<Item, Outcome<CommandAnswer>> {
    let probe = key.clone();
    bus.read(move |read| read.room_item(&probe))
        .map_err(|error| store_fault("room::item-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            refusal(
                "room::item-unknown",
                Some(key.to_string()),
                "name an open operator item key as `spice room inbox` lists it",
            )
        })
}

fn lane_resides(bus: &Bus, lane: CanonicalUuid) -> Result<bool, Outcome<CommandAnswer>> {
    bus.read(move |read| {
        Ok(read
            .residency_head()?
            .is_some_and(|head| head.body().lanes.iter().any(|seat| seat.lane == lane)))
    })
    .map_err(|error| store_fault("room::residency-unreadable", &error.to_string()))
}

fn guard_item(write: &mut JournalWrite, key: &RoomReceiptRef, position: Seq) {
    let key = key.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.room_item(&key)? {
            Some((_, head)) if head.position() == position => Ok(()),
            _ => Err(GuardError::refused(MOVED)),
        }
    }));
}

/// The answer's return addresses a lane that must still reside when it lands.
fn guard_residence(write: &mut JournalWrite, lane: CanonicalUuid) {
    write.guard(JournalGuard::new(move |read| {
        if read
            .residency_head()?
            .is_some_and(|head| head.body().lanes.iter().any(|seat| seat.lane == lane))
        {
            Ok(())
        } else {
            Err(GuardError::refused(MOVED))
        }
    }));
}

/// The first conversation stamp at or after `micros` no admitted
/// conversation or room publication already holds.
pub(super) fn mint_stamp(
    bus: &Bus,
    room: Option<&RoomAnchor>,
    mut micros: i64,
) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    loop {
        let candidate = RoomAuthorityStamp::for_micros(micros).map_err(|error| {
            environment_fault("room::publication-clock-invalid", &error.to_string())
        })?;
        let (probe, room) = (candidate.clone(), room.cloned());
        let occupied = bus
            .read(move |read| {
                Ok(read.room_conversation(&probe)?.is_some()
                    || match &room {
                        Some(room) => read.room_publication_seq(room, &probe)?.is_some(),
                        None => false,
                    })
            })
            .map_err(|error| store_fault("room::publication-unreadable", &error.to_string()))?;
        if !occupied {
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

fn commit<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    body: &B,
    mut write: JournalWrite,
    answer: RoomControlAnswer,
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.command_result(
        request(command, body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("room::control-receipt", &error.to_string());
    }
    match committed_preserving_guard_refusal(
        bus,
        write,
        CommandAnswer::RoomControl(Box::new(answer)),
        MOVED,
    ) {
        Outcome::Refusal(refused) if refused.reason() == MOVED => refusal(
            MOVED,
            None,
            "an operator item or its requester seat moved after this command read it; read `spice \
             room inbox` and retry",
        ),
        outcome => outcome,
    }
}

fn now() -> Result<Instant, Outcome<CommandAnswer>> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::clock-invalid", &error.to_string()))
}

pub(super) fn expiry(
    at: Instant,
    config: &steer::Config,
) -> Result<Instant, Outcome<CommandAnswer>> {
    let invalid = |error: &dyn std::fmt::Display| {
        environment_fault("room::expiry-invalid", &error.to_string())
    };
    let duration =
        jiff::SignedDuration::try_from(config.expiry().get()).map_err(|error| invalid(&error))?;
    let expires = at
        .get()
        .checked_add(duration)
        .map_err(|error| invalid(&error))?;
    Instant::new(expires).map_err(|error| invalid(&error))
}

fn resident_origin(
    command: &wire::AdmittedCommand,
) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    format!("resident:{}", command.epoch())
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))
}
