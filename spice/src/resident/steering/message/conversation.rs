//! ASK, ANS, and operator-routed MSG publication with its `room::conversed`
//! record, and the checks an ANS must pass against its stored question.

use super::*;

/// Publish one ASK, ANS, or operator-routed MSG with its conversation record
/// in one write.
///
/// Each admitted lane route is one native peer directive, as for a plain MSG;
/// it is delivered now when seated or queued when its proved lane is vacant.
/// An ASK's directive carries the ASK's freshly minted result handle as its
/// key. The conversation record names those directives, and the journal opens
/// the operator item and the question result it owes. An ANS is checked against
/// its stored question before anything is written: only its named respondent
/// lane answers, while the result is pending and before its deadline, and an
/// asserted context must equal the stored one. Its return reaches the
/// requester lane's seated occupant, or opens an operator item, and the same
/// write acknowledges the respondent's still-live ASK directive. An ASK or ANS
/// whose one lane route can neither deliver nor queue publishes nothing; an MSG
/// still reaches its operator and every admitted lane route.
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
    let lanes = seats(&residency.body().lanes, view);
    let answer = match &packet.request {
        Request::Ans {
            result, context, ..
        } => Some(answerable(
            bus,
            view,
            &lanes,
            packet,
            result,
            context.as_ref(),
        )?),
        Request::Msg { .. } | Request::Ask { .. } => None,
    };
    let (directed, contexts) = addressed(answer.as_ref(), packet);
    refuse_self(&directed, sender)?;
    let rooms = packet_rooms(contexts, view)?;
    let identities = identities(packet, &directed);
    let (targets, unreached) =
        resolve_targets(bus, &residency.body().lanes, view, &directed, &identities)?;
    if !matches!(packet.request, Request::Msg { .. }) && !unreached.is_empty() {
        return Err(Unpublished::Refused {
            reason: UNAVAILABLE.to_owned(),
            detail: unreached.join("; "),
        });
    }
    let mut minted_micros = minted(bus)?;
    let at = current_instant().map_err(Unpublished::Failed)?;
    let expires = expiry(at, config.expiry().get()).map_err(Unpublished::Failed)?;
    let asserted = answer.as_ref().and_then(|answer| answer.asserted.clone());
    let (operation, keys, result) =
        operation(packet, asserted, targets.len(), &mut minted_micros, expires)?;
    let routes = conversation_routes(bus, packet, &targets, &identities)?;
    let opens_item = opens_item(answer.as_ref(), &routes);
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
                operation,
                routes,
                directed: targets.iter().map(|target| target.directive).collect(),
                expires,
            },
            FactCause::Existing(residency.position()),
        )
        .map_err(fact_invalid)?;
    if let Some(answer) = &answer {
        answer.stage(&mut proposal, packet, epoch, at)?;
    }
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
        unreached,
        rooms: record.rooms.into_iter().map(|(handle, _)| handle).collect(),
        conversed: Some(Conversed {
            verb: verb(&packet.request),
            operator: opens_item,
            result,
        }),
        held_room,
    })
}

/// The lane routes a packet directs, with the contexts that name its rooms.
///
/// An ANS directs only its return to a lane requester, in the stored question
/// context; an MSG or ASK directs each occupant route its header spelled.
fn addressed<'p>(
    answer: Option<&'p Answerable>,
    packet: &'p Packet<'_>,
) -> (Vec<Directed>, Vec<&'p TaskHandle>) {
    match answer {
        Some(answer) => (answer.directed.clone(), answer.context.iter().collect()),
        None => {
            let routes = packet.request_routes();
            (
                occupant_routes(routes),
                routes
                    .iter()
                    .filter_map(|route| route.context.as_ref())
                    .collect(),
            )
        }
    }
}

