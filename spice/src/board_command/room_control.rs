//! Operator room controls and explicit question closure.
//!
//! The operator ACKs or NAKs its exact operator items, answers questions
//! whose stored respondent is the operator, and recovers lapsed items. The
//! stored respondent declines and the stored requester cancels one pending
//! question; the operator closes for a lane party only by explicit override,
//! which the closure's operator actor then attributes. Each command reads the
//! current item and result heads, refuses what the journal would refuse with
//! the repair the caller can take, and guards every head it read, so a
//! concurrent move refuses instead of overwriting.

use super::*;

use wire::CanonicalUuid;
use wire::intent::{RoomClose, RoomControl, RoomControlAnswer, RoomItemMove, RoomResultMove};
use wire::rooms::{
    QuestionClosure, QuestionResultHead, QuestionResultRecord, QuestionResultRef,
    QuestionResultState, ROOM_CONTROL_RETURN_RESERVE_BYTES, ROOM_PUBLICATION_BYTES_LIMIT,
    ROOM_RELEVANCE_TEXT_LIMIT, RoomAnchor, RoomAuthorityStamp, RoomDestination, RoomDisposed,
    RoomPosted, RoomPublicationKind, RoomQuestionClosed, RoomReceiptDisposition, RoomReceiptHead,
    RoomReceiptRecord, RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState,
    RoomRelevanceAssessed, RoomRelevanceAssessment, RoomRelevanceVerdict,
};

/// Guard reason when an item, result, or requester seat moved between this
/// command's read and its write.
const MOVED: &str = "room::control-moved";

type Item = (RoomReceiptRecord, journal::JournalHead<RoomReceiptHead>);
type Question = (
    QuestionResultRecord,
    journal::JournalHead<QuestionResultHead>,
);

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
        RoomControl::Answer {
            result,
            context,
            text,
        } => answer(
            bus,
            command,
            control,
            config,
            result,
            context.as_ref(),
            text,
        )
        .map(|(answer, lane)| (answer, Some(lane))),
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

/// Decline or cancel one pending question as its stored party, or as the
/// operator overriding a lane party.
pub(crate) fn room_close(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    close: &RoomClose,
) -> Outcome<CommandAnswer> {
    closed(bus, command, close).unwrap_or_else(|outcome| outcome)
}

fn dispose(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    disposition: RoomReceiptDisposition,
    keys: &[RoomReceiptRef],
    clause: &str,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    require_clause(clause)?;
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
        results: Vec::new(),
        answer: None,
        returned: None,
        assessed: None,
    };
    for key in control_shape.keys() {
        let (_, head) = item(bus, key)?;
        guard_item(&mut write, key, head.position());
        let question = match disposition {
            RoomReceiptDisposition::Nak => item_question(bus, key)?,
            RoomReceiptDisposition::Ack => None,
        };
        let declines = question.as_ref().is_some_and(|(record, question)| {
            matches!(question.body().state(), QuestionResultState::Pending {})
                && record.control_result(&control_shape).is_some()
        });
        let state = head.body().state();
        if !matches!(state, RoomReceiptState::Pending { .. }) && !declines {
            return Err(item_refusal(key, state));
        }
        if matches!(state, RoomReceiptState::Pending { .. }) {
            answer.items.push(RoomItemMove {
                item: key.clone(),
                state: control_shape.receipt_state().name().to_owned(),
            });
        }
        if let (true, Some((record, question))) = (declines, question) {
            guard_result(&mut write, record.result(), question.position());
            answer.results.push(RoomResultMove {
                result: record.result().clone(),
                state: "declined".to_owned(),
            });
        }
    }
    write
        .fact(fact, FactCause::Root)
        .map_err(|error| store_fault("room::control-build", &error.to_string()))?;
    Ok(commit(bus, command, control, write, answer))
}

