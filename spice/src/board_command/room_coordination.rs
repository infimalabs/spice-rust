use super::*;

use journal::JournalHead;
use wire::{
    CanonicalUuid, OccupantAddress,
    rooms::{
        CoordinationControl, RoomCoordinationStarted, RoomPutAside as RoomPutAsideFact,
        RoomStopRequest,
    },
};

pub(crate) fn coordination_lane(bus: &Bus, target: OccupantAddress) -> Option<CanonicalUuid> {
    let snapshot = bus.task_snapshot().ok()?;
    let residency = snapshot.residency()?;
    snapshot
        .facts()
        .seat_addresses(&residency.body().lanes)
        .into_iter()
        .find_map(|(lane, address)| (address.address() == Some(target)).then_some(lane))
}

use super::room::mint_publication;

/// A room's latest coordination records, read beside the board snapshot.
struct Coordination {
    directed: Option<JournalHead<RoomCoordinationStarted>>,
    shelved: Option<JournalHead<RoomPutAsideFact>>,
}

impl Coordination {
    fn read(bus: &Bus, room: &wire::TaskStamp) -> Result<Self, Outcome<CommandAnswer>> {
        let room = room.clone();
        bus.read(move |read| {
            Ok(Self {
                directed: read.room_direction(&room)?,
                shelved: read.room_shelving(&room)?,
            })
        })
        .map_err(|error| store_fault("room::coordination-unreadable", &error.to_string()))
    }

    /// Admit `write` only while the room's coordination records are the ones
    /// this command read, so concurrent coordination commands serialize and
    /// at most one coordination request is live.
    fn guard(&self, room: &wire::TaskStamp, write: &mut JournalWrite) {
        let room = room.clone();
        let directed = self.directed.as_ref().map(JournalHead::position);
        let shelved = self.shelved.as_ref().map(JournalHead::position);
        write.guard(journal::JournalGuard::new(move |read| {
            if read
                .room_direction(&room)?
                .as_ref()
                .map(JournalHead::position)
                == directed
                && read
                    .room_shelving(&room)?
                    .as_ref()
                    .map(JournalHead::position)
                    == shelved
            {
                Ok(())
            } else {
                Err(journal::GuardError::refused("room::coordination-moved"))
            }
        }));
    }
}

const REQUEST_FIELD_BYTES: usize = 2_048;

pub(crate) fn room_coordination_read(
    bus: &Bus,
    request: &wire::intent::RoomCoordinationRead,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    let view = match project(bus, anchor.handle(), &heads, &coordination, false) {
        Ok(view) => view,
        Err(outcome) => return outcome,
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(snapshot.cursor),
        body: CommandAnswer::RoomCoordination(view),
    })
}

pub(crate) fn room_coordinate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    room_coordinate_with(bus, command, request, config, projects, allocation, || {})
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn room_coordinate_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    before_commit: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = admit_request(request) {
        return outcome;
    }
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    if heads.workflow.body.status != "pending" || heads.workflow.body.completion.is_some() {
        return refusal(
            "room::anchor-settled",
            Some(anchor.handle().to_string()),
            "coordinate only an unresolved anchor task",
        );
    }
    let current = match project(bus, anchor.handle(), &heads, &coordination, false) {
        Ok(view) => view,
        Err(outcome) => return outcome,
    };
    if !current.requests.is_empty() {
        return refusal(
            "room::coordination-live",
            Some(current.requests[0].key.clone()),
            "retire or put aside the unanswered request before issuing fresh scope",
        );
    }
    if let Some(stop) = current.stop {
        return refusal(
            "room::stop-pending",
            Some(stop.key),
            "wait for the holder's native completion or free before restarting coordination",
        );
    }
    let lane = match admit_target(&snapshot, &heads, request.target) {
        Ok(lane) => lane,
        Err(outcome) => return outcome,
    };
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let publication = match mint_publication(bus, anchor.born(), now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let room = anchor.born().clone();
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let message = direction(anchor.handle(), &heads, lane, request);
    let control = CoordinationControl::Coordinate(room.clone()).to_string();
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    coordination.guard(&room, &mut write);
    guard_target_claim(&mut write, lane, &room);
    let (key, _) = match crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        &mut write,
        crate::resident::OperatorDirective {
            directive: command.id(),
            fanout: None,
            lane,
            text: &message,
            control: Some(&control),
            room: None,
        },
        at,
        FactCause::Root,
    ) {
        Ok(staged) => staged,
        Err(outcome) => return outcome,
    };
    if let Err(error) = write.fact(
        RoomCoordinationStarted {
            room,
            publication,
            key: key.clone(),
            target: request.target,
        },
        FactCause::Root,
    ) {
        return store_fault("room::coordination-build", &error.to_string());
    }
    if let Err(outcome) = crate::resident::steering_envelope_admits(&write) {
        return outcome;
    }
    let view = wire::intent::RoomCoordinationView {
        room: anchor.handle().clone(),
        requests: vec![wire::intent::RoomCoordinationRequest {
            key,
            target: request.target,
        }],
        stop: None,
        hide: false,
    };
    commit_coordinate(bus, command, request, write, view, before_commit)
}

