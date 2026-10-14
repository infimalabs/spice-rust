//! Room conversations: delivery receipts, question results, and operator
//! items each move under their own authority.

use super::*;
use app::RoomEventKind;
use wire::rooms::{
    QuestionResultRef, QuestionResultState, RoomConversed, RoomDelivery, RoomDestination,
    RoomDisposed, RoomPosted, RoomPublicationKind, RoomReceiptDisposition, RoomReceiptLapsed,
    RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState, RoomRoute,
};

const ASK_PACKET: &str = "dVsYdDj7m";
const ASK: &str = "dVsYdDj7n";
const RESULT: &str = "dVsYdDj7p";
const ANS_PACKET: &str = "dVsYdDj7q";
const ANS: &str = "dVsYdDj7r";
const ASKED: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
const RETURNED: &str = "018f82d4-6f59-7d06-92e3-bce913116302";

fn instant(text: &str) -> wire::Instant {
    text.parse().expect("fixture instant")
}

fn hunk(lane: &str, offset: u64) -> wire::rooms::TranscriptHunk {
    wire::rooms::TranscriptHunk {
        head: wire::TranscriptSourceHead {
            lane: uuid(lane),
            vendor: "omp".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid(REQUEST),
            next: wire::body::TranscriptPosition {
                member: 1,
                offset: 256,
            },
        },
        locus: wire::MessageLocus {
            member: 1,
            offset,
            length: 32,
            event: 0,
        },
        text: TextRange { start: 0, end: 12 },
    }
}

/// `from` tells `to` one room-scoped packet as `directive` under `key`.
fn told(position: u64, directive: &str, key: &str, from: &str, to: &str) -> serde_json::Value {
    let body = wire::body::SteerMessaged {
        directive: uuid(directive),
        key: key.to_owned(),
        lane: uuid(to),
        occupant: uuid(to),
        from: uuid(from),
        sender: uuid(from),
        text: "Packet body".to_owned(),
        locus: wire::body::TranscriptLocus {
            source: Some(uuid(REQUEST)),
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
            line: 1,
            column: 1,
        },
        room: Some(room()),
        expires: instant("2030-04-13T18:44:09.271Z"),
    };
    envelope(position, "steer::messaged", &body)
}

fn acked(position: u64, directive: &str, key: &str, lane: &str) -> serde_json::Value {
    let body = wire::body::SteerAcked {
        directive: uuid(directive),
        key: key.to_owned(),
        lane: uuid(lane),
        epoch: uuid(EPOCH),
        locus: wire::body::TranscriptLocus {
            source: Some(uuid(REQUEST)),
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
            line: 1,
            column: 1,
        },
    };
    envelope(position, "steer::acked", &body)
}

/// One lane packet's room record and the conversation the same write admits.
fn conversed(
    position: u64,
    (packet, publication): (&str, &str),
    author: &str,
    operation: RoomPublicationKind,
    routes: Vec<RoomRoute>,
    directed: &[&str],
) -> [serde_json::Value; 2] {
    let directed = directed
        .iter()
        .map(|directive| uuid(directive))
        .collect::<Vec<_>>();
    let source = hunk(author, position);
    [
        envelope(
            position,
            "room::messaged",
            &RoomMessaged {
                room: room(),
                publication: authority(packet),
                hunk: source.clone(),
                author: uuid(author),
                directed: directed.clone(),
            },
        ),
        envelope(
            position + 1,
            "room::conversed",
            &RoomConversed {
                publication: authority(publication),
                hunk: source,
                operation,
                routes,
                directed,
                expires: instant("2030-04-13T18:44:09.271Z"),
            },
        ),
    ]
}

fn ask_of_second_lane(position: u64) -> [serde_json::Value; 2] {
    conversed(
        position,
        (ASK_PACKET, ASK),
        LANE,
        RoomPublicationKind::Ask {
            result: QuestionResultRef::new(authority(RESULT)),
            deadline: instant("2030-04-12T19:44:09.271Z"),
        },
        vec![RoomRoute {
            destination: RoomDestination::Lane {
                lane: uuid(SECOND_LANE),
            },
            context: Some(room()),
            direction: None,
        }],
        &[ASKED],
    )
}