fn answer(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    control: &RoomControl,
    config: &steer::Config,
    result: &QuestionResultRef,
    asserted: Option<&TaskHandle>,
    text: &str,
) -> Result<(Outcome<CommandAnswer>, CanonicalUuid), Outcome<CommandAnswer>> {
    let ((record, head), at, lane) = answerable(bus, result, asserted, text)?;
    let context = record.context().cloned();
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
    guard_result(&mut write, result, head.position());
    guard_residence(&mut write, lane);
    let returned = format!("ANS {result}: {text}");
    let (key, _) = crate::resident::append_operator_directive(
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
    let minted = key
        .parse::<steer::Key>()
        .ok()
        .and_then(|key| key.minted_micros())
        .ok_or_else(|| store_fault("room::return-key-invalid", &key))?;
    let micros = jiff::Timestamp::now()
        .as_microsecond()
        .max(minted.saturating_add(1));
    let publication = mint_stamp(bus, context.as_ref(), micros)?;
    let mut answer = RoomControlAnswer {
        items: Vec::new(),
        results: vec![RoomResultMove {
            result: result.clone(),
            state: "answered".to_owned(),
        }],
        answer: Some(publication.clone()),
        returned: Some(key),
        assessed: None,
    };
    if let Some(item_key) = record.item() {
        let (_, item_head) = item(bus, &item_key)?;
        if matches!(item_head.body().state(), RoomReceiptState::Pending { .. }) {
            guard_item(&mut write, &item_key, item_head.position());
            answer.items.push(RoomItemMove {
                item: item_key,
                state: "acked".to_owned(),
            });
        }
    }
    write
        .fact(
            RoomPosted {
                publication,
                text: text.to_owned(),
                operation: RoomPublicationKind::Ans {
                    result: result.clone(),
                    context,
                },
                routes: Vec::new(),
                directed: vec![command.id()],
                expires: expiry(at, config)?,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::answer-build", &error.to_string()))?;
    crate::resident::steering_envelope_admits(&write)?;
    Ok((commit(bus, command, control, write, answer), lane))
}

/// Reply to one pending operator item a lane's MSG or ANS opened.
///
/// The reply is operator direction to the authoring lane, recorded as an
/// operator MSG routed to that lane in the item's room, and the same write
/// settles the item acked. An ASK item is answered, never replied to.
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
        results: Vec::new(),
        answer: Some(publication),
        returned: Some(returned_key),
        assessed: None,
    };
    Ok((commit(bus, command, control, write, answer), lane))
}

/// The pending lane-opened item `text` may reply to now, with the resident
/// lane the reply returns to, how the reply cites what it answers, and the
/// source publication's one unambiguous room, when it names one.
fn repliable(
    bus: &Bus,
    key: &RoomReceiptRef,
    text: &str,
) -> Result<
    (
        journal::JournalHead<RoomReceiptHead>,
        CanonicalUuid,
        String,
        Option<RoomAnchor>,
    ),
    Outcome<CommandAnswer>,
> {
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
    let cited = match opened.kind() {
        wire::rooms::RoomPublicationKind::Msg {} => opened
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
            })?,
        wire::rooms::RoomPublicationKind::Ans { result, .. } => result.to_string(),
        wire::rooms::RoomPublicationKind::Ask { result, .. } => {
            return Err(refusal(
                "room::reply-question",
                Some(result.to_string()),
                &format!("a question takes an answer: `spice room answer {result} TEXT`"),
            ));
        }
    };
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

/// The item context stored for an ANS return, or the one distinct room an MSG
/// explicitly speaks in. A multi-room packet has no single room for one
/// operator post to enter.
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

