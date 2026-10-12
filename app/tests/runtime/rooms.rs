//! Room authority comes from each subscription's room snapshot and the live
//! facts after its horizon, never from browser-local foreground state.

use super::*;
use serde::Serialize;
use wire::rooms::{
    CandidateRef, CoverageRef, DecisionRef, ExecutionAuthorization, ExecutionAuthorized,
    RoomAuthorityStamp, RoomCoordinationStarted, RoomMessaged, RoomPutAside, RoomStopRequest,
    TextRange, WorkingGroup, WorkingGroupRemoved, WorkingGroupStored,
};

const ROOM_EVENT: &str = "dVsYdDj7d";
const COORDINATION_EVENT: &str = "dVsYdDj7k";
const GROUP: &str = "dVsYdDj7f";
const STOP_EVENT: &str = "dVsYdDj7g";
const CANDIDATE: &str = "dVsYdDj7h";
const COORDINATION_KEY: &str = "dCoordKey1";
const STOP_KEY: &str = "dStopKey1";

fn authority(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("fixture room authority")
}

fn envelope<T: Serialize>(position: u64, kind: &str, body: &T) -> serde_json::Value {
    let mut fact = serde_json::to_value(body)
        .expect("fact body serializes")
        .as_object()
        .expect("fact body is an object")
        .clone();
    fact.insert("kind".to_owned(), serde_json::json!(kind));
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": "thread:operator",
        "fact": fact,
    })
}

fn live_fact(fact: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "fact",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "fact": fact,
    }))
    .expect("live fact frame serializes")
}

fn room() -> wire::TaskStamp {
    fixture_stamp("PUBLIC-work1")
}

fn group(name: &str) -> WorkingGroup {
    WorkingGroup {
        group: authority(GROUP),
        name: name.to_owned(),
        anchors: vec![room()],
    }
}

fn group_stored(position: u64, name: &str) -> serde_json::Value {
    envelope(
        position,
        "group::stored",
        &WorkingGroupStored { group: group(name) },
    )
}

fn room_message(position: u64) -> serde_json::Value {
    let body = RoomMessaged {
        room: room(),
        publication: authority(ROOM_EVENT),
        hunk: wire::rooms::TranscriptHunk {
            head: wire::TranscriptSourceHead {
                lane: uuid(LANE),
                vendor: "omp".parse().expect("fixture vendor"),
                stream: "session".to_owned(),
                source: uuid(REQUEST),
                next: wire::body::TranscriptPosition {
                    member: 1,
                    offset: 64,
                },
            },
            locus: wire::MessageLocus {
                member: 1,
                offset: 8,
                length: 32,
                event: 0,
            },
            text: TextRange { start: 0, end: 12 },
        },
        // An occupant is not a logical lane; the projection must not use this
        // identity as its participant key.
        author: uuid(SECOND_LANE),
        directed: Vec::new(),
    };
    envelope(position, "room::messaged", &body)
}

/// The operator direction a coordination record follows, one position earlier.
fn direction(position: u64, key: &str, lane: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "directive": REQUEST,
        "fanout": REQUEST,
        "key": key,
        "lane": lane,
        "text": "Contribute the bounded review",
        "control": format!("room-coordinate:{}", room()),
        "expires": "2030-04-13T18:44:09.271Z",
    });
    envelope(position, "steer::sent", &body)
}

fn coordination(position: u64) -> serde_json::Value {
    let body = RoomCoordinationStarted {
        room: room(),
        publication: authority(COORDINATION_EVENT),
        key: COORDINATION_KEY.to_owned(),
        target: wire::OccupantAddress::new(seq(1)),
    };
    envelope(position, "room::directed", &body)
}

fn stop() -> RoomStopRequest {
    RoomStopRequest {
        key: STOP_KEY.to_owned(),
        target: wire::OccupantAddress::new(seq(1)),
        claim: wire::SequenceCode::new(3),
    }
}

fn put_aside(position: u64) -> serde_json::Value {
    let body = RoomPutAside {
        room: room(),
        publication: authority(STOP_EVENT),
        canceled: Vec::new(),
        stop: Some(stop()),
    };
    envelope(position, "room::shelved", &body)
}

fn authorization(decision: u64) -> ExecutionAuthorization {
    let candidate = CandidateRef {
        controller: room(),
        candidate: authority(CANDIDATE),
    };
    ExecutionAuthorization {
        decision: DecisionRef {
            controller: room(),
            decision: wire::SequenceCode::new(decision),
        },
        candidate: candidate.clone(),
        governed_inputs: vec![room()],
        selected_outputs: vec![fixture_stamp("HIDDEN-oops1")],
        coverage: vec![CoverageRef {
            candidate,
            coverage: wire::SequenceCode::new(1),
        }],
    }
}

fn authorized(position: u64, decision: u64) -> serde_json::Value {
    let body = ExecutionAuthorized {
        authorization: authorization(decision),
    };
    envelope(position, "proposal::authorized", &body)
}

