//! Current MSG room publications, delivery receipts, and operator items each
//! move under their own authority.

use super::*;
use app::RoomEventKind;
use wire::rooms::{
    RoomConversed, RoomDestination, RoomDisposed, RoomPosted, RoomPublicationKind,
    RoomReceiptDisposition, RoomReceiptLapsed, RoomReceiptRecovered, RoomReceiptRef,
    RoomReceiptState, RoomRoute,
};

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

fn told(position: u64, directive: &str, key: &str, from: &str, to: &str) -> serde_json::Value {
    envelope(
        position,
        "steer::messaged",
        &wire::body::SteerMessaged {
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
            addressed: None,
            expires: instant("2030-04-13T18:44:09.271Z"),
        },
    )
}

fn acked(position: u64, directive: &str, key: &str, lane: &str) -> serde_json::Value {
    envelope(
        position,
        "steer::acked",
        &wire::body::SteerAcked {
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
        },
    )
}

fn conversed(
    position: u64,
    packet: &str,
    publication: &str,
    author: &str,
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
                operation: RoomPublicationKind::Msg {},
                routes,
                directed,
                expires: instant("2030-04-13T18:44:09.271Z"),
            },
        ),
    ]
}

fn conversation_runtime_as(
    session: &Session,
    actor: &str,
    snapshot: &[serde_json::Value],
) -> AppRuntime {
    let accepted = acceptance(session, actor, EPOCH);
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

fn conversation_runtime(session: &Session, snapshot: &[serde_json::Value]) -> AppRuntime {
    conversation_runtime_as(session, "operator", snapshot)
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

fn operator_message(position: u64, publication: &str) -> serde_json::Value {
    envelope(
        position,
        "room::posted",
        &RoomPosted {
            publication: authority(publication),
            text: format!("Operator item {publication}"),
            operation: RoomPublicationKind::Msg {},
            routes: operator_route(Some(room()), None),
            directed: Vec::new(),
            expires: instant("2030-04-13T18:44:09.271Z"),
        },
    )
}

fn conversation_text_reply(
    session: &Session,
    publication: &str,
    direction: Option<wire::intent::RoomConcernedDirection>,
    recipient: Option<wire::Actor>,
) -> app::Admitted<Response<wire::intent::RoomConversationTextAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(9)),
            body: wire::intent::RoomConversationTextAnswer {
                publication: authority(publication),
                content: wire::intent::RoomConversationContent::Text {
                    text: "Started A.".to_owned(),
                },
                direction,
                recipient,
            },
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("conversation text response is admitted")
}

#[test]
fn msg_delivery_receipt_moves_without_rewriting_the_room_event() {
    const PACKET: &str = "dVsYdDj7m";
    const MESSAGE: &str = "dVsYdDj7n";
    const DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
    let session = session(OPERATOR, EPOCH);
    let [packet, message] = conversed(
        2,
        PACKET,
        MESSAGE,
        LANE,
        vec![RoomRoute {
            destination: RoomDestination::Lane {
                lane: uuid(SECOND_LANE),
            },
            context: Some(room()),
            direction: None,
        }],
        &[DIRECTIVE],
    );
    let mut runtime = conversation_runtime(
        &session,
        &[
            told(1, DIRECTIVE, MESSAGE, LANE, SECOND_LANE),
            packet,
            message,
        ],
    );
    let event = runtime
        .room_projection()
        .room(&room())
        .and_then(|state| state.events().get(&authority(PACKET)))
        .expect("MSG packet is a room event");
    assert!(matches!(
        &event.kind,
        RoomEventKind::Messaged {
            conversation: Some(conversation),
            ..
        } if *conversation == authority(MESSAGE)
    ));
    assert_eq!(
        runtime
            .room_projection()
            .conversation()
            .delivery(&uuid(DIRECTIVE))
            .expect("delivery")
            .value
            .disposition,
        wire::Disposition::Live
    );
    live(
        &mut runtime,
        &session,
        acked(4, DIRECTIVE, MESSAGE, SECOND_LANE),
    );
    assert_eq!(
        runtime
            .room_projection()
            .conversation()
            .delivery(&uuid(DIRECTIVE))
            .expect("delivery")
            .value
            .disposition,
        wire::Disposition::Acked
    );
}

#[test]
fn operator_item_lapses_recovers_and_settles_under_one_key() {
    const ITEM: &str = "dVsYdDj7s";
    let session = session(OPERATOR, EPOCH);
    let mut runtime = conversation_runtime(&session, &[operator_message(1, ITEM)]);
    let item = RoomReceiptRef::new(authority(ITEM));
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
    assert!(matches!(state(&runtime), RoomReceiptState::Pending { .. }));
    live(
        &mut runtime,
        &session,
        envelope(
            2,
            "room::lapsed",
            &RoomReceiptLapsed {
                receipt: item.clone(),
            },
        ),
    );
    assert!(matches!(state(&runtime), RoomReceiptState::Expired { .. }));
    let expires = instant("2030-04-14T18:44:09.271Z");
    live(
        &mut runtime,
        &session,
        envelope(
            3,
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
            4,
            "room::disposed",
            &RoomDisposed {
                disposition: RoomReceiptDisposition::Ack,
                keys: vec![item.clone()],
                clause: "Handled".to_owned(),
            },
        ),
    );
    assert!(matches!(state(&runtime), RoomReceiptState::Acked { .. }));
    assert!(
        runtime
            .room_projection()
            .conversation()
            .open_items()
            .is_empty()
    );
}