/// The pending operator-answered question `text` may answer now, with the
/// admission instant and the resident lane its answer returns to.
fn answerable(
    bus: &Bus,
    result: &QuestionResultRef,
    asserted: Option<&TaskHandle>,
    text: &str,
) -> Result<(Question, Instant, CanonicalUuid), Outcome<CommandAnswer>> {
    require_text(text, "answer")?;
    let (record, head) = question(bus, result)?;
    if record.respondent() != &(RoomDestination::Operator {}) {
        return Err(refusal(
            "room::not-respondent",
            Some(destination(record.respondent())),
            &format!(
                "the stored respondent answers with `ANS {result}` in its own prose; the operator \
                 may decline for it with `spice room decline {result} --clause TEXT --override`"
            ),
        ));
    }
    require_pending(result, head.body())?;
    if let Some(asserted) = asserted {
        let stamp = asserted.born().map_err(|error| {
            refusal(
                "room::context-invalid",
                Some(asserted.to_string()),
                &format!("name the question's context as a task handle: {error}"),
            )
        })?;
        if Some(&stamp) != record.context() {
            return Err(refusal(
                "room::context-mismatch",
                Some(
                    record
                        .context()
                        .map_or_else(|| "no context".to_owned(), ToString::to_string),
                ),
                "omit --context or name the question's stored context",
            ));
        }
    }
    let at = now()?;
    if at >= record.deadline() {
        return Err(refusal(
            "room::deadline-passed",
            Some(record.deadline().to_string()),
            &format!(
                "the result times out at its deadline; close it now with `spice room cancel \
                 {result} --clause TEXT --override`"
            ),
        ));
    }
    let RoomDestination::Lane { lane } = *record.requester() else {
        return Err(store_fault(
            "room::requester-invalid",
            "an operator-answered question names no lane requester",
        ));
    };
    if !lane_resides(bus, lane)? {
        return Err(refusal(
            "room::requester-gone",
            Some(lane.to_string()),
            &format!(
                "the requesting lane left residency; close the question with `spice room cancel \
                 {result} --clause TEXT --override`"
            ),
        ));
    }
    Ok(((record, head), at, lane))
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
        results: Vec::new(),
        answer: None,
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
        results: Vec::new(),
        answer: None,
        returned: None,
        assessed: Some((observation.clone(), verdict)),
    };
    Ok(commit(bus, command, control, write, answer))
}