/// The fact page an operator subscription opens with: live, empty, at its horizon.
fn operator_facts(session: &Session, horizon: u64) -> app::Admitted<StreamFrame> {
    admitted_stream(session, &packed_frame("facts", &[], None, horizon))
}

fn room_page(
    session: &Session,
    rows: &[serde_json::Value],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> app::Admitted<StreamFrame> {
    admitted_stream(session, &packed_frame("rooms", rows, overflow, horizon))
}

/// The fixture board with the room anchor under `project`, or absent, and
/// optionally held by `LANE` under the claim won at `claimed`.
fn room_board(
    session: &Session,
    project: Option<&str>,
    claimed: Option<u64>,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let original = board_response(session, "hidden");
    let Outcome::Reply(reply) = &original.get().outcome else {
        panic!("fixture board reply");
    };
    let mut body = reply.body.clone();
    let room = room();
    if let Some(project) = project {
        let task = body
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.born == room)
            .expect("room anchor is on the fixture board");
        task.statement.body.project = project.to_owned();
        task.statement.body.handle = wire::TaskHandle::for_project(project, &room);
        task.claim = claimed.map(|claimed| {
            serde_json::from_value(serde_json::json!({
                "position": claimed,
                "body": {
                    "actor": format!("lane:{LANE}"),
                    "fact": claimed,
                    "claimed": claimed,
                    "cell": {"project": project, "phase": "todo"},
                    "lane": LANE,
                    "occupant": REQUEST,
                    "epoch": EPOCH,
                    "disposition": "active",
                },
            }))
            .expect("fixture claim head")
        });
    } else {
        body.tasks.retain(|task| task.statement.body.born != room);
    }
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(4)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("room board response is admitted")
}

fn participants(runtime: &AppRuntime) -> Vec<app::RoomParticipant> {
    runtime
        .room_projection()
        .participants(&room(), runtime.board())
}

/// The frames an operator subscription opens with: its live-start fact page,
/// then a two-page room snapshot at the same horizon.
fn opening(session: &Session) -> [app::Admitted<StreamFrame>; 3] {
    [
        operator_facts(session, 8),
        room_page(
            session,
            &[group_stored(1, "Delivery"), room_message(2)],
            Some((4, 4)),
            8,
        ),
        room_page(
            session,
            &[
                authorized(4, 1),
                direction(5, COORDINATION_KEY, SECOND_LANE),
                coordination(6),
                put_aside(7),
            ],
            None,
            8,
        ),
    ]
}

#[test]
fn operator_subscription_rebuilds_room_authority_from_its_paged_room_snapshot() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    assert!(runtime.subjects().contains(&ViewSubject::Rooms));
    let [facts, first, last] = opening(&session);

    runtime.apply_stream(&facts);
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading,
        "a live-start fact page carries no room authority"
    );
    assert_eq!(
        runtime.apply_stream(&first),
        vec![StreamDirective::Continue {
            subject: ViewSubject::Rooms,
            cursor: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(4),
            },
        }]
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert!(runtime.apply_stream(&last).is_empty());
    let projection = runtime.room_projection();
    assert_eq!(projection.status(), app::RoomProjectionStatus::Available);
    assert_eq!(
        projection.groups()[&authority(GROUP)].value,
        group("Delivery")
    );
    assert_eq!(
        projection.preparation().authorizations[&authorization(1).decision].value,
        authorization(1)
    );
    let positions = projection
        .room(&room())
        .expect("room")
        .events()
        .iter()
        .map(|(id, event)| (id.clone(), event.position))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        positions,
        [(ROOM_EVENT, 2), (COORDINATION_EVENT, 6), (STOP_EVENT, 7)]
            .map(|(id, position)| (authority(id), seq(position)))
            .into()
    );
    assert_eq!(
        participants(&runtime),
        vec![
            app::RoomParticipant {
                lane: uuid(LANE),
                reasons: vec![app::ParticipantReason::Contribution(authority(ROOM_EVENT))],
            },
            app::RoomParticipant {
                lane: uuid(SECOND_LANE),
                reasons: vec![app::ParticipantReason::Coordination(
                    COORDINATION_KEY.to_owned()
                )],
            },
        ],
        "the coordination lane comes from its direction, never a message author occupant"
    );
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "no board snapshot proves the stopped claim active"
    );
}