/// The conversation operation a packet records, one key per reached target,
/// and the question result it opens or answers.
///
/// An ASK mints one key, which is both its result handle and the key of its
/// one lane directive; an MSG or ANS mints one key per target.
fn operation(
    packet: &Packet<'_>,
    asserted: Option<TaskStamp>,
    targets: usize,
    minted_micros: &mut Option<i64>,
    expires: Instant,
) -> Result<
    (
        RoomPublicationKind,
        Vec<steer::Key>,
        Option<QuestionResultRef>,
    ),
    Unpublished,
> {
    Ok(match &packet.request {
        Request::Ans { result, .. } => (
            RoomPublicationKind::Ans {
                result: result.clone(),
                context: asserted,
            },
            mint_keys(minted_micros, targets)?,
            Some(result.clone()),
        ),
        Request::Ask { .. } => {
            let key = mint_keys(minted_micros, 1)?.remove(0);
            let result = key
                .as_str()
                .parse::<RoomAuthorityStamp>()
                .map_err(|error| {
                    Unpublished::Failed(typed_store_fault(
                        "ask::result-stamp-invalid",
                        &error.to_string(),
                    ))
                })?;
            let result = QuestionResultRef::new(result);
            (
                RoomPublicationKind::Ask {
                    result: result.clone(),
                    deadline: expires,
                },
                vec![key; targets],
                Some(result),
            )
        }
        Request::Msg { .. } => (
            RoomPublicationKind::Msg {},
            mint_keys(minted_micros, targets)?,
            None,
        ),
    })
}

/// Whether the record opens an operator item: an ANS returning to an operator
/// requester, or a packet routed to the operator.
fn opens_item(answer: Option<&Answerable>, routes: &[RoomRoute]) -> bool {
    match answer {
        Some(answer) => answer.record.requester() == &RoomDestination::Operator {},
        None => routes
            .iter()
            .any(|route| route.destination == RoomDestination::Operator {}),
    }
}