fn conversation_runtime(session: &Session, snapshot: &[serde_json::Value]) -> AppRuntime {
    let accepted = acceptance(session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let horizon = snapshot.len() as u64;
    runtime.apply_stream(&operator_facts(session, horizon));
    runtime.apply_stream(&room_page(session, snapshot, None, horizon));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    runtime
}

fn delivery(runtime: &AppRuntime, directive: &str) -> (wire::Disposition, bool) {
    let delivery = &runtime
        .room_projection()
        .conversation()
        .delivery(&uuid(directive))
        .expect("named delivery")
        .value;
    (delivery.disposition, delivery.queued)
}

fn result_state(runtime: &AppRuntime) -> QuestionResultState {
    runtime
        .room_projection()
        .conversation()
        .result(&QuestionResultRef::new(authority(RESULT)))
        .expect("question result")
        .value
        .state
        .clone()
}

/// The worked MSG/ASK/ACK/ANS/ACK sequence: a delivery receipt never settles
/// the question, and the answer returns under its own receipt.
#[test]
fn delivery_receipts_and_question_results_move_separately() {
    let session = session(OPERATOR, EPOCH);
    let [packet, asked] = ask_of_second_lane(2);
    let mut runtime = conversation_runtime(
        &session,
        &[told(1, ASKED, RESULT, LANE, SECOND_LANE), packet, asked],
    );
    let Some(RoomEventKind::Messaged { conversation, .. }) = runtime
        .room_projection()
        .room(&room())
        .and_then(|state| state.events().get(&authority(ASK_PACKET)))
        .map(|event| event.kind.clone())
    else {
        panic!("the ASK packet is a room event");
    };
    assert_eq!(
        conversation,
        Some(authority(ASK)),
        "the room packet links the conversation its own write admitted"
    );
    assert_eq!(delivery(&runtime, ASKED), (wire::Disposition::Live, false));
    assert_eq!(result_state(&runtime), QuestionResultState::Pending {});

    live(&mut runtime, &session, acked(4, ASKED, RESULT, SECOND_LANE));
    assert_eq!(delivery(&runtime, ASKED).0, wire::Disposition::Acked);
    assert_eq!(
        result_state(&runtime),
        QuestionResultState::Pending {},
        "ACK of the ASK delivery leaves its result pending"
    );

    live(
        &mut runtime,
        &session,
        told(5, RETURNED, "dReturn1", SECOND_LANE, LANE),
    );
    let [packet, answered] = conversed(
        6,
        (ANS_PACKET, ANS),
        SECOND_LANE,
        RoomPublicationKind::Ans {
            result: QuestionResultRef::new(authority(RESULT)),
            context: None,
        },
        Vec::new(),
        &[RETURNED],
    );
    live(&mut runtime, &session, packet);
    live(&mut runtime, &session, answered);
    let QuestionResultState::Answered {
        answer,
        return_delivery,
    } = result_state(&runtime)
    else {
        panic!("the answer settles the result");
    };
    assert_eq!(answer, authority(ANS));
    assert_eq!(
        return_delivery.delivery(),
        &RoomDelivery::Directive {
            directive: uuid(RETURNED)
        }
    );
    assert_eq!(
        delivery(&runtime, RETURNED),
        (wire::Disposition::Live, false),
        "the return awaits its own receipt"
    );

    live(&mut runtime, &session, acked(8, RETURNED, "dReturn1", LANE));
    assert_eq!(delivery(&runtime, RETURNED).0, wire::Disposition::Acked);
    assert!(
        matches!(result_state(&runtime), QuestionResultState::Answered { .. }),
        "the return receipt never moves the answered result"
    );
}

fn answer_operator_question(runtime: &mut AppRuntime, session: &Session, expires: wire::Instant) {
    live(
        runtime,
        session,
        envelope(
            6,
            "steer::sent",
            &serde_json::json!({
                "directive": RETURNED,
                "key": "dReturn1",
                "lane": LANE,
                "text": "Operator answer",
                "room": room(),
                "expires": "2030-04-13T18:44:09.271Z",
            }),
        ),
    );
    live(
        runtime,
        session,
        envelope(
            7,
            "room::posted",
            &RoomPosted {
                publication: authority(ANS),
                text: "Operator answer".to_owned(),
                operation: RoomPublicationKind::Ans {
                    result: QuestionResultRef::new(authority(RESULT)),
                    context: None,
                },
                routes: Vec::new(),
                directed: vec![uuid(RETURNED)],
                expires,
            },
        ),
    );
}

/// A lane's ASK of the operator opens an item the operator settles; its lapse
/// and recovery keep the same key, ACK keeps the separate answer duty visible,
/// and the operator's answer settles the result and records in its room.
#[test]
fn operator_items_lapse_recover_and_retire_with_the_answer() {
    let session = session(OPERATOR, EPOCH);
    let item = RoomReceiptRef::new(authority(RESULT));
    let [packet, asked] = conversed(
        1,
        (ASK_PACKET, ASK),
        LANE,
        RoomPublicationKind::Ask {
            result: QuestionResultRef::new(authority(RESULT)),
            deadline: instant("2030-04-12T19:44:09.271Z"),
        },
        vec![RoomRoute {
            destination: RoomDestination::Operator {},
            context: Some(room()),
            direction: None,
        }],
        &[],
    );
    let mut runtime = conversation_runtime(&session, &[packet, asked]);
    let state = |runtime: &AppRuntime| {
        runtime
            .room_projection()
            .conversation()
            .item(&item)
            .expect("operator item")
            .value
            .state
            .clone()
    };
    let open = |runtime: &AppRuntime| runtime.room_projection().conversation().inbox_items().len();
    assert!(matches!(state(&runtime), RoomReceiptState::Pending { .. }));
    assert_eq!(open(&runtime), 1);

    live(
        &mut runtime,
        &session,
        envelope(
            3,
            "room::lapsed",
            &RoomReceiptLapsed {
                receipt: item.clone(),
            },
        ),
    );
    assert!(matches!(state(&runtime), RoomReceiptState::Expired { .. }));
    assert_eq!(open(&runtime), 1, "a lapsed item still owes a disposition");
    let expires = instant("2030-04-14T18:44:09.271Z");
    live(
        &mut runtime,
        &session,
        envelope(
            4,
            "room::recovered",
            &RoomReceiptRecovered {
                receipt: item.clone(),
                expires,
            },
        ),
    );
    assert_eq!(state(&runtime), RoomReceiptState::Pending { expires });

    live(
        &mut runtime,
        &session,
        envelope(
            5,
            "room::disposed",
            &RoomDisposed {
                disposition: RoomReceiptDisposition::Ack,
                keys: vec![item.clone()],
                clause: "I will answer".to_owned(),
            },
        ),
    );
    assert!(matches!(state(&runtime), RoomReceiptState::Acked { .. }));
    let inbox = runtime.room_projection().conversation().inbox_items();
    assert_eq!(
        inbox.len(),
        1,
        "ACK leaves the operator answer duty visible"
    );
    assert!(matches!(
        inbox[0].result.map(|result| &result.value.state),
        Some(QuestionResultState::Pending {})
    ));

    // The operator's answer returns to the lane requester under the operator
    // direction its own write opens.
    answer_operator_question(&mut runtime, &session, expires);
    assert_eq!(
        delivery(&runtime, RETURNED),
        (wire::Disposition::Live, false)
    );
    assert!(matches!(
        result_state(&runtime),
        QuestionResultState::Answered { answer, .. } if answer == authority(ANS)
    ));
    assert!(matches!(state(&runtime), RoomReceiptState::Acked { .. }));
    assert_eq!(open(&runtime), 0);
    assert_eq!(
        runtime
            .room_projection()
            .room(&room())
            .and_then(|state| state.events().get(&authority(ANS)))
            .map(|event| &event.kind),
        Some(&RoomEventKind::Posted),
        "the operator answer records in the question's room"
    );
}

fn operator_message(position: u64, publication: &str) -> serde_json::Value {
    envelope(
        position,
        "room::posted",
        &RoomPosted {
            publication: authority(publication),
            text: format!("Operator item {publication}"),
            operation: RoomPublicationKind::Msg {},
            routes: vec![RoomRoute {
                destination: RoomDestination::Operator {},
                context: Some(room()),
                direction: None,
            }],
            directed: Vec::new(),
            expires: instant("2030-04-13T18:44:09.271Z"),
        },
    )
}

#[test]
fn operator_inbox_order_is_immutable_across_receipt_lapse_and_recovery() {
    const FIRST: &str = "dVsYdDj7s";
    const SECOND: &str = "dVsYdDj7t";
    let session = session(OPERATOR, EPOCH);
    let mut runtime = conversation_runtime(
        &session,
        &[operator_message(1, FIRST), operator_message(2, SECOND)],
    );
    let order = |runtime: &AppRuntime| {
        runtime
            .room_projection()
            .conversation()
            .inbox_items()
            .into_iter()
            .map(|entry| (entry.key.to_string(), entry.item.value.admitted))
            .collect::<Vec<_>>()
    };
    let stable = vec![(FIRST.to_owned(), seq(1)), (SECOND.to_owned(), seq(2))];
    assert_eq!(order(&runtime), stable);

    let first = RoomReceiptRef::new(authority(FIRST));
    live(
        &mut runtime,
        &session,
        envelope(
            3,
            "room::lapsed",
            &RoomReceiptLapsed {
                receipt: first.clone(),
            },
        ),
    );
    assert_eq!(
        order(&runtime),
        stable,
        "the latest head position does not become presentation order"
    );
    live(
        &mut runtime,
        &session,
        envelope(
            4,
            "room::recovered",
            &RoomReceiptRecovered {
                receipt: first,
                expires: instant("2030-04-14T18:44:09.271Z"),
            },
        ),
    );
    assert_eq!(order(&runtime), stable);
}

fn conversation_text_reply(
    session: &Session,
    publication: &str,
    content: wire::intent::RoomConversationContent,
    direction: Option<wire::intent::RoomConcernedDirection>,
) -> app::Admitted<Response<wire::intent::RoomConversationTextAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(9)),
            body: wire::intent::RoomConversationTextAnswer {
                publication: authority(publication),
                content,
                direction,
            },
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("conversation text response is admitted")
}