fn closed(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    close: &RoomClose,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    require_clause(&close.clause)?;
    let result = &close.result;
    let (record, head) = question(bus, result)?;
    require_pending(result, head.body())?;
    let (role, party, verb) = match close.closure {
        QuestionClosure::Declined => ("respondent", record.respondent(), "decline"),
        QuestionClosure::Cancelled => ("requester", record.requester(), "cancel"),
    };
    let actor = command.actor().clone();
    let operator = command.holds(wire::principal::PrincipalRole::Operator);
    match (&actor, party, close.override_party) {
        (_, RoomDestination::Operator {}, true) if operator => {
            return Err(refusal(
                "room::override-unneeded",
                Some(role.to_owned()),
                &format!(
                    "the operator is the stored {role}; repeat without --override: `spice room \
                     {verb} {result} --clause TEXT`"
                ),
            ));
        }
        (_, RoomDestination::Lane { lane }, false) if operator => {
            return Err(refusal(
                "room::override-required",
                Some(lane.to_string()),
                &format!(
                    "the stored {role} is lane {lane}; let it {verb}, or act for it with `spice \
                     room {verb} {result} --clause TEXT --override`"
                ),
            ));
        }
        _ if operator => {}
        (_, _, true) => {
            return Err(refusal(
                "room::override-operator-only",
                None,
                "only the operator overrides a stored party; drop --override",
            ));
        }
        (Actor::Lane(lane), RoomDestination::Lane { lane: stored }, false) if lane == stored => {}
        _ => {
            return Err(refusal(
                "room::not-party",
                Some(destination(party)),
                &format!(
                    "only the stored {role} may {verb} this question; ask the operator to \
                     override with `spice room {verb} {result} --clause TEXT --override`"
                ),
            ));
        }
    }
    let at = now()?;
    let origin = match record.context() {
        Some(room) => task_origin(room)?,
        None => resident_origin(command)?,
    };
    let mut write = JournalWrite::new(actor, origin, at, Producer::CommandDoor);
    guard_result(&mut write, result, head.position());
    let state = match close.closure {
        QuestionClosure::Declined => "declined",
        QuestionClosure::Cancelled => "cancelled",
    };
    let mut answer = RoomControlAnswer {
        items: Vec::new(),
        results: vec![RoomResultMove {
            result: result.clone(),
            state: state.to_owned(),
        }],
        answer: None,
        returned: None,
        assessed: None,
    };
    if let Some(item_key) = record.item() {
        let (_, item_head) = item(bus, &item_key)?;
        if matches!(item_head.body().state(), RoomReceiptState::Pending { .. }) {
            guard_item(&mut write, &item_key, item_head.position());
            answer.items.push(RoomItemMove {
                item: item_key,
                state: state.to_owned(),
            });
        }
    }
    write
        .fact(
            RoomQuestionClosed {
                result: result.clone(),
                closure: close.closure,
                clause: close.clause.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::close-build", &error.to_string()))?;
    Ok(commit(bus, command, close, write, answer))
}

/// A decision clause must say something and fit the control capacity its
/// item reserved at admission, so a saturated scope still admits it.
fn require_clause(clause: &str) -> Result<(), Outcome<CommandAnswer>> {
    if clause.trim().is_empty() {
        return Err(refusal(
            "room::clause-required",
            None,
            "pass a nonblank --clause stating the decision",
        ));
    }
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

fn require_pending(
    result: &QuestionResultRef,
    head: &QuestionResultHead,
) -> Result<(), Outcome<CommandAnswer>> {
    if head.state().is_terminal() {
        return Err(refusal(
            "room::result-settled",
            Some(head.state().name().to_owned()),
            &format!("nothing remains to settle; read `spice room result {result}`"),
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

fn question(bus: &Bus, result: &QuestionResultRef) -> Result<Question, Outcome<CommandAnswer>> {
    let probe = result.clone();
    bus.read(move |read| read.question_result(&probe))
        .map_err(|error| store_fault("room::result-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            refusal(
                "room::result-unknown",
                Some(result.to_string()),
                "name a question result handle as its ASK minted it",
            )
        })
}

/// The question an operator ASK item stands for, when the item is one.
fn item_question(
    bus: &Bus,
    key: &RoomReceiptRef,
) -> Result<Option<Question>, Outcome<CommandAnswer>> {
    let result = QuestionResultRef::new(key.authority().clone());
    let probe = result.clone();
    let question = bus
        .read(move |read| read.question_result(&probe))
        .map_err(|error| store_fault("room::result-unreadable", &error.to_string()))?;
    Ok(question.filter(|(record, _)| record.item().as_ref() == Some(key)))
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

fn guard_result(write: &mut JournalWrite, result: &QuestionResultRef, position: Seq) {
    let result = result.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.question_result(&result)? {
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
fn mint_stamp(
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
    match committed_preserving_guard_refusal(bus, write, CommandAnswer::RoomControl(answer), MOVED)
    {
        Outcome::Refusal(refused) if refused.reason() == MOVED => refusal(
            MOVED,
            None,
            "an item, result, or requester seat moved after this command read it; read `spice \
             room inbox` or `spice room result RESULT` and retry",
        ),
        outcome => outcome,
    }
}

fn now() -> Result<Instant, Outcome<CommandAnswer>> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("room::clock-invalid", &error.to_string()))
}

fn expiry(at: Instant, config: &steer::Config) -> Result<Instant, Outcome<CommandAnswer>> {
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

fn destination(destination: &RoomDestination) -> String {
    match destination {
        RoomDestination::Operator {} => "operator".to_owned(),
        RoomDestination::Lane { lane } => format!("lane {lane}"),
        RoomDestination::Task { task } => format!("room {task}"),
    }
}