fn commit_coordinate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    mut write: JournalWrite,
    view: wire::intent::RoomCoordinationView,
    before_commit: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.command_result(
        request_command(command, request.clone()),
        command.epoch(),
        view.clone(),
    ) {
        return store_fault("room::coordination-receipt", &error.to_string());
    }
    before_commit();
    committed_preserving_guard_refusal(
        bus,
        write,
        CommandAnswer::RoomCoordination(view),
        "room::lane-busy",
    )
}

pub(crate) fn room_put_aside(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomPutAside,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    let current = match project(bus, anchor.handle(), &heads, &coordination, false) {
        Ok(view) => view,
        Err(outcome) => return outcome,
    };
    if current.requests.is_empty()
        && (current.stop.is_some() || active_lane_claim(&heads).is_none())
    {
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(snapshot.cursor),
            body: CommandAnswer::RoomCoordination(wire::intent::RoomCoordinationView {
                hide: true,
                ..current
            }),
        });
    }
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let publication = match mint_publication(bus, anchor.born(), now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let room = anchor.born().clone();
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let existing_stop = current.stop.clone();
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    coordination.guard(&room, &mut write);
    let canceled = match cancel_requests(bus, &mut write, at, current.requests) {
        Ok(canceled) => canceled,
        Err(outcome) => return outcome,
    };
    let stop = if existing_stop.is_some() {
        None
    } else {
        match stage_stop_request(
            bus, &snapshot, &anchor, &heads, command, config, origin, &room, &mut write, at,
        ) {
            Ok(stop) => stop,
            Err(outcome) => return outcome,
        }
    };
    if canceled.is_empty() && stop.is_none() {
        return refusal(
            "room::already-quiescent",
            Some(anchor.handle().to_string()),
            "the room has no unanswered coordination or active anchor claim to put aside",
        );
    }
    if let Err(error) = write.fact(
        RoomPutAsideFact {
            room,
            publication,
            canceled,
            stop: stop.clone(),
        },
        FactCause::Root,
    ) {
        return store_fault("room::put-aside-build", &error.to_string());
    }
    if let Err(outcome) = crate::resident::steering_envelope_admits(&write) {
        return outcome;
    }
    let visible_stop = stop
        .clone()
        .map(|stop| wire::intent::RoomCoordinationStop {
            key: stop.key,
            target: stop.target,
            claim: stop.claim,
        })
        .or(existing_stop);
    let view = wire::intent::RoomCoordinationView {
        room: anchor.handle().clone(),
        requests: Vec::new(),
        stop: visible_stop,
        hide: true,
    };
    if let Err(error) = write.command_result(
        request_command(command, request.clone()),
        command.epoch(),
        view.clone(),
    ) {
        return store_fault("room::put-aside-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomCoordination(view))
}

fn cancel_requests(
    bus: &Bus,
    write: &mut JournalWrite,
    at: Instant,
    requests: Vec<wire::intent::RoomCoordinationRequest>,
) -> Result<Vec<String>, Outcome<CommandAnswer>> {
    let mut canceled = Vec::with_capacity(requests.len());
    for request in requests {
        let head = match steering_key(bus, &request.key)? {
            Some(head) if head.body().disposition == wire::Disposition::Live => head,
            _ => continue,
        };
        canceled.push(crate::resident::append_cancellation(write, &head, at)?);
    }
    Ok(canceled)
}

fn stage_stop_request(
    bus: &Bus,
    snapshot: &ValidatedSnapshot,
    anchor: &Entry,
    heads: &wire::intent::BoardTask,
    command: &wire::AdmittedCommand,
    config: &steer::Config,
    origin: wire::Origin,
    room: &wire::TaskStamp,
    write: &mut JournalWrite,
    at: Instant,
) -> Result<Option<RoomStopRequest>, Outcome<CommandAnswer>> {
    let Some((lane, claim)) = active_lane_claim(heads) else {
        return Ok(None);
    };
    let target = target_for_lane(snapshot, lane)?;
    let claim_code = wire::SequenceCode::new(claim.body.claimed.get());
    let control = CoordinationControl::Stop {
        room: room.clone(),
        claim: claim_code,
    }
    .to_string();
    let message = format!(
        "Put aside task room {}. Checkpoint the anchor's current work and run `spice task free {}` at the next safe boundary. Do not release child-plan claims or unrelated steering/ASK duties.",
        anchor.handle(),
        anchor.handle(),
    );
    let (key, _) = crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        write,
        crate::resident::OperatorDirective {
            directive: command.id(),
            fanout: None,
            lane,
            text: &message,
            control: Some(&control),
            room: None,
        },
        at,
        FactCause::Existing(claim.body.claimed),
    )?;
    Ok(Some(RoomStopRequest {
        key,
        target,
        claim: claim_code,
    }))
}

fn context(
    bus: &Bus,
    room: &TaskHandle,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<
    (
        ValidatedSnapshot,
        Entry,
        wire::intent::BoardTask,
        Coordination,
    ),
    Outcome<CommandAnswer>,
> {
    let snapshot = validated_working_snapshot(
        bus,
        Actor::Operator,
        std::slice::from_ref(room),
        projects,
        allocation,
    )?;
    let anchor = task_entry(&snapshot.entries, room)
        .cloned()
        .ok_or_else(|| {
            refusal(
                "room::anchor-unknown",
                Some(room.to_string()),
                "read the board and name one current task handle",
            )
        })?;
    let heads = task_heads(&snapshot.answer, room)?.clone();
    let coordination = Coordination::read(bus, anchor.born())?;
    Ok((snapshot, anchor, heads, coordination))
}

fn admit_request(request: &wire::intent::RoomCoordinate) -> Result<(), Outcome<CommandAnswer>> {
    for (name, value) in [
        ("criterion", &request.criterion),
        ("contribution", &request.contribution),
        ("stop", &request.stop),
    ] {
        if value.trim().is_empty() {
            return Err(refusal(
                "room::coordination-scope-empty",
                Some(name.to_owned()),
                "name the unresolved criterion, bounded contribution, and stop condition",
            ));
        }
        if value.len() > REQUEST_FIELD_BYTES {
            return Err(refusal(
                "room::coordination-scope-too-large",
                Some(format!("{name}={} bytes", value.len())),
                "keep each scoped request field within 2048 UTF-8 bytes",
            ));
        }
    }
    Ok(())
}

fn admit_target(
    snapshot: &ValidatedSnapshot,
    anchor: &wire::intent::BoardTask,
    target: OccupantAddress,
) -> Result<CanonicalUuid, Outcome<CommandAnswer>> {
    let lane = snapshot
        .answer
        .addresses
        .iter()
        .find_map(|(lane, address)| (address.address() == Some(target)).then_some(*lane))
        .ok_or_else(|| {
            refusal(
                "room::address-unavailable",
                Some(target.to_string()),
                "choose the current base-52 address of a seated occupant",
            )
        })?;
    if let Some(claim) = anchor.claim.as_ref()
        && claim.body.disposition == ClaimDisposition::Active
    {
        return match claim.body.actor {
            Actor::Lane(holder) if holder == lane => Ok(lane),
            _ => Err(refusal(
                "room::anchor-held-elsewhere",
                Some(anchor.statement.body.handle.to_string()),
                "resume an active anchor only through its current holder",
            )),
        };
    }
    if snapshot.entries.iter().any(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == Actor::Lane(lane) && claim.disposition == ClaimDisposition::Active
        })
    }) {
        return Err(refusal(
            "room::lane-busy",
            Some(target.to_string()),
            "choose an idle occupant or wait for its current native claim disposition",
        ));
    }
    Ok(lane)
}