/// A lane's operator MSG and ASK carry their words only in the author's
/// transcript, so the runtime asks the resident for each exactly once and
/// shows what it read; operator-authored text needs no read. Only the ASK
/// carries an answer owed back to its requesting lane.
#[test]
fn operator_items_read_lane_text_once_and_only_asks_owe_an_answer() {
    const MSG_PACKET: &str = "dVsYdDj7v";
    const MSG: &str = "dVsYdDj7w";
    const OPERATOR_MSG: &str = "dVsYdDj7x";
    let session = session(OPERATOR, EPOCH);
    let [ask_packet, asked] = conversed(
        1,
        (ASK_PACKET, ASK),
        LANE,
        RoomPublicationKind::Ask {
            result: QuestionResultRef::new(authority(RESULT)),
            deadline: instant("2030-04-12T19:44:09.271Z"),
        },
        operator_route(Some(room()), None),
        &[],
    );
    let [msg_packet, messaged] = conversed(
        3,
        (MSG_PACKET, MSG),
        LANE,
        RoomPublicationKind::Msg {},
        operator_route(Some(room()), None),
        &[],
    );
    let snapshot = [
        ask_packet,
        asked,
        msg_packet,
        messaged,
        operator_message(5, OPERATOR_MSG),
    ];
    let mut runtime = conversation_runtime(&session, &snapshot);

    let inbox = runtime.room_projection().conversation().inbox_items();
    let duty = |publication: &str| {
        inbox
            .iter()
            .find(|entry| *entry.item.value.record.publication() == authority(publication))
            .expect("open operator item")
            .result
            .map(|result| {
                (
                    result.value.record.requester().clone(),
                    result.value.record.respondent().clone(),
                )
            })
    };
    assert_eq!(
        duty(ASK),
        Some((
            RoomDestination::Lane { lane: uuid(LANE) },
            RoomDestination::Operator {}
        )),
        "the operator answers an ASK back to its requesting lane"
    );
    assert_eq!(duty(MSG), None, "a MSG asks only for receipt");

    assert_eq!(
        runtime.begin_operator_item_texts(),
        vec![authority(ASK), authority(MSG)],
        "only lane-authored items need their transcript read"
    );
    assert!(runtime.begin_operator_item_texts().is_empty());

    let asked_text = "Build bounded room automation? A, B or C.";
    runtime.apply_conversation_text(
        &authority(ASK),
        &conversation_text_reply(
            &session,
            ASK,
            wire::intent::RoomConversationContent::Text {
                text: asked_text.to_owned(),
            },
            None,
        ),
    );
    assert_eq!(
        runtime.conversation_text(&authority(ASK)),
        Some(&app::ConversationText::Read {
            content: wire::intent::RoomConversationContent::Text {
                text: asked_text.to_owned()
            },
            direction: None,
        })
    );

    // A drop loses the MSG read in flight; the next subscription asks again
    // while the settled ASK text stands.
    runtime.reconnect(&acceptance(&session, "operator", EPOCH));
    runtime.apply_stream(&operator_facts(&session, snapshot.len() as u64));
    runtime.apply_stream(&room_page(&session, &snapshot, None, snapshot.len() as u64));
    assert_eq!(runtime.begin_operator_item_texts(), vec![authority(MSG)]);

    // A reply naming another publication never becomes this one's text.
    runtime.apply_conversation_text(
        &authority(MSG),
        &conversation_text_reply(
            &session,
            ASK,
            wire::intent::RoomConversationContent::Text {
                text: asked_text.to_owned(),
            },
            None,
        ),
    );
    assert!(matches!(
        runtime.conversation_text(&authority(MSG)),
        Some(app::ConversationText::Failed(_))
    ));
}

