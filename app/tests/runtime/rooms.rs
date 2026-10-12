//! Room authority is a fact projection, never browser-local foreground state.

use super::*;
use serde::Serialize;
use wire::rooms::{
    CandidateRef, CoverageRef, DecisionRef, ExecutionAuthorization, ExecutionAuthorized,
    OutputControlled, RoomAuthorityStamp, RoomMessaged, RoomPutAside, RoomStopRequest, TextRange,
    WorkingGroup, WorkingGroupRemoved, WorkingGroupStored,
};

const ROOM_EVENT: &str = "dVsYdDj7d";
const GROUP: &str = "dVsYdDj7f";
const STOP_EVENT: &str = "dVsYdDj7g";
const CANDIDATE: &str = "dVsYdDj7h";

fn authority(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("fixture room authority")
}

fn envelope<T: Serialize>(
    position: u64,
    actor: &str,
    origin: &str,
    kind: &str,
    body: &T,
) -> serde_json::Value {
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
        "actor": actor,
        "origin": origin,
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

fn room_message(room: wire::TaskStamp) -> RoomMessaged {
    RoomMessaged {
        room,
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
    }
}

fn room_board_response(
    session: &Session,
    room: &wire::TaskStamp,
    project: Option<&str>,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let original = board_response(session, "hidden");
    let Outcome::Reply(reply) = &original.get().outcome else {
        panic!("fixture board reply");
    };
    let mut body = reply.body.clone();
    if let Some(project) = project {
        let task = body
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.born == *room)
            .expect("room anchor is on the fixture board");
        task.statement.position = seq(5);
        task.statement.body.project = project.to_owned();
        task.statement.body.handle = wire::TaskHandle::for_project(project, room);
    } else {
        body.tasks.retain(|task| task.statement.body.born != *room);
    }
    body.cursor = seq(5);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(5)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("room board response is admitted")
}

#[test]
fn replay_group_edits_and_rehomed_anchors_preserve_room_card_identity() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let mut other_browser = AppRuntime::new(&accepted);
    let room = fixture_stamp("PUBLIC-work1");
    let group = WorkingGroup {
        group: authority(GROUP),
        name: "Delivery".to_owned(),
        anchors: vec![room.clone()],
    };
    let rows = vec![
        envelope(
            1,
            "operator",
            "thread:operator",
            "group::stored",
            &WorkingGroupStored {
                group: group.clone(),
            },
        ),
        envelope(
            2,
            &format!("lane:{LANE}"),
            &format!("task:{room}"),
            "room::messaged",
            &room_message(room.clone()),
        ),
    ];
    let snapshot = admitted_stream(&session, &packed_frame("facts", &rows, None, 2));

    runtime.apply_stream(&snapshot);
    other_browser.apply_stream(&snapshot);
    runtime.apply_stream(&snapshot);
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    assert_eq!(
        runtime.room_projection().groups()[&authority(GROUP)].value,
        group
    );
    let card = &runtime
        .room_projection()
        .room(&room)
        .expect("room")
        .events()[&authority(ROOM_EVENT)];
    assert_eq!(card.position, seq(2));
    assert_eq!(
        runtime
            .room_projection()
            .participants(&room, runtime.board()),
        vec![app::RoomParticipant {
            lane: uuid(LANE),
            reasons: vec![app::ParticipantReason::Contribution(authority(ROOM_EVENT))],
        }],
        "a transcript lane is not replaced by its occupant or by a team"
    );

    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room, runtime.board()),
        app::RoomAnchorAuthority::Unavailable
    );
    let missing = room_board_response(&session, &room, None);
    runtime.apply_board(&missing);
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room, runtime.board()),
        app::RoomAnchorAuthority::Missing
    );
    let board = room_board_response(&session, &room, Some("app.rooms"));
    runtime.apply_board(&board);
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room, runtime.board()),
        app::RoomAnchorAuthority::Available(wire::TaskHandle::for_project("app.rooms", &room))
    );

    let removal = envelope(
        3,
        "operator",
        "thread:operator",
        "group::removed",
        &WorkingGroupRemoved {
            group: authority(GROUP),
        },
    );
    let removal = admitted_stream(&session, &live_fact(removal));
    runtime.apply_stream(&removal);
    other_browser.apply_stream(&removal);
    assert!(runtime.room_projection().groups().is_empty());
    assert!(other_browser.room_projection().groups().is_empty());
    assert!(
        runtime
            .room_projection()
            .room(&room)
            .expect("room")
            .events()
            .contains_key(&authority(ROOM_EVENT)),
        "shared catalog edits do not replace, mount, reorder, or close room cards"
    );
}

#[test]
fn packed_continuation_regression_requires_a_fresh_snapshot() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let room = fixture_stamp("gap-room");
    let stored = envelope(
        1,
        "operator",
        "thread:operator",
        "group::stored",
        &WorkingGroupStored {
            group: WorkingGroup {
                group: authority(GROUP),
                name: "Gap".to_owned(),
                anchors: vec![room],
            },
        },
    );
    let first = admitted_stream(
        &session,
        &packed_frame("facts", std::slice::from_ref(&stored), Some((2, 2)), 4),
    );
    assert_eq!(
        runtime.apply_stream(&first),
        vec![StreamDirective::Continue {
            subject: ViewSubject::Facts,
            cursor: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(2),
            },
        }]
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );

    let regressed = admitted_stream(
        &session,
        &packed_frame("facts", std::slice::from_ref(&stored), None, 4),
    );
    assert_eq!(
        runtime.apply_stream(&regressed),
        vec![StreamDirective::Reconnect]
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::RequiresSnapshot
    );
    assert_eq!(runtime.resume(), None);

    assert!(!runtime.reconnect(&accepted));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert_eq!(runtime.resume(), None);
    runtime.apply_stream(&admitted_stream(
        &session,
        &packed_frame("facts", &[stored], None, 1),
    ));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    assert_eq!(runtime.room_projection().groups().len(), 1);
}