/// Recheck the target's one active-claim slot in the same transaction that
/// publishes the coordination direction. Holding this room's anchor is the
/// one admitted active claim; every other claim makes the lane busy.
fn guard_target_claim(write: &mut JournalWrite, lane: CanonicalUuid, room: &wire::TaskStamp) {
    let actor = Actor::Lane(lane);
    let room = room.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.active_claim_for_actor(&actor)? {
            None => Ok(()),
            Some(active) if active.statement().body().born == room => Ok(()),
            Some(_) => Err(GuardError::refused("room::lane-busy")),
        }
    }));
}

fn target_for_lane(
    snapshot: &ValidatedSnapshot,
    lane: CanonicalUuid,
) -> Result<OccupantAddress, Outcome<CommandAnswer>> {
    snapshot
        .answer
        .addresses
        .get(&lane)
        .and_then(wire::SeatAddress::address)
        .ok_or_else(|| {
            refusal(
                "room::holder-address-unavailable",
                None,
                "the current anchor holder has no proved base-52 occupant address",
            )
        })
}

fn direction(
    handle: &TaskHandle,
    heads: &wire::intent::BoardTask,
    lane: CanonicalUuid,
    request: &wire::intent::RoomCoordinate,
) -> String {
    let action = match request.action {
        wire::intent::RoomCoordinateAction::Start => "Start",
        wire::intent::RoomCoordinateAction::Resume => "Resume",
    };
    let capture = if heads.claim.as_ref().is_some_and(|claim| {
        claim.body.disposition == ClaimDisposition::Active && claim.body.actor == Actor::Lane(lane)
    }) {
        "Continue under your current native claim.".to_owned()
    } else {
        format!(
            "Run `spice task capture {handle}` to acquire this anchor through its ordinary native claim path; any refusal leaves this request pending without possession."
        )
    };
    format!(
        "{action} task room {handle}. Before acting, re-read its current task and cited authority; push back or redirect if the ground changed.\nUnresolved criterion: {}\nBounded contribution: {}\nStop condition: {}\n{capture}",
        request.criterion, request.contribution, request.stop,
    )
}