fn verb(request: &Request) -> Verb {
    match request {
        Request::Msg { .. } => Verb::Msg,
        Request::Ask { .. } => Verb::Ask,
        Request::Ans { .. } => Verb::Ans,
    }
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

/// The routes a conversation record freezes: the operator concern, each
/// admitted lane route with its own context, and their task contexts in header
/// order. A lane route is admitted when it delivers to a seat or queues for a
/// vacant lane; an operator route about a direction names one the operator sent
/// this lane.
fn conversation_routes(
    bus: &Bus,
    packet: &Packet<'_>,
    targets: &[Target],
    identities: &[CanonicalUuid],
) -> Result<Vec<RoomRoute>, Unpublished> {
    let mut directed = identities.iter();
    let mut routes = Vec::new();
    for route in packet.request_routes() {
        let context = context_room(route.context.as_ref());
        let destination = match route.audience {
            Audience::OperatorConcern => RoomDestination::Operator {},
            Audience::Occupant(_) => {
                let directive = directed.next().copied();
                match targets
                    .iter()
                    .find(|target| Some(target.directive) == directive)
                {
                    Some(target) => RoomDestination::Lane { lane: target.lane },
                    None => continue,
                }
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

/// One ANS checked against its stored question.
struct Answerable {
    result: QuestionResultRef,
    record: QuestionResultRecord,
    head: journal::JournalHead<QuestionResultHead>,
    /// The respondent's ASK directive, while it is still live.
    asked: Option<journal::JournalHead<wire::SteeringHead>>,
    /// The return route to a lane requester's seated occupant, in the stored
    /// context; empty for an operator requester.
    directed: Vec<Directed>,
    /// The stored question context, spelled as a handle.
    context: Option<TaskHandle>,
    /// The context the ANS asserted, which equals the stored one.
    asserted: Option<TaskStamp>,
}

/// Check one ANS against its stored question.
fn answerable(
    bus: &Bus,
    view: &bus::TaskSnapshot,
    lanes: &Seats,
    packet: &Packet<'_>,
    result: &QuestionResultRef,
    asserted: Option<&TaskHandle>,
) -> Result<Answerable, Unpublished> {
    let refuse = |reason: &str, detail: String| {
        Err(Unpublished::Refused {
            reason: reason.to_owned(),
            detail,
        })
    };
    let read_result = result.clone();
    let (stored, asked) = bus
        .read(move |read| {
            Ok((
                read.question_result(&read_result)?,
                read.steering_key(read_result.as_str())?,
            ))
        })
        .map_err(|error| {
            Unpublished::Failed(typed_store_fault(
                "steer::room-unreadable",
                &error.to_string(),
            ))
        })?;
    let Some((record, head)) = stored else {
        return refuse("ans::result-unknown", format!("{result} names no question"));
    };
    if record.respondent() != &(RoomDestination::Lane { lane: packet.lane }) {
        return refuse(
            "ans::respondent-other",
            format!("only the named respondent of {result} answers it"),
        );
    }
    if head.body().state() != &(QuestionResultState::Pending {}) {
        return refuse(
            "ans::result-settled",
            format!("{result} already has its one terminal result"),
        );
    }
    let asserted = asserted.map(|handle| handle.born().ok());
    if let Some(asserted) = &asserted
        && asserted.as_ref() != record.context()
    {
        return refuse(
            "ans::context-mismatch",
            format!("the asserted context is not the stored context of {result}"),
        );
    }
    let now = current_instant().map_err(Unpublished::Failed)?;
    if now >= record.deadline() {
        return refuse(
            "ans::deadline-passed",
            format!("{result} reached its deadline unanswered"),
        );
    }
    let context = record
        .context()
        .map(|room| {
            view.task_heads(room)
                .map(|heads| TaskHandle::for_project(&heads.statement().body().project, room))
                .ok_or_else(|| {
                    Unpublished::Failed(typed_store_fault(
                        "steer::room-head-missing",
                        &format!("frozen question context {room} has no task head"),
                    ))
                })
        })
        .transpose()?;
    let directed = match record.requester() {
        RoomDestination::Lane { lane } => match lanes.get(lane) {
            Some(Some((_, Some(address)))) => vec![(*address, context.clone())],
            _ => {
                return refuse(
                    UNAVAILABLE,
                    format!("requester lane {lane} of {result} seats no proved occupant"),
                );
            }
        },
        RoomDestination::Operator {} | RoomDestination::Task { .. } => Vec::new(),
    };
    let asked = asked.filter(|asked| {
        asked.body().disposition == Disposition::Live && asked.body().lane == packet.lane
    });
    Ok(Answerable {
        result: result.clone(),
        record,
        head,
        asked,
        directed,
        context,
        asserted: asserted.flatten(),
    })
}

impl Answerable {
    /// Guard the question unmoved and retire the still-live ASK directive in
    /// the answer's own write.
    fn stage(
        &self,
        proposal: &mut JournalWrite,
        packet: &Packet<'_>,
        epoch: CanonicalUuid,
        at: Instant,
    ) -> Result<(), Unpublished> {
        let (result, position) = (self.result.clone(), self.head.position());
        proposal.guard(JournalGuard::new(move |read| {
            match read.question_result(&result)? {
                Some((_, head)) if head.position() == position => Ok(()),
                _ => Err(GuardError::refused(RESULT_MOVED)),
            }
        }));
        let Some(asked) = &self.asked else {
            return Ok(());
        };
        let key = asked.body().key.parse::<steer::Key>().map_err(|error| {
            Unpublished::Failed(typed_protocol_fault(
                "steer::key-invalid",
                &error.to_string(),
            ))
        })?;
        proposal.guard(live_guard(asked.body().directive, asked.position()));
        let (pending, next) = super::transitions::append_disposition(
            proposal,
            asked,
            &key,
            Disposition::Acked,
            packet.locus.clone(),
            epoch,
            at,
        )
        .map_err(Unpublished::Failed)?;
        proposal.steering_head(pending, move |_| next);
        Ok(())
    }
}