#[test]
fn snapshot_stops_follow_the_board_and_later_live_facts_retire_authority() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(3)));
    assert_eq!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .map(|stop| &stop.value),
        Some(&stop())
    );
    assert_eq!(
        participants(&runtime)[0].reasons,
        vec![
            app::ParticipantReason::Contribution(authority(ROOM_EVENT)),
            app::ParticipantReason::Work(room()),
            app::ParticipantReason::Stop(STOP_KEY.to_owned()),
        ]
    );

    let canceled = envelope(
        9,
        "steer::canceled",
        &wire::body::SteerCanceled {
            key: COORDINATION_KEY.to_owned(),
            lane: uuid(SECOND_LANE),
        },
    );
    runtime.apply_stream(&admitted_stream(&session, &live_fact(canceled)));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(authorized(10, 2))));
    assert_eq!(
        participants(&runtime)
            .iter()
            .map(|participant| participant.lane)
            .collect::<Vec<_>>(),
        vec![uuid(LANE)],
        "a retired coordination no longer places its lane in the room"
    );
    assert_eq!(
        runtime.room_projection().preparation().authorizations.len(),
        2
    );

    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(5)));
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "a later claim win leaves the stop in the timeline but not current"
    );
    assert_eq!(
        runtime
            .room_projection()
            .room(&room())
            .expect("room")
            .events()[&authority(STOP_EVENT)]
            .position,
        seq(7)
    );
}

#[test]
fn same_scope_reconnect_rebuilds_from_the_new_snapshot_with_stable_identities() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let mut other_browser = AppRuntime::new(&accepted);
    for browser in [&mut runtime, &mut other_browser] {
        browser.apply_stream(&operator_facts(&session, 2));
        browser.apply_stream(&room_page(
            &session,
            &[group_stored(1, "Delivery"), room_message(2)],
            None,
            2,
        ));
    }
    let removal = admitted_stream(
        &session,
        &live_fact(envelope(
            3,
            "group::removed",
            &WorkingGroupRemoved {
                group: authority(GROUP),
            },
        )),
    );
    runtime.apply_stream(&removal);
    other_browser.apply_stream(&removal);
    for browser in [&runtime, &other_browser] {
        assert!(browser.room_projection().groups().is_empty());
        assert!(
            browser
                .room_projection()
                .room(&room())
                .expect("room")
                .events()
                .contains_key(&authority(ROOM_EVENT)),
            "shared catalog edits never replace, mount, reorder, or close room cards"
        );
    }

    // The socket drops; a renamed group and a coordination land unseen.
    assert!(!runtime.reconnect(&accepted));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert!(runtime.room_projection().room(&room()).is_none());
    runtime.apply_stream(&operator_facts(&session, 7));
    runtime.apply_stream(&room_page(
        &session,
        &[
            room_message(2),
            group_stored(4, "Renamed"),
            direction(5, COORDINATION_KEY, SECOND_LANE),
            coordination(6),
        ],
        None,
        7,
    ));
    let projection = runtime.room_projection();
    assert_eq!(projection.status(), app::RoomProjectionStatus::Available);
    assert_eq!(
        projection.groups()[&authority(GROUP)].value,
        group("Renamed")
    );
    assert_eq!(projection.groups()[&authority(GROUP)].position, seq(4));
    let events = projection.room(&room()).expect("room").events();
    assert_eq!(events[&authority(ROOM_EVENT)].position, seq(2));
    assert!(events.contains_key(&authority(COORDINATION_EVENT)));

    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Unavailable
    );
    runtime.apply_board(&room_board(&session, None, None));
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Missing
    );
    runtime.apply_board(&room_board(&session, Some("app.rooms"), None));
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Available(wire::TaskHandle::for_project("app.rooms", &room()))
    );
    assert!(
        runtime
            .room_projection()
            .room(&room())
            .expect("re-homed room")
            .events()
            .contains_key(&authority(ROOM_EVENT)),
        "a re-homed anchor keeps its durable room cards"
    );
}

#[test]
fn room_snapshots_refuse_moved_horizons_regressions_and_live_gaps() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let requires_snapshot = |runtime: &AppRuntime| {
        runtime.room_projection().status() == app::RoomProjectionStatus::RequiresSnapshot
    };

    runtime.apply_stream(&room_page(
        &session,
        &[group_stored(1, "A")],
        Some((1, 3)),
        4,
    ));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[room_message(3)], None, 5)),
        vec![StreamDirective::Reconnect],
        "every page of one snapshot declares the same horizon"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(
        &session,
        &[group_stored(1, "A")],
        Some((1, 3)),
        4,
    ));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[room_message(2)], None, 4)),
        vec![StreamDirective::Reconnect],
        "a continuation never moves backward"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(group_stored(6, "B"))));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[group_stored(1, "A")], None, 5)),
        vec![StreamDirective::Reconnect],
        "a live fact past the snapshot horizon before completion is a gap"
    );
    assert!(requires_snapshot(&runtime));

    // The runtime already admitted live position 6; overlap is judged above it.
    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(&session, &[group_stored(1, "A")], None, 8));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    assert_eq!(
        runtime.apply_stream(&admitted_stream(&session, &live_fact(group_stored(7, "B")))),
        vec![StreamDirective::Reconnect],
        "a live fact at or before the horizon overlaps the snapshot"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(&session, &[group_stored(1, "A")], None, 8));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(group_stored(9, "B"))));
    assert_eq!(
        runtime.room_projection().groups()[&authority(GROUP)].value,
        group("B")
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
}
