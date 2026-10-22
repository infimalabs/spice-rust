//! Operator-routed MSG publication with its `room::conversed` record.

use super::*;

/// Publish one operator-routed MSG with its conversation record in one write.
///
/// Each admitted lane route is one native peer directive, as for a plain MSG;
/// it is delivered now when seated or queued when its proved lane is vacant.
/// The conversation record names those directives, and the journal opens the
/// operator item it owes. The packet reaches its operator and every admitted
/// lane route even when another lane route can neither deliver nor queue.
pub(super) fn publish_conversation(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    packet: &Packet<'_>,
    view: &bus::TaskSnapshot,
) -> Result<Published, Unpublished> {
    let residency = current(view, epoch)?;
    let sender = proved_sender(view, packet)?;
    let hunk = packet_hunk(bus, packet)?;
    if conversed(bus, packet.lane, sender, &hunk)? {
        return Ok(Published::Already);
    }
    let directed = occupant_routes(&packet.routes);
    refuse_self(&directed, sender)?;
    let contexts = packet
        .routes
        .iter()
        .filter_map(|route| route.context.as_ref())
        .collect();
    let rooms = packet_rooms(contexts, view)?;
    let identities = identities(packet, &directed);
    let (targets, unreached) =
        resolve_targets(bus, &residency.body().lanes, view, &directed, &identities)?;
    let mut minted_micros = minted(bus)?;
    let at = current_instant().map_err(Unpublished::Failed)?;
    let expires = expiry(at, config.expiry().get()).map_err(Unpublished::Failed)?;
    let keys = mint_keys(&mut minted_micros, targets.len())?;
    let routes = conversation_routes(bus, packet, &targets, &identities)?;
    let record = RoomRecord {
        hunk: (!rooms.is_empty()).then(|| hunk.clone()),
        rooms,
        author: packet.author,
    };
    let moment = moment(minted_micros);
    let mut proposal = lane_write(epoch, packet, at)?;
    let sending = Sending {
        epoch,
        residency: residency.position(),
        packet,
        at,
        expires,
    };
    sending.stage(&mut proposal, &targets, keys)?;
    stage_rooms(
        &mut proposal,
        residency.position(),
        &record,
        &targets,
        moment,
    )?;
    proposal
        .fact(
            RoomConversed {
                publication: conversation_stamp(moment, record.rooms.len())?,
                hunk,
                operation: RoomPublicationKind::Msg {},
                routes,
                directed: targets.iter().map(|target| target.directive).collect(),
                expires,
            },
            FactCause::Existing(residency.position()),
        )
        .map_err(fact_invalid)?;
    admit_send_envelope(&proposal).map_err(unpublished)?;
    commit(bus, proposal).map_err(|outcome| unpublished(typed_outcome(outcome)))?;
    let held_room = names_held_task(&record.rooms, view, packet.lane);
    Ok(Published::Sent {
        lanes: targets
            .iter()
            .filter(|target| target.occupant.is_some())
            .map(|target| target.lane)
            .collect(),
        queued: targets
            .iter()
            .filter(|target| target.occupant.is_none())
            .map(|target| target.recipient)
            .collect(),
        redirected: targets
            .iter()
            .filter_map(|target| target.redirected.as_ref().map(Redirected::note))
            .collect(),
        unreached,
        rooms: record.rooms.into_iter().map(|(handle, _)| handle).collect(),
        operator: true,
        held_room,
    })
}

pub(super) fn conversation_stamp(
    moment: i64,
    offset: usize,
) -> Result<RoomAuthorityStamp, Unpublished> {
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    RoomAuthorityStamp::for_micros(moment.saturating_add(offset)).map_err(|error| {
        Unpublished::Failed(typed_store_fault(
            "msg::room-stamp-invalid",
            &error.to_string(),
        ))
    })
}

/// The routes a conversation record freezes: the operator direction, each
/// admitted lane route with its own context, and each room-only route, in
/// header order. A lane route is admitted when it delivers to a seat or queues
/// for a vacant lane; an operator route names a direction the operator sent
/// this lane.
fn conversation_routes(
    bus: &Bus,
    packet: &Packet<'_>,
    targets: &[Target],
    identities: &[CanonicalUuid],
) -> Result<Vec<RoomRoute>, Unpublished> {
    let mut directed = identities.iter();
    let mut routes = Vec::new();
    for route in &packet.routes {
        let context = context_room(route.context.as_ref());
        let destination = match route.audience {
            Some(Audience::Operator) => RoomDestination::Operator {},
            Some(Audience::Occupant(_)) => {
                let directive = directed.next().copied();
                match targets
                    .iter()
                    .find(|target| Some(target.directive) == directive)
                {
                    Some(target) => RoomDestination::Lane { lane: target.lane },
                    None => continue,
                }
            }
            None => {
                let task = context.ok_or_else(|| {
                    Unpublished::Failed(typed_protocol_fault(
                        "msg::route-empty",
                        "a room-only route names no task",
                    ))
                })?;
                routes.push(RoomRoute {
                    destination: RoomDestination::Task { task },
                    context: None,
                    direction: None,
                });
                continue;
            }
        };
        let direction = route
            .direction
            .as_ref()
            .map(|key| operator_direction(bus, packet.lane, key))
            .transpose()?;
        routes.push(RoomRoute {
            destination,
            context,
            direction,
        });
    }
    Ok(routes)
}

/// `key` when it names a direction the operator sent `lane`.
fn operator_direction(
    bus: &Bus,
    lane: CanonicalUuid,
    key: &steer::Key,
) -> Result<String, Unpublished> {
    let key = key.to_string();
    let read_key = key.clone();
    let head = bus
        .read(move |read| read.steering_key(&read_key))
        .map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "steer::room-unreadable",
                &error.to_string(),
            ))
        })?;
    let refuse = |reason: &str, detail: String| {
        Err(Unpublished::Refused {
            reason: reason.to_owned(),
            detail,
        })
    };
    match head.map(|head| head.into_body()) {
        None => refuse(
            "msg::direction-unknown",
            format!("#{key} names no direction"),
        ),
        Some(head) if head.lane != lane => refuse(
            "msg::direction-other-lane",
            format!("#{key} addresses another lane"),
        ),
        Some(head) if head.from.is_some() || head.fanout.is_none() => refuse(
            "msg::direction-not-operator",
            format!("#{key} is not a direction from the operator"),
        ),
        Some(_) => Ok(key),
    }
}

/// Whether this lane already admitted a conversation record of these exact
/// bytes since the author's birth.
fn conversed(
    bus: &Bus,
    lane: CanonicalUuid,
    author: OccupantAddress,
    hunk: &TranscriptHunk,
) -> Result<bool, Unpublished> {
    let hunk = hunk.clone();
    bus.read(move |read| {
        let mut after = Some(author.birth());
        loop {
            let page = read.selected_fact_page(
                &[wire::FactKind::RoomConversed],
                after,
                None,
                journal::FactReadBudget::RETAINED,
            )?;
            if page.rows.iter().any(|envelope| {
                envelope
                    .fact
                    .read::<RoomConversed>()
                    .is_some_and(|body| body.hunk.head.lane == lane && body.hunk.same_bytes(&hunk))
            }) {
                return Ok(true);
            }
            match page.continuation {
                Some(continuation) => after = Some(continuation),
                None => return Ok(false),
            }
        }
    })
    .map_err(|error| {
        Unpublished::Failed(typed_store_fault(
            "steer::room-unreadable",
            &error.to_string(),
        ))
    })
}