#[test]
fn operator_inbox_keeps_admission_order_across_lapse_and_recovery() {
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
            .open_items()
            .into_iter()
            .map(|entry| (entry.key.to_string(), entry.item.value.admitted))
            .collect::<Vec<_>>()
    };
    let stable = vec![(FIRST.to_owned(), seq(1)), (SECOND.to_owned(), seq(2))];
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
    assert_eq!(order(&runtime), stable);
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

#[test]
fn inbox_text_reads_once_and_routes_only_to_its_principal() {
    const MINE_PACKET: &str = "dVsYdDj7v";
    const MINE: &str = "dVsYdDj7w";
    const THEIRS_PACKET: &str = "dVsYdDj7x";
    const THEIRS: &str = "dVsYdDj7y";
    let me = wire::Actor::Human(uuid("018f82d4-6f59-7d06-92e3-bce9131163b0"));
    let them = wire::Actor::Human(uuid("018f82d4-6f59-7d06-92e3-bce9131163b1"));
    let session = session(OPERATOR, EPOCH);
    let [mine_packet, mine] = conversed(
        1,
        MINE_PACKET,
        MINE,
        LANE,
        operator_route(None, Some("dDirection1")),
        &[],
    );
    let [their_packet, theirs] = conversed(
        3,
        THEIRS_PACKET,
        THEIRS,
        LANE,
        operator_route(None, Some("dDirection2")),
        &[],
    );
    let snapshot = [mine_packet, mine, their_packet, theirs];
    let mut runtime = conversation_runtime_as(&session, &me.to_string(), &snapshot);
    assert_eq!(
        runtime.begin_operator_item_texts(),
        vec![authority(MINE), authority(THEIRS)]
    );
    assert!(runtime.begin_operator_item_texts().is_empty());
    runtime.apply_conversation_text(
        &authority(MINE),
        &conversation_text_reply(
            &session,
            MINE,
            Some(wire::intent::RoomConcernedDirection {
                key: "dDirection1".to_owned(),
                text: "Which first?".to_owned(),
            }),
            Some(me.clone()),
        ),
    );
    runtime.apply_conversation_text(
        &authority(THEIRS),
        &conversation_text_reply(&session, THEIRS, None, Some(them)),
    );
    let inbox = runtime.operator_inbox();
    assert_eq!(inbox.len(), 1);
    assert_eq!(*inbox[0].item.value.record.publication(), authority(MINE));
    assert_eq!(
        runtime.item_concern(&inbox[0].item.value.record),
        app::ItemConcern::Direction {
            key: "dDirection1".to_owned(),
            text: Some("Which first?".to_owned())
        }
    );
}

/// The operator post at `position` under `publication`.
fn operator_post_at(position: u64, publication: &RoomAuthorityStamp) -> serde_json::Value {
    envelope(
        position,
        "room::published",
        &wire::rooms::RoomPublished {
            room: room(),
            publication: publication.clone(),
            content: wire::rooms::RoomPublicationContent::Authored {
                text: "Please inspect this room.".to_owned(),
            },
        },
    )
}

#[test]
fn a_nudge_message_its_lane_acknowledges_hears_the_posts_before_it() {
    const NUDGE: &str = "dVsYdDj7q";
    const DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
    const KEY: &str = "dNudgeKey1";
    let session = session(OPERATOR, EPOCH);
    let sent = envelope(
        2,
        "steer::sent",
        &serde_json::json!({
            "directive": DIRECTIVE,
            "fanout": DIRECTIVE,
            "key": KEY,
            "lane": SECOND_LANE,
            "text": "The operator pushed this Design to proposal.",
            "expires": "2030-04-13T18:44:09.271Z",
        }),
    );
    let posted = envelope(
        3,
        "room::posted",
        &RoomPosted {
            publication: authority(NUDGE),
            text: "The operator pushed this Design to proposal.".to_owned(),
            operation: RoomPublicationKind::Msg {},
            routes: vec![RoomRoute {
                destination: RoomDestination::Lane {
                    lane: uuid(SECOND_LANE),
                },
                context: Some(room()),
                direction: None,
            }],
            directed: vec![uuid(DIRECTIVE)],
            expires: instant("2030-04-13T18:44:09.271Z"),
        },
    );
    let nudged = envelope(
        4,
        "proposal::nudged",
        &wire::rooms::DesignNudged {
            design: room(),
            message: Some(authority(NUDGE)),
            recipient: None,
        },
    );
    let mut runtime = conversation_runtime(&session, &[room_post(1), sent, posted, nudged]);
    let later = RoomAuthorityStamp::for_micros(authority(NUDGE).micros() + 1)
        .expect("fixture later post authority");
    live(&mut runtime, &session, operator_post_at(5, &later));
    let unheard = |runtime: &AppRuntime, post: &RoomAuthorityStamp| {
        runtime.room_projection().unheard_post(&room(), post, false)
    };
    assert!(
        unheard(&runtime, &operator_post()),
        "an undelivered nudge proves nothing heard"
    );

    live(
        &mut runtime,
        &session,
        acked(6, DIRECTIVE, KEY, SECOND_LANE),
    );
    assert!(
        !unheard(&runtime, &operator_post()),
        "the acknowledged nudge carried the earlier post"
    );
    assert!(
        unheard(&runtime, &later),
        "a post after the nudge was never carried"
    );
}