fn operator_route(
    context: Option<wire::rooms::RoomAnchor>,
    direction: Option<&str>,
) -> Vec<RoomRoute> {
    vec![RoomRoute {
        destination: RoomDestination::Operator {},
        context,
        direction: direction.map(str::to_owned),
    }]
}

/// Current operator items concern a direction or task; a retained pre-cutover
/// route with neither still replays and presents as unnamed.
#[test]
fn operator_items_concern_their_direction_or_task_and_retain_legacy_unnamed() {
    const MSG_PACKET: &str = "dVsYdDj7v";
    const MSG: &str = "dVsYdDj7w";
    const BARE_PACKET: &str = "dVsYdDj7x";
    const BARE: &str = "dVsYdDj7y";
    const DIRECTION: &str = "dVwg6Zqlf";
    let session = session(OPERATOR, EPOCH);
    let [ask_packet, asked] = conversed(
        1,
        (ASK_PACKET, ASK),
        LANE,
        RoomPublicationKind::Ask {
            result: QuestionResultRef::new(authority(RESULT)),
            deadline: instant("2030-04-12T19:44:09.271Z"),
        },
        operator_route(Some(room()), None),
        &[],
    );
    let [msg_packet, messaged] = conversed(
        3,
        (MSG_PACKET, MSG),
        LANE,
        RoomPublicationKind::Msg {},
        operator_route(None, Some(DIRECTION)),
        &[],
    );
    let [bare_packet, bare] = conversed(
        5,
        (BARE_PACKET, BARE),
        LANE,
        RoomPublicationKind::Msg {},
        operator_route(None, None),
        &[],
    );
    let snapshot = [ask_packet, asked, msg_packet, messaged, bare_packet, bare];
    let mut runtime = conversation_runtime(&session, &snapshot);
    let concern = |runtime: &app::AppRuntime, publication: &str| {
        let inbox = runtime.room_projection().conversation().inbox_items();
        let entry = inbox
            .iter()
            .find(|entry| *entry.item.value.record.publication() == authority(publication))
            .expect("open operator item");
        runtime.item_concern(&entry.item.value.record)
    };
    assert_eq!(concern(&runtime, ASK), app::ItemConcern::Task(room()));
    assert_eq!(concern(&runtime, BARE), app::ItemConcern::Unnamed);
    assert_eq!(
        concern(&runtime, MSG),
        app::ItemConcern::Direction {
            key: DIRECTION.to_owned(),
            text: None
        },
        "the direction is named before its text is read"
    );

    runtime.apply_conversation_text(
        &authority(MSG),
        &conversation_text_reply(
            &session,
            MSG,
            wire::intent::RoomConversationContent::Text {
                text: "Started A.".to_owned(),
            },
            Some(wire::intent::RoomConcernedDirection {
                key: DIRECTION.to_owned(),
                text: "Which first?".to_owned(),
            }),
        ),
    );
    assert_eq!(
        concern(&runtime, MSG),
        app::ItemConcern::Direction {
            key: DIRECTION.to_owned(),
            text: Some("Which first?".to_owned())
        }
    );
}