/// The room's coordination state: the latest request while its steering is
/// still live, and the latest stop while the claim win it names stays active.
/// Starting refuses while a request is live and every coordination write
/// guards the records it read, so no earlier request can still be live.
fn project(
    bus: &Bus,
    handle: &TaskHandle,
    heads: &wire::intent::BoardTask,
    coordination: &Coordination,
    hide: bool,
) -> Result<wire::intent::RoomCoordinationView, Outcome<CommandAnswer>> {
    let mut requests = Vec::new();
    if let Some(directed) = &coordination.directed {
        let started = directed.body();
        let Some(head) = steering_key(bus, &started.key)? else {
            return Err(store_fault(
                "room::coordination-steering-missing",
                &format!("room marker names absent steering key {}", started.key),
            ));
        };
        if head.body().disposition == wire::Disposition::Live {
            requests.push(wire::intent::RoomCoordinationRequest {
                key: started.key.clone(),
                target: started.target,
            });
        }
    }
    let active = active_lane_claim(heads);
    let stop = coordination
        .shelved
        .as_ref()
        .and_then(|shelved| shelved.body().stop.clone())
        .filter(|stop| {
            active.is_some_and(|(_, claim)| claim.body.claimed.get() == stop.claim.get())
        })
        .map(|stop| wire::intent::RoomCoordinationStop {
            key: stop.key,
            target: stop.target,
            claim: stop.claim,
        });
    Ok(wire::intent::RoomCoordinationView {
        room: handle.clone(),
        requests,
        stop,
        hide,
    })
}

fn steering_key(
    bus: &Bus,
    key: &str,
) -> Result<Option<journal::JournalHead<wire::SteeringHead>>, Outcome<CommandAnswer>> {
    let key = key.to_owned();
    bus.read(move |read| read.steering_key(&key))
        .map_err(|error| store_fault("room::coordination-unreadable", &error.to_string()))
}

fn active_lane_claim(
    heads: &wire::intent::BoardTask,
) -> Option<(CanonicalUuid, &wire::intent::Positioned<wire::ClaimHead>)> {
    let claim = heads
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)?;
    let Actor::Lane(lane) = claim.body.actor else {
        return None;
    };
    Some((lane, claim))
}

fn request_command<B: serde::Serialize + Clone>(
    command: &wire::AdmittedCommand,
    body: B,
) -> wire::Command<B> {
    request(command, body)
}