struct AuthorityFixture {
    session: Session,
    runtime: AppRuntime,
    room: wire::TaskStamp,
    output: wire::TaskStamp,
    stop: RoomStopRequest,
    preparation: wire::TaskPreparation,
    candidate: CandidateRef,
    decision: DecisionRef,
    authorization: ExecutionAuthorization,
}

fn authority_fixture() -> AuthorityFixture {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let room: wire::TaskStamp = "1kKRRsDP".parse().expect("fixture room");
    let output: wire::TaskStamp = "1kKRRsDR".parse().expect("fixture output");
    let claim = serde_json::json!({
        "task": room,
        "actor": "operator",
        "lane": LANE,
        "occupant": REQUEST,
        "epoch": EPOCH,
        "until": "2030-04-12T19:44:09.271Z",
        "cell": {"project": "app", "phase": "todo"},
        "source": {"proven": {"heads": []}},
    });
    let claim = envelope(1, "operator", &format!("task:{room}"), "claim::won", &claim);
    let stop = RoomStopRequest {
        key: "dStopKey1".to_owned(),
        target: wire::OccupantAddress::new(seq(9)),
        claim: wire::SequenceCode::new(1),
    };
    let put_aside = envelope(
        2,
        "operator",
        "thread:operator",
        "room::shelved",
        &RoomPutAside {
            room: room.clone(),
            publication: authority(STOP_EVENT),
            canceled: Vec::new(),
            stop: Some(stop.clone()),
        },
    );
    let preparation = wire::TaskPreparation {
        parent: room.clone(),
        controller: room.clone(),
        construction: room.clone(),
    };
    let controlled = envelope(
        3,
        "operator",
        &format!("task:{output}"),
        "task::controlled",
        &OutputControlled {
            output: output.clone(),
            preparation: preparation.clone(),
        },
    );
    let candidate = CandidateRef {
        controller: room.clone(),
        candidate: authority(CANDIDATE),
    };
    let decision = DecisionRef {
        controller: room.clone(),
        decision: wire::SequenceCode::new(1),
    };
    let authorization = ExecutionAuthorization {
        decision: decision.clone(),
        candidate: candidate.clone(),
        governed_inputs: vec![room.clone()],
        selected_outputs: vec![output.clone()],
        coverage: vec![CoverageRef {
            candidate: candidate.clone(),
            coverage: wire::SequenceCode::new(1),
        }],
    };
    let authorized = envelope(
        4,
        "operator",
        &format!("task:{room}"),
        "proposal::authorized",
        &ExecutionAuthorized {
            authorization: authorization.clone(),
        },
    );
    runtime.apply_stream(&admitted_stream(
        &session,
        &packed_frame(
            "facts",
            &[claim, put_aside, controlled, authorized],
            None,
            4,
        ),
    ));
    AuthorityFixture {
        session,
        runtime,
        room,
        output,
        stop,
        preparation,
        candidate,
        decision,
        authorization,
    }
}

#[test]
fn stop_and_authorization_authority_follow_exact_native_identities() {
    let AuthorityFixture {
        session,
        mut runtime,
        room,
        output,
        stop,
        preparation,
        candidate,
        decision,
        authorization,
    } = authority_fixture();

    assert_eq!(
        runtime
            .room_projection()
            .current_stop(&room)
            .expect("live stop")
            .value,
        stop
    );
    assert_eq!(
        runtime.room_projection().preparation().controls[&output].value,
        preparation
    );
    assert_eq!(
        runtime.room_projection().preparation().authorizations[&decision].value,
        authorization
    );

    let next_decision = DecisionRef {
        controller: room.clone(),
        decision: wire::SequenceCode::new(2),
    };
    let next_authorization = ExecutionAuthorization {
        decision: next_decision.clone(),
        candidate,
        governed_inputs: vec![room.clone()],
        selected_outputs: vec![output],
        coverage: vec![CoverageRef {
            candidate: CandidateRef {
                controller: room.clone(),
                candidate: authority(CANDIDATE),
            },
            coverage: wire::SequenceCode::new(1),
        }],
    };
    let changed = envelope(
        5,
        "operator",
        &format!("task:{room}"),
        "proposal::authorized",
        &ExecutionAuthorized {
            authorization: next_authorization.clone(),
        },
    );
    runtime.apply_stream(&admitted_stream(&session, &live_fact(changed)));
    assert_eq!(
        runtime.room_projection().preparation().authorizations[&next_decision].value,
        next_authorization
    );

    let done = envelope(
        6,
        &format!("lane:{LANE}"),
        &format!("task:{room}"),
        "task::done",
        &serde_json::json!({
            "task": room,
            "validation": "complete",
            "source": {"proven": {"heads": []}},
        }),
    );
    runtime.apply_stream(&admitted_stream(&session, &live_fact(done)));
    assert!(runtime.room_projection().current_stop(&room).is_none());
    assert!(
        runtime
            .room_projection()
            .room(&room)
            .expect("room")
            .events()
            .contains_key(&authority(STOP_EVENT)),
        "claim disposition clears current stop without deleting its timeline event"
    );
}
